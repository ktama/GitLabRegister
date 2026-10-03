//! GitLab-EE の応答を 8 分類へ正規化する (設計書 6.2)。
//!
//! 呼び出し方式 (REST と GraphQL) の違いは分類に現れない。
//! 画面とログは分類だけを見て振る舞いを決め、状態コードを直接解釈しない。

use crate::logger::redact_token_patterns;
use std::time::Duration;

/// 設計書 6.2 の 8 分類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// 認証が拒否された。再試行せず PAT の再入力を求める (EC-11)。
    Authentication,
    /// 操作が許可されていない。該当件を失敗とし他は続行する (EC-12)。
    Authorization,
    /// 値が受け付けられなかった (EC-13、EC-14、EC-33)。
    Validation,
    /// 対象が見つからない (EC-29)。
    NotFound,
    /// 要求数の上限超過 (EC-18)。待機時間の指示があれば保持する。
    RateLimited { retry_after: Option<Duration> },
    /// サーバー側エラー (EC-17)。
    Transient,
    /// 接続できない、応答がない (EC-10、EC-16)。
    Unreachable,
    /// プロキシが認証を拒否した (EC-34)。
    ProxyAuthentication,
}

impl ErrorKind {
    /// この分類で再試行するか (設計書 6.2)。
    pub fn is_retryable(&self) -> bool {
        matches!(self, ErrorKind::RateLimited { .. } | ErrorKind::Transient)
    }

    /// 一括登録の全体を止める分類か。設計書 8.1 によりこの 2 つに限る。
    pub fn aborts_batch(&self) -> bool {
        matches!(self, ErrorKind::Unreachable | ErrorKind::Authentication)
    }

    /// 利用者が次に取れる操作 (NFR-16)。
    pub fn next_action(&self) -> &'static str {
        match self {
            ErrorKind::Authentication => "パーソナルアクセストークンを再入力してください",
            ErrorKind::Authorization => "登録先プロジェクトまたは担当者を見直してください",
            ErrorKind::Validation => "表示された項目を修正して再実行してください",
            ErrorKind::NotFound => "対象が GitLab-EE 上に存在するか確認してください",
            ErrorKind::RateLimited { .. } => "時間をおいて再実行してください",
            ErrorKind::Transient => "時間をおいて再実行してください",
            ErrorKind::Unreachable => "接続先 URL とネットワークの状態を確認してください",
            ErrorKind::ProxyAuthentication => "プロキシの設定と資格情報を見直してください",
        }
    }
}

/// 正規化された失敗。
///
/// `message` は画面とログに出す。生成の時点でトークンらしい文字列を伏せるため、
/// 応答に認証情報が含まれていてもそのまま表示・記録されない (NFR-07)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabError {
    pub kind: ErrorKind,
    pub message: String,
    /// 失敗した操作の名称。ログの「操作の種別」に使う (FR-17)。
    pub operation: String,
}

impl GitLabError {
    pub fn new(kind: ErrorKind, operation: impl Into<String>, message: impl AsRef<str>) -> Self {
        Self {
            kind,
            message: redact_token_patterns(message.as_ref()),
            operation: operation.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

impl std::fmt::Display for GitLabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.operation, self.message)
    }
}

impl std::error::Error for GitLabError {}

/// 状態コードと応答本文から分類を決める。
///
/// 待機時間の指示 (`Retry-After`) は秒数とみなす。解釈できない値は指示なしとして扱う。
pub fn classify_status(status: u16, retry_after_header: Option<&str>) -> ErrorKind {
    match status {
        401 => ErrorKind::Authentication,
        403 => ErrorKind::Authorization,
        404 => ErrorKind::NotFound,
        407 => ErrorKind::ProxyAuthentication,
        429 => ErrorKind::RateLimited {
            retry_after: parse_retry_after(retry_after_header),
        },
        400 | 422 => ErrorKind::Validation,
        s if (500..600).contains(&s) => ErrorKind::Transient,
        // 上記に当てはまらない 4xx は、値が受け付けられなかったものとして扱う。
        s if (400..500).contains(&s) => ErrorKind::Validation,
        _ => ErrorKind::Transient,
    }
}

fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let seconds: u64 = value?.trim().parse().ok()?;
    Some(Duration::from_secs(seconds))
}

/// GitLab-EE の応答本文から、利用者に示す理由を取り出す。
///
/// GitLab は `message` または `error` に理由を入れる。どちらもなければ本文をそのまま使う。
/// 本文が長い場合は切り詰める。画面とログを読めなくしないためである。
pub fn extract_message(body: &str) -> String {
    const MAX_CHARS: usize = 500;
    let text = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => extract_from_json(&value).unwrap_or_else(|| body.trim().to_string()),
        Err(_) => body.trim().to_string(),
    };
    if text.chars().count() > MAX_CHARS {
        text.chars().take(MAX_CHARS).collect::<String>() + "…"
    } else {
        text
    }
}

fn extract_from_json(value: &serde_json::Value) -> Option<String> {
    for key in ["message", "error", "error_description"] {
        match value.get(key) {
            Some(serde_json::Value::String(s)) => return Some(s.clone()),
            Some(other @ serde_json::Value::Object(_)) => {
                // GitLab は項目ごとの検証エラーを {"title": ["is too long ..."]} の形で返す。
                let parts: Vec<String> = other
                    .as_object()?
                    .iter()
                    .map(|(field, reasons)| format!("{field}: {}", join_reasons(reasons)))
                    .collect();
                if !parts.is_empty() {
                    return Some(parts.join(" / "));
                }
            }
            Some(serde_json::Value::Array(items)) => {
                let parts: Vec<String> = items
                    .iter()
                    .filter_map(|i| i.as_str().map(|s| s.to_string()))
                    .collect();
                if !parts.is_empty() {
                    return Some(parts.join(" / "));
                }
            }
            _ => {}
        }
    }
    None
}

fn join_reasons(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|i| i.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 設計書 6.2: 状態コードが 8 分類へ正規化される。
    #[test]
    fn error_classify_status_covers_all_kinds() {
        assert_eq!(classify_status(401, None), ErrorKind::Authentication);
        assert_eq!(classify_status(403, None), ErrorKind::Authorization);
        assert_eq!(classify_status(404, None), ErrorKind::NotFound);
        assert_eq!(classify_status(407, None), ErrorKind::ProxyAuthentication);
        assert_eq!(classify_status(400, None), ErrorKind::Validation);
        assert_eq!(classify_status(422, None), ErrorKind::Validation);
        assert_eq!(classify_status(500, None), ErrorKind::Transient);
        assert_eq!(classify_status(503, None), ErrorKind::Transient);
        assert_eq!(
            classify_status(429, None),
            ErrorKind::RateLimited { retry_after: None }
        );
    }

    /// EC-18: 待機時間の指示を秒数として取り出す。
    #[test]
    fn error_classify_rate_limited_keeps_retry_after() {
        assert_eq!(
            classify_status(429, Some("30")),
            ErrorKind::RateLimited {
                retry_after: Some(Duration::from_secs(30))
            }
        );
        // 解釈できない値は指示なしとして扱う。
        assert_eq!(
            classify_status(429, Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            ErrorKind::RateLimited { retry_after: None }
        );
    }

    /// 既知でない 4xx は入力エラーとして扱う。
    #[test]
    fn error_classify_unknown_4xx_is_validation() {
        assert_eq!(classify_status(409, None), ErrorKind::Validation);
        assert_eq!(classify_status(418, None), ErrorKind::Validation);
    }

    /// 設計書 6.2: 再試行する分類は 2 つに限る。
    #[test]
    fn error_only_transient_and_rate_limited_are_retryable() {
        assert!(ErrorKind::Transient.is_retryable());
        assert!(ErrorKind::RateLimited { retry_after: None }.is_retryable());
        for kind in [
            ErrorKind::Authentication,
            ErrorKind::Authorization,
            ErrorKind::Validation,
            ErrorKind::NotFound,
            ErrorKind::Unreachable,
            ErrorKind::ProxyAuthentication,
        ] {
            assert!(!kind.is_retryable(), "{kind:?} を再試行してはいけない");
        }
    }

    /// 設計書 8.1: 一括登録の全体を止める分類は 2 つに限る。
    #[test]
    fn error_only_unreachable_and_authentication_abort_batch() {
        assert!(ErrorKind::Unreachable.aborts_batch());
        assert!(ErrorKind::Authentication.aborts_batch());
        for kind in [
            ErrorKind::Authorization,
            ErrorKind::Validation,
            ErrorKind::NotFound,
            ErrorKind::Transient,
            ErrorKind::RateLimited { retry_after: None },
            ErrorKind::ProxyAuthentication,
        ] {
            assert!(!kind.aborts_batch(), "{kind:?} で全体を止めてはいけない");
        }
    }

    /// NFR-16: すべての分類に、利用者が次に取れる操作がある。
    #[test]
    fn error_every_kind_has_next_action() {
        for kind in [
            ErrorKind::Authentication,
            ErrorKind::Authorization,
            ErrorKind::Validation,
            ErrorKind::NotFound,
            ErrorKind::RateLimited { retry_after: None },
            ErrorKind::Transient,
            ErrorKind::Unreachable,
            ErrorKind::ProxyAuthentication,
        ] {
            assert!(!kind.next_action().is_empty(), "{kind:?} に案内がない");
        }
    }

    /// NFR-07: 応答に認証情報が含まれていても、失敗の表示に残らない。
    #[test]
    fn error_message_redacts_token_in_response() {
        let err = GitLabError::new(
            ErrorKind::Authentication,
            "接続",
            "invalid token glpat-abcdefghijklmnop",
        );
        assert!(
            !err.message.contains("glpat-abcdefghijklmnop"),
            "トークンが残っている: {}",
            err.message
        );
    }

    /// EC-33: 項目ごとの検証エラーから、項目名と理由を取り出す。
    #[test]
    fn error_extract_message_from_field_errors() {
        let body = r#"{"message":{"title":["is too long (maximum is 255 characters)"]}}"#;
        let got = extract_message(body);
        assert!(got.contains("title"), "{got}");
        assert!(got.contains("255"), "{got}");
    }

    #[test]
    fn error_extract_message_from_plain_message() {
        let body = r#"{"message":"404 Project Not Found"}"#;
        assert_eq!(extract_message(body), "404 Project Not Found");
    }

    #[test]
    fn error_extract_message_from_error_key() {
        let body = r#"{"error":"insufficient_scope"}"#;
        assert_eq!(extract_message(body), "insufficient_scope");
    }

    #[test]
    fn error_extract_message_from_message_array() {
        let body = r#"{"message":["Milestone not found","Label invalid"]}"#;
        let got = extract_message(body);
        assert!(got.contains("Milestone not found"), "{got}");
        assert!(got.contains("Label invalid"), "{got}");
    }

    /// JSON でない本文はそのまま使う。
    #[test]
    fn error_extract_message_from_non_json() {
        assert_eq!(extract_message("  502 Bad Gateway  "), "502 Bad Gateway");
    }

    /// 長い本文は切り詰める。画面とログを読めなくしないため。
    #[test]
    fn error_extract_message_truncates_long_body() {
        let body = "あ".repeat(1_000);
        let got = extract_message(&body);
        assert_eq!(got.chars().count(), 501, "切り詰めていない");
        assert!(got.ends_with('…'));
    }
}
