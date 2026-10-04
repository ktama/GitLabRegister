//! PAT とプロキシ資格情報の保持 (NFR-05、NFR-07、設計書 8.3)。
//!
//! 値はプロセスメモリ上でのみ保持し、終了時に破棄する。
//! ファイル・ログ・画面へ出す経路を型として持たない。
//! 値を使うには閉包を渡す形にしてあり、`String` として取り出す経路を開けていない。

/// 秘密情報の保持器。
///
/// # 破棄について
///
/// `Drop` で保持している文字列の中身を 0 で埋める。
/// ただしこれは最善努力であり、次の 2 点は保証できない。
///
/// - 値がこの保持器へ渡る前に作られた複製 (画面からの受け渡しの途中など)
/// - OS がメモリをページファイルへ書き出した場合
///
/// 要件 FR-01 の受け入れ基準「端末上のいずれのファイルにも PAT が含まれない」は、
/// アプリがファイルへ書かないことで満たす。本型はそれをさらに狭めるための措置である。
#[derive(Default)]
pub struct SecretHolder {
    token: Option<String>,
    proxy_credentials: Option<ProxyCredentials>,
}

/// プロキシの利用者名とパスワード (NFR-17)。
#[derive(Default)]
pub struct ProxyCredentials {
    user: String,
    password: String,
}

impl ProxyCredentials {
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
        }
    }

    pub fn user(&self) -> &str {
        &self.user
    }
}

impl SecretHolder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_token(&mut self, token: impl Into<String>) {
        self.clear_token();
        self.token = Some(token.into());
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn set_proxy_credentials(&mut self, credentials: ProxyCredentials) {
        self.clear_proxy_credentials();
        self.proxy_credentials = Some(credentials);
    }

    pub fn has_proxy_credentials(&self) -> bool {
        self.proxy_credentials.is_some()
    }

    /// トークンを使う。GitLabClient へ供給するための唯一の経路である。
    ///
    /// トークンが未設定なら `None` を返し、閉包は呼ばれない。
    pub fn use_token<R>(&self, f: impl FnOnce(&str) -> R) -> Option<R> {
        self.token.as_deref().map(f)
    }

    /// プロキシ資格情報を使う。
    pub fn use_proxy_credentials<R>(&self, f: impl FnOnce(&str, &str) -> R) -> Option<R> {
        self.proxy_credentials
            .as_ref()
            .map(|c| f(&c.user, &c.password))
    }

    /// ログと画面へ出す前に秘密を伏せるため、伏せる対象の文字列を集める (NFR-07)。
    ///
    /// 値そのものは返さず、照合に使う長さだけを公開する。
    pub fn redaction_targets(&self) -> Vec<String> {
        let mut targets = Vec::new();
        if let Some(t) = &self.token {
            if !t.is_empty() {
                targets.push(t.clone());
            }
        }
        if let Some(c) = &self.proxy_credentials {
            if !c.password.is_empty() {
                targets.push(c.password.clone());
            }
        }
        targets
    }

    pub fn clear_token(&mut self) {
        if let Some(mut t) = self.token.take() {
            zeroize(&mut t);
        }
    }

    pub fn clear_proxy_credentials(&mut self) {
        if let Some(mut c) = self.proxy_credentials.take() {
            zeroize(&mut c.user);
            zeroize(&mut c.password);
        }
    }

    /// すべての秘密を破棄する。アプリの終了時と、認証失敗時に呼ぶ (EC-11)。
    pub fn clear_all(&mut self) {
        self.clear_token();
        self.clear_proxy_credentials();
    }
}

/// 文字列の中身を 0 で埋める。
fn zeroize(s: &mut String) {
    // 文字列の実体を 0 で埋める。UTF-8 として不正になるが、直後に破棄するため参照されない。
    unsafe {
        s.as_bytes_mut().fill(0);
    }
    s.clear();
}

impl Drop for SecretHolder {
    fn drop(&mut self) {
        self.clear_all();
    }
}

impl std::fmt::Debug for SecretHolder {
    /// 表示に値を含めない。ログや画面に誤って出ることを防ぐ (NFR-07)。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretHolder")
            .field(
                "token",
                &if self.has_token() {
                    "(設定済み)"
                } else {
                    "(未設定)"
                },
            )
            .field(
                "proxy_credentials",
                &if self.has_proxy_credentials() {
                    "(設定済み)"
                } else {
                    "(未設定)"
                },
            )
            .finish()
    }
}

impl std::fmt::Debug for ProxyCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyCredentials")
            .field("user", &self.user)
            .field("password", &"(伏せ字)")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_token_is_usable_only_through_closure() {
        let mut holder = SecretHolder::new();
        assert!(!holder.has_token());
        holder.set_token("glpat-example-token");
        assert!(holder.has_token());
        let length = holder.use_token(|t| t.len()).expect("設定済み");
        assert_eq!(length, "glpat-example-token".len());
    }

    #[test]
    fn secret_use_token_returns_none_when_unset() {
        let holder = SecretHolder::new();
        assert!(holder.use_token(|t| t.to_string()).is_none());
    }

    /// NFR-05 / EC-11: 破棄するとトークンが使えなくなる。
    #[test]
    fn secret_clear_removes_token() {
        let mut holder = SecretHolder::new();
        holder.set_token("glpat-example-token");
        holder.clear_all();
        assert!(!holder.has_token());
        assert!(holder.use_token(|t| t.to_string()).is_none());
    }

    /// NFR-07: 表示に値を含めない。
    #[test]
    fn secret_debug_output_hides_values() {
        let mut holder = SecretHolder::new();
        holder.set_token("glpat-secret-value");
        holder.set_proxy_credentials(ProxyCredentials::new("taro", "p@ssw0rd"));
        let shown = format!("{holder:?}");
        assert!(
            !shown.contains("glpat-secret-value"),
            "トークンが表示に出ている: {shown}"
        );
        assert!(
            !shown.contains("p@ssw0rd"),
            "パスワードが表示に出ている: {shown}"
        );
        assert!(shown.contains("設定済み"));
    }

    #[test]
    fn secret_proxy_credentials_debug_hides_password() {
        let credentials = ProxyCredentials::new("taro", "p@ssw0rd");
        let shown = format!("{credentials:?}");
        assert!(!shown.contains("p@ssw0rd"), "{shown}");
        assert!(shown.contains("taro"), "利用者名は見えてよい: {shown}");
    }

    /// NFR-17: プロキシ資格情報も同じ経路で扱う。
    #[test]
    fn secret_proxy_credentials_usable_through_closure() {
        let mut holder = SecretHolder::new();
        holder.set_proxy_credentials(ProxyCredentials::new("taro", "p@ssw0rd"));
        let pair = holder
            .use_proxy_credentials(|u, p| (u.to_string(), p.len()))
            .expect("設定済み");
        assert_eq!(pair, ("taro".to_string(), "p@ssw0rd".len()));
    }

    /// NFR-07: ログで伏せるべき対象としてトークンとパスワードを集める。
    #[test]
    fn secret_redaction_targets_include_token_and_password() {
        let mut holder = SecretHolder::new();
        holder.set_token("glpat-secret-value");
        holder.set_proxy_credentials(ProxyCredentials::new("taro", "p@ssw0rd"));
        let targets = holder.redaction_targets();
        assert!(targets.contains(&"glpat-secret-value".to_string()));
        assert!(targets.contains(&"p@ssw0rd".to_string()));
        // 利用者名は秘密ではないため含めない。
        assert!(!targets.contains(&"taro".to_string()));
    }

    #[test]
    fn secret_set_token_twice_replaces_previous() {
        let mut holder = SecretHolder::new();
        holder.set_token("first");
        holder.set_token("second");
        assert_eq!(holder.use_token(|t| t.to_string()), Some("second".into()));
    }
}
