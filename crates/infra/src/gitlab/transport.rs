//! 差し替え可能な通信口 (ADR-08、NFR-06、NFR-17)。
//!
//! GitLabClient の判断 (分類・再試行・待機) を通信の実体から切り離すために、
//! 通信口をトレイトにする。試験では応答を模した実装を差し込み、
//! ネットワークも試験サーバーも使わずに規則を検証する。

use std::collections::BTreeMap;
use std::time::Duration;

/// 要求の方式。本アプリが使うのはこの 3 つに限る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
        }
    }
}

/// 1 回の要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// 接続先 URL に続く部分。先頭の `/` を含む。
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<serde_json::Value>,
}

impl HttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            path: path.into(),
            query: Vec::new(),
            body: None,
        }
    }

    pub fn post(path: impl Into<String>, body: serde_json::Value) -> Self {
        Self {
            method: Method::Post,
            path: path.into(),
            query: Vec::new(),
            body: Some(body),
        }
    }

    pub fn with_query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }
}

/// 1 回の応答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    /// 見出し。小文字の名前で引ける。`Retry-After` の取得に使う (EC-18)。
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.insert(name.to_ascii_lowercase(), value.into());
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|v| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// 通信そのものが成立しなかった理由。状態コードを伴わない失敗を表す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// 応答が時間内に返らなかった (EC-16)。
    Timeout,
    /// 接続できなかった (EC-10、EC-16)。
    Connect(String),
    /// プロキシが認証を要求した、または拒否した (EC-34)。
    ProxyAuthentication(String),
    /// トークンが設定されていない。接続前に送ろうとした場合に起こる。
    MissingToken,
    /// 要求の組み立てや応答の読み取りに失敗した。
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Timeout => write!(f, "応答が時間内に返りませんでした"),
            TransportError::Connect(d) => write!(f, "接続できませんでした: {d}"),
            TransportError::ProxyAuthentication(d) => {
                write!(f, "プロキシの認証に失敗しました: {d}")
            }
            TransportError::MissingToken => {
                write!(f, "パーソナルアクセストークンが設定されていません")
            }
            TransportError::Other(d) => write!(f, "通信に失敗しました: {d}"),
        }
    }
}

/// 通信口。
pub trait HttpTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl std::future::Future<Output = Result<HttpResponse, TransportError>> + Send;
}

/// 認証に使う値の供給元 (NFR-05、設計書 8.3)。
///
/// 通信口はトークンを保持しない。送るたびにここから取り出す。
/// SecretHolder が唯一の持ち主であり続けるようにするための仕組みである。
///
/// 戻り値は送信のために作る写しである。HTTP の見出しに値を載せる以上この写しは避けられない。
/// 要求を送り終えた時点で捨てられ、ファイルにもログにも渡らない。
pub trait TokenSource: Send + Sync {
    /// 認証の見出しに載せる値。未設定なら `None`。
    fn token_value(&self) -> Option<String>;
}

/// `Arc<RwLock<SecretHolder>>` をそのまま供給元として渡せるようにする。
///
/// 実装を `RwLock` に対して行うのは、`Arc<RwLock<..>>` から `Arc<dyn TokenSource>` へ
/// 変換できるようにするためである。
impl TokenSource for std::sync::RwLock<crate::secret::SecretHolder> {
    fn token_value(&self) -> Option<String> {
        let holder = self.read().ok()?;
        holder.use_token(|t| t.to_string())
    }
}

/// 1 件あたりの通信を打ち切る時間 (EC-16)。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// プロキシの使い方の指定 (NFR-17)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyChoice {
    /// OS に設定されたプロキシ構成に従う。既定。
    FollowSystem,
    /// 指定したプロキシを使う。資格情報は保存せず、必要なときに渡す。
    Manual {
        url: String,
        credentials: Option<(String, String)>,
    },
    /// プロキシを経由しない。
    Direct,
}

/// reqwest を使う実装。
///
/// # TLS と証明書
///
/// reqwest 0.13 の既定の TLS は OS の検証器を使う。
/// 社内の証明機関を OS の証明書ストアに導入している環境でそのまま接続できる (NFR-06)。
/// TLS の機能指定を変更して束ねた CA を使う構成にすると、この性質が失われる。
///
/// # プロキシ
///
/// reqwest は既定で Windows のシステムプロキシ設定に従う (NFR-17)。
/// `ProxyChoice::FollowSystem` ではこの既定に任せ、明示的な設定を加えない。
///
/// # 証明書の検証
///
/// 検証を無効化する経路を設けない (NFR-06)。
///
/// # 送信先
///
/// 要求を送る先は、作成時に渡された接続先とプロキシに限る (NFR-08)。
/// 利用統計や障害情報を送る経路を持たない。
pub struct ReqwestTransport {
    base_url: String,
    client: reqwest::Client,
    tokens: std::sync::Arc<dyn TokenSource>,
}

impl std::fmt::Debug for ReqwestTransport {
    /// トークンの供給元を表示に含めない (NFR-07)。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReqwestTransport")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// 通信口を作れなかった理由。
#[derive(Debug)]
pub enum TransportSetupError {
    /// 接続先 URL が HTTPS でない (NFR-06)。
    NotHttps { url: String },
    /// 接続先 URL を解釈できない。
    InvalidUrl { url: String, detail: String },
    /// プロキシの指定を解釈できない。
    InvalidProxy { url: String, detail: String },
    /// 通信口の作成に失敗した。
    Build { detail: String },
}

impl std::fmt::Display for TransportSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportSetupError::NotHttps { url } => write!(
                f,
                "接続先は HTTPS で指定してください (指定された URL: {url})"
            ),
            TransportSetupError::InvalidUrl { url, detail } => {
                write!(f, "接続先 URL を解釈できません ({url}): {detail}")
            }
            TransportSetupError::InvalidProxy { url, detail } => {
                write!(f, "プロキシの指定を解釈できません ({url}): {detail}")
            }
            TransportSetupError::Build { detail } => {
                write!(f, "通信の準備に失敗しました: {detail}")
            }
        }
    }
}

impl std::error::Error for TransportSetupError {}

impl ReqwestTransport {
    /// 接続先と プロキシの指定から通信口を作る。
    ///
    /// 接続先が HTTPS でなければ作らずに失敗する (NFR-06)。
    pub fn new(
        base_url: &str,
        proxy: &ProxyChoice,
        tokens: std::sync::Arc<dyn TokenSource>,
    ) -> Result<Self, TransportSetupError> {
        let trimmed = base_url.trim_end_matches('/');
        let parsed = reqwest::Url::parse(trimmed).map_err(|e| TransportSetupError::InvalidUrl {
            url: trimmed.to_string(),
            detail: e.to_string(),
        })?;
        if parsed.scheme() != "https" {
            return Err(TransportSetupError::NotHttps {
                url: trimmed.to_string(),
            });
        }

        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        builder = match proxy {
            // 既定の挙動 (OS のプロキシ構成に従う) に任せる。
            ProxyChoice::FollowSystem => builder,
            ProxyChoice::Direct => builder.no_proxy(),
            ProxyChoice::Manual { url, credentials } => {
                let mut p =
                    reqwest::Proxy::all(url).map_err(|e| TransportSetupError::InvalidProxy {
                        url: url.clone(),
                        detail: e.to_string(),
                    })?;
                if let Some((user, password)) = credentials {
                    p = p.basic_auth(user, password);
                }
                builder.proxy(p)
            }
        };
        let client = builder.build().map_err(|e| TransportSetupError::Build {
            detail: e.to_string(),
        })?;
        Ok(Self {
            base_url: trimmed.to_string(),
            client,
            tokens,
        })
    }

    /// 要求に付ける認証の見出しを組み立てる。
    ///
    /// トークンは引数で受け取り、構造体に保持しない。
    /// 保持すると SecretHolder 以外に写しが生まれ、NFR-05 の方針から外れる。
    fn build(&self, request: &HttpRequest, token: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, request.path);
        let mut builder = match request.method {
            Method::Get => self.client.get(&url),
            Method::Post => self.client.post(&url),
            Method::Put => self.client.put(&url),
        };
        builder = builder.header("PRIVATE-TOKEN", token);
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        builder
    }

    /// トークンを添えて 1 回送る。
    pub async fn send_with_token(
        &self,
        request: HttpRequest,
        token: &str,
    ) -> Result<HttpResponse, TransportError> {
        let response = self
            .build(&request, token)
            .send()
            .await
            .map_err(classify_reqwest_error)?;
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            if let Ok(text) = value.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), text.to_string());
            }
        }
        let body = response
            .text()
            .await
            .map_err(|e| TransportError::Other(e.to_string()))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// 要求がどの方式で送られるかを示す。ログに使う。
    pub fn describe(request: &HttpRequest) -> String {
        format!("{} {}", request.method.as_str(), request.path)
    }
}

impl HttpTransport for ReqwestTransport {
    /// 送るたびにトークンを取り出す。通信口は値を保持しない (NFR-05)。
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let token = self
            .tokens
            .token_value()
            .ok_or(TransportError::MissingToken)?;
        self.send_with_token(request, &token).await
    }
}

fn classify_reqwest_error(error: reqwest::Error) -> TransportError {
    if error.is_timeout() {
        return TransportError::Timeout;
    }
    if error.is_connect() {
        return TransportError::Connect(error.to_string());
    }
    TransportError::Other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 固定の値を返す供給元。試験でのみ使う。
    struct FixedToken(Option<String>);

    impl TokenSource for FixedToken {
        fn token_value(&self) -> Option<String> {
            self.0.clone()
        }
    }

    fn tokens() -> Arc<dyn TokenSource> {
        Arc::new(FixedToken(Some("glpat-test-token".into())))
    }

    /// NFR-06: 接続先が HTTPS でなければ通信口を作らない。
    #[test]
    fn transport_rejects_non_https_url() {
        let err = ReqwestTransport::new(
            "http://gitlab.example.com",
            &ProxyChoice::FollowSystem,
            tokens(),
        )
        .expect_err("拒否される");
        assert!(matches!(err, TransportSetupError::NotHttps { .. }), "{err}");
    }

    #[test]
    fn transport_accepts_https_url_and_trims_trailing_slash() {
        let transport = ReqwestTransport::new(
            "https://gitlab.example.com/",
            &ProxyChoice::FollowSystem,
            tokens(),
        )
        .expect("作れる");
        assert_eq!(transport.base_url, "https://gitlab.example.com");
    }

    #[test]
    fn transport_rejects_unparsable_url() {
        let err = ReqwestTransport::new("これはURLではない", &ProxyChoice::FollowSystem, tokens())
            .expect_err("拒否される");
        assert!(
            matches!(err, TransportSetupError::InvalidUrl { .. }),
            "{err}"
        );
    }

    /// NFR-17: プロキシの 3 つの指定すべてで通信口を作れる。
    #[test]
    fn transport_builds_for_each_proxy_choice() {
        for proxy in [
            ProxyChoice::FollowSystem,
            ProxyChoice::Direct,
            ProxyChoice::Manual {
                url: "http://proxy.example.com:8080".into(),
                credentials: Some(("taro".into(), "p@ssw0rd".into())),
            },
        ] {
            ReqwestTransport::new("https://gitlab.example.com", &proxy, tokens())
                .unwrap_or_else(|e| panic!("{proxy:?} で作れない: {e}"));
        }
    }

    /// NFR-17: プロキシの指定を解釈できなければ失敗する。
    #[test]
    fn transport_rejects_invalid_proxy() {
        let err = ReqwestTransport::new(
            "https://gitlab.example.com",
            &ProxyChoice::Manual {
                url: "::::".into(),
                credentials: None,
            },
            tokens(),
        )
        .expect_err("拒否される");
        assert!(
            matches!(err, TransportSetupError::InvalidProxy { .. }),
            "{err}"
        );
    }

    #[test]
    fn transport_response_header_lookup_is_case_insensitive() {
        let response = HttpResponse::new(429, "").with_header("Retry-After", "30");
        assert_eq!(response.header("retry-after"), Some("30"));
        assert_eq!(response.header("RETRY-AFTER"), Some("30"));
    }

    #[test]
    fn transport_request_builders_set_method_and_body() {
        let get = HttpRequest::get("/api/v4/user").with_query("per_page", "100");
        assert_eq!(get.method, Method::Get);
        assert_eq!(get.query, vec![("per_page".to_string(), "100".to_string())]);
        assert!(get.body.is_none());

        let post = HttpRequest::post(
            "/api/v4/projects/1/issues",
            serde_json::json!({"title": "a"}),
        );
        assert_eq!(post.method, Method::Post);
        assert!(post.body.is_some());
    }

    #[test]
    fn transport_describe_shows_method_and_path() {
        let request = HttpRequest::get("/api/v4/version");
        assert_eq!(ReqwestTransport::describe(&request), "GET /api/v4/version");
    }

    /// NFR-05: 通信口はトークンを保持せず、送るたびに供給元から取り出す。
    #[test]
    fn transport_pulls_token_from_source_each_time() {
        use crate::secret::SecretHolder;
        let holder = Arc::new(std::sync::RwLock::new(SecretHolder::new()));
        let source: Arc<dyn TokenSource> = holder.clone();
        assert!(source.token_value().is_none(), "未設定では供給しない");

        holder
            .write()
            .expect("施錠できる")
            .set_token("glpat-example");
        assert_eq!(source.token_value(), Some("glpat-example".to_string()));

        // EC-11: 破棄すると供給されなくなる。
        holder.write().expect("施錠できる").clear_all();
        assert!(source.token_value().is_none());
    }

    /// NFR-07: 通信口の表示にトークンの供給元を含めない。
    #[test]
    fn transport_debug_output_hides_token_source() {
        let transport = ReqwestTransport::new(
            "https://gitlab.example.com",
            &ProxyChoice::FollowSystem,
            tokens(),
        )
        .expect("作れる");
        let shown = format!("{transport:?}");
        assert!(!shown.contains("glpat-test-token"), "{shown}");
        assert!(shown.contains("gitlab.example.com"), "{shown}");
    }

    #[test]
    fn transport_timeout_is_30_seconds() {
        // EC-16: 1 件あたり 30 秒で通信を打ち切る。
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }
}
