//! 操作ログの出力とローテーション (FR-17、NFR-07、NFR-10)。
//!
//! 秘密情報の除去は本モジュールの内部で行う。
//! SecretHolder からログへ値が渡る経路は作らず、伏せる対象の文字列だけを受け取る。

use chrono::NaiveDateTime;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 1 ファイルの上限 (NFR-10)。
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// 保持する世代数 (NFR-10)。
pub const MAX_GENERATIONS: usize = 5;
/// 伏せ字に置き換える文字列。
pub const REDACTED: &str = "(伏せ字)";

/// ログの重大度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// 1 件のログ (FR-17 の記録項目)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord<'a> {
    pub at: NaiveDateTime,
    pub level: Level,
    /// 操作の種別。接続、展開、一括登録、設定の変更などを表す。
    pub operation: &'a str,
    /// 対象の識別情報。定義名、発生日、プロジェクトなど。
    pub target: &'a str,
    /// 結果。
    pub outcome: &'a str,
}

/// ファイルへ出力するロガー。
#[derive(Debug)]
pub struct Logger {
    path: PathBuf,
    max_bytes: u64,
    max_generations: usize,
    /// 伏せる対象の文字列。SecretHolder から受け取る (NFR-07)。
    redaction_targets: Vec<String>,
}

impl Logger {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_bytes: MAX_FILE_BYTES,
            max_generations: MAX_GENERATIONS,
            redaction_targets: Vec::new(),
        }
    }

    /// 上限と世代数を変えて作る。試験でローテーションを確かめるために使う。
    pub fn with_limits(path: impl Into<PathBuf>, max_bytes: u64, max_generations: usize) -> Self {
        Self {
            path: path.into(),
            max_bytes,
            max_generations,
            redaction_targets: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 伏せる対象を設定する。接続時と PAT の再入力時に呼ぶ。
    pub fn set_redaction_targets(&mut self, targets: Vec<String>) {
        // 長いものから置き換える。短い文字列が長い文字列の一部である場合の取りこぼしを防ぐ。
        let mut targets = targets;
        targets.retain(|t| !t.is_empty());
        targets.sort_by_key(|t| std::cmp::Reverse(t.len()));
        self.redaction_targets = targets;
    }

    /// 1 件を書き出す。上限に達していれば先にローテーションする。
    pub fn write(&self, record: &LogRecord<'_>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.rotate_if_needed()?;
        let line = format!(
            "{} [{}] {} | {} | {}\n",
            record.at.format("%Y-%m-%d %H:%M:%S"),
            record.level.label(),
            self.redact(record.operation),
            self.redact(record.target),
            self.redact(record.outcome),
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())
    }

    /// 秘密情報を伏せ字へ置き換える (NFR-07)。
    ///
    /// 保持している値そのものに加え、GitLab のトークンに使われる前置詞と
    /// 認証ヘッダーの形も伏せる。応答に認証情報が含まれて渡された場合に備える。
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for target in &self.redaction_targets {
            out = out.replace(target.as_str(), REDACTED);
        }
        out = redact_token_patterns(&out);
        out
    }

    fn rotate_if_needed(&self) -> std::io::Result<()> {
        let size = match fs::metadata(&self.path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if size < self.max_bytes {
            return Ok(());
        }
        // 古い世代から順に押し出す。上限を超えた世代は削除する。
        let oldest = self.generation_path(self.max_generations);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for generation in (1..self.max_generations).rev() {
            let from = self.generation_path(generation);
            if from.exists() {
                fs::rename(&from, self.generation_path(generation + 1))?;
            }
        }
        fs::rename(&self.path, self.generation_path(1))
    }

    fn generation_path(&self, generation: usize) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "app".to_string());
        let extension = self
            .path
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "log".to_string());
        let name = format!("{stem}.{generation}.{extension}");
        match self.path.parent() {
            Some(parent) => parent.join(name),
            None => PathBuf::from(name),
        }
    }
}

/// トークンらしい文字列を伏せる。
///
/// 保持している値と一致しない経路 (GitLab の応答に含まれる別のトークンなど) でも
/// ログへ残らないようにするための二重の防御である。
pub fn redact_token_patterns(text: &str) -> String {
    const PREFIXES: [&str; 4] = ["glpat-", "gloas-", "PRIVATE-TOKEN: ", "Bearer "];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: loop {
        let mut earliest: Option<(usize, &str)> = None;
        for prefix in PREFIXES {
            if let Some(at) = rest.find(prefix) {
                if earliest.is_none_or(|(e, _)| at < e) {
                    earliest = Some((at, prefix));
                }
            }
        }
        match earliest {
            Some((at, prefix)) => {
                out.push_str(&rest[..at]);
                out.push_str(REDACTED);
                let after = &rest[at + prefix.len()..];
                // 値の終わりは空白・改行・引用符とみなす。
                let end = after
                    .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                    .unwrap_or(after.len());
                rest = &after[end..];
            }
            None => break 'outer,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::{ProxyCredentials, SecretHolder};
    use chrono::NaiveDate;

    fn at() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 3)
            .expect("有効な日付")
            .and_hms_opt(14, 30, 0)
            .expect("有効な時刻")
    }

    fn record<'a>(operation: &'a str, target: &'a str, outcome: &'a str) -> LogRecord<'a> {
        LogRecord {
            at: at(),
            level: Level::Info,
            operation,
            target,
            outcome,
        }
    }

    /// FR-17: 日時・操作の種別・対象・結果が記録される。
    #[test]
    fn logger_writes_required_fields() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::new(dir.path().join("app.log"));
        logger
            .write(&record(
                "一括登録",
                "月次報告 2026-10-05 group/project",
                "成功 #123",
            ))
            .expect("書ける");
        let text = fs::read_to_string(logger.path()).expect("読める");
        assert!(text.contains("2026-10-03 14:30:00"), "{text}");
        assert!(text.contains("INFO"), "{text}");
        assert!(text.contains("一括登録"), "{text}");
        assert!(text.contains("月次報告 2026-10-05 group/project"), "{text}");
        assert!(text.contains("成功 #123"), "{text}");
    }

    /// FR-17: 20 件の一括登録がそれぞれ記録される。
    #[test]
    fn logger_writes_one_line_per_record() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::new(dir.path().join("app.log"));
        for i in 0..20 {
            let target = format!("定義{i} 2026-10-05 group/project");
            logger
                .write(&record("一括登録", &target, "成功"))
                .expect("書ける");
        }
        let text = fs::read_to_string(logger.path()).expect("読める");
        assert_eq!(text.lines().count(), 20);
    }

    /// NFR-07 / FR-17: 保持しているトークンはログに現れない。
    #[test]
    fn logger_redacts_held_token() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let mut holder = SecretHolder::new();
        holder.set_token("glpat-abcdefghijklmnop");
        holder.set_proxy_credentials(ProxyCredentials::new("taro", "p@ssw0rd"));

        let mut logger = Logger::new(dir.path().join("app.log"));
        logger.set_redaction_targets(holder.redaction_targets());
        logger
            .write(&record(
                "接続",
                "https://gitlab.example.com",
                "失敗 token=glpat-abcdefghijklmnop pass=p@ssw0rd",
            ))
            .expect("書ける");

        let text = fs::read_to_string(logger.path()).expect("読める");
        assert!(
            !text.contains("glpat-abcdefghijklmnop"),
            "トークンがログに出ている: {text}"
        );
        assert!(
            !text.contains("p@ssw0rd"),
            "パスワードがログに出ている: {text}"
        );
        assert!(text.contains(REDACTED), "{text}");
    }

    /// NFR-07: 保持していないトークンでも、形から判断して伏せる。
    #[test]
    fn logger_redacts_unknown_token_patterns() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::new(dir.path().join("app.log"));
        logger
            .write(&record(
                "接続",
                "https://gitlab.example.com",
                r#"応答に PRIVATE-TOKEN: zzzzzzzzzz と Bearer yyyyyyyy が含まれる"#,
            ))
            .expect("書ける");
        let text = fs::read_to_string(logger.path()).expect("読める");
        assert!(!text.contains("zzzzzzzzzz"), "{text}");
        assert!(!text.contains("yyyyyyyy"), "{text}");
    }

    #[test]
    fn logger_redact_keeps_surrounding_text() {
        let mut logger = Logger::new("dummy.log");
        logger.set_redaction_targets(vec!["glpat-secret".into()]);
        let got = logger.redact("前 glpat-secret 後");
        assert_eq!(got, format!("前 {REDACTED} 後"));
    }

    #[test]
    fn logger_redact_without_secret_is_unchanged() {
        let logger = Logger::new("dummy.log");
        assert_eq!(logger.redact("通常のログ 2026-10-05"), "通常のログ 2026-10-05");
    }

    /// NFR-10: 上限に達すると世代が切り替わり、過去の世代が残る。
    #[test]
    fn logger_rotates_at_limit_and_keeps_generations() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        // 上限を小さくして 3 世代で確かめる。
        let logger = Logger::with_limits(dir.path().join("app.log"), 200, 3);
        for i in 0..60 {
            let target = format!("定義{i} 2026-10-05 group/project 長めの対象名で桁を稼ぐ");
            logger
                .write(&record("一括登録", &target, "成功"))
                .expect("書ける");
        }
        assert!(logger.path().exists(), "現行のログがない");
        assert!(dir.path().join("app.1.log").exists(), "1 世代前がない");
        assert!(dir.path().join("app.2.log").exists(), "2 世代前がない");
        assert!(dir.path().join("app.3.log").exists(), "3 世代前がない");
        // 上限の世代数を超えたファイルは残さない。
        assert!(
            !dir.path().join("app.4.log").exists(),
            "世代数の上限を超えたファイルが残っている"
        );
    }

    /// NFR-10: ローテーション後も現行のログへ書き続けられる。
    #[test]
    fn logger_continues_writing_after_rotation() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::with_limits(dir.path().join("app.log"), 120, 2);
        for i in 0..10 {
            let target = format!("定義{i} 2026-10-05");
            logger
                .write(&record("展開", &target, "成功"))
                .expect("書ける");
        }
        let text = fs::read_to_string(logger.path()).expect("読める");
        assert!(!text.is_empty());
        assert!(text.contains("定義9"), "最後の記録がない: {text}");
    }

    #[test]
    fn logger_creates_log_directory() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::new(dir.path().join("logs").join("app.log"));
        logger.write(&record("接続", "-", "成功")).expect("書ける");
        assert!(logger.path().exists());
    }

    #[test]
    fn logger_levels_are_labelled() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let logger = Logger::new(dir.path().join("app.log"));
        for (level, label) in [
            (Level::Info, "INFO"),
            (Level::Warn, "WARN"),
            (Level::Error, "ERROR"),
        ] {
            logger
                .write(&LogRecord {
                    level,
                    ..record("接続", "-", "結果")
                })
                .expect("書ける");
            let text = fs::read_to_string(logger.path()).expect("読める");
            assert!(text.contains(label), "{label} がない: {text}");
        }
    }
}
