//! 接続の状態と接続確認 (FR-01、EC-11、EC-19、EC-35、NFR-17、設計書 7.5)。

use gitlabregister_infra::gitlab::client::{Connection, GitLabClient, Tier, Version};
use gitlabregister_infra::gitlab::transport::{
    HttpTransport, ProxyChoice, ReqwestTransport, TokenSource, TransportSetupError,
};
use gitlabregister_infra::gitlab::{ErrorKind, GitLabError};
use gitlabregister_infra::secret::{ProxyCredentials, SecretHolder};
use gitlabregister_infra::store::{AppSettings, ProxyMode};
use std::sync::{Arc, RwLock};

/// 接続の状態 (設計書 7.5)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// 未接続。定義・休日・プロジェクト・履歴の閲覧と編集、展開とプレビューができる。
    Disconnected,
    /// 接続済み。上記に加えて一括登録・単発登録・候補の取得ができる。
    Connected(Connection),
    /// 通信が届かない。未接続と同じ範囲に縮退する (EC-19)。
    Unreachable { username: String },
    /// バージョンが下限未満。登録を実行しない (EC-35)。
    VersionUnsupported { found: Version },
}

impl ConnectionState {
    /// 一括登録と単発登録を実行できる状態か (EC-19、EC-35)。
    pub fn can_register(&self) -> bool {
        matches!(self, ConnectionState::Connected(_))
    }

    /// 候補値を GitLab-EE から取得できる状態か (FR-16、EC-19)。
    pub fn can_fetch_candidates(&self) -> bool {
        matches!(self, ConnectionState::Connected(_))
    }

    /// EE 固有項目の入力を許すか (EC-15)。
    ///
    /// 接続していない間は要件書 第 3 節の前提 (Premium 以上) に従って許す。
    pub fn allows_ee_fields(&self) -> bool {
        match self {
            ConnectionState::Connected(c) => c.tier.allows_ee_fields(),
            _ => true,
        }
    }

    /// 画面に出す説明。
    pub fn describe(&self) -> String {
        match self {
            ConnectionState::Disconnected => "未接続".into(),
            ConnectionState::Connected(c) => format!(
                "接続済み (利用者 {} / GitLab-EE {} / ティア {})",
                c.username,
                c.version,
                describe_tier(c.tier)
            ),
            ConnectionState::Unreachable { username } => {
                format!("到達不能 (利用者 {username})。登録はできません")
            }
            ConnectionState::VersionUnsupported { found } => {
                format!("対応していないバージョンです (接続先 {found} / 必要なのは 19.2 以降)")
            }
        }
    }
}

fn describe_tier(tier: Tier) -> &'static str {
    match tier {
        Tier::PremiumOrAbove => "Premium 以上",
        Tier::Free => "Free",
        Tier::Unknown => "判定できません",
    }
}

/// 接続を試みた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOutcome {
    pub state: ConnectionState,
    /// 画面に出す注意事項。
    pub warnings: Vec<String>,
}

/// 接続の準備ができなかった理由。
#[derive(Debug)]
pub enum ConnectError {
    /// 接続先 URL が未入力。
    UrlMissing,
    /// 通信口を作れなかった (URL が HTTPS でない、プロキシの指定が不正など)。
    Setup(TransportSetupError),
    /// GitLab-EE との通信で失敗した。
    Request(GitLabError),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UrlMissing => write!(f, "接続先 URL を入力してください"),
            ConnectError::Setup(e) => write!(f, "{e}"),
            ConnectError::Request(e) => write!(f, "{}: {}", e.message, e.kind.next_action()),
        }
    }
}

impl std::error::Error for ConnectError {}

/// 接続確認と状態の保持。
///
/// 秘密情報は `SecretHolder` が持ち、本サービスは写しを持たない (NFR-05)。
pub struct ConnectionService {
    secrets: Arc<RwLock<SecretHolder>>,
    state: RwLock<ConnectionState>,
}

impl ConnectionService {
    pub fn new(secrets: Arc<RwLock<SecretHolder>>) -> Self {
        Self {
            secrets,
            state: RwLock::new(ConnectionState::Disconnected),
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state.read().expect("施錠できる").clone()
    }

    fn set_state(&self, state: ConnectionState) {
        *self.state.write().expect("施錠できる") = state;
    }

    /// 入力された接続先と秘密情報を保持し、通信口を作る (FR-01、NFR-17)。
    ///
    /// トークンとプロキシの資格情報は `SecretHolder` へ渡し、本サービスは保持しない。
    pub fn prepare_transport(
        &self,
        settings: &AppSettings,
        token: &str,
        proxy_credentials: Option<(String, String)>,
    ) -> Result<ReqwestTransport, ConnectError> {
        let url = settings
            .gitlab_url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .ok_or(ConnectError::UrlMissing)?;

        {
            let mut holder = self.secrets.write().expect("施錠できる");
            holder.set_token(token);
            match &proxy_credentials {
                Some((user, password)) => {
                    holder.set_proxy_credentials(ProxyCredentials::new(user, password))
                }
                None => holder.clear_proxy_credentials(),
            }
        }

        let proxy = to_proxy_choice(settings, proxy_credentials);
        let tokens: Arc<dyn TokenSource> = self.secrets.clone();
        ReqwestTransport::new(url, &proxy, tokens).map_err(ConnectError::Setup)
    }

    /// 接続確認を行い、状態を更新する (FR-01、EC-11、EC-35)。
    pub async fn connect<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
    ) -> Result<ConnectOutcome, ConnectError> {
        match client.connect().await {
            Ok(connection) => {
                let mut warnings = Vec::new();
                if !connection.version.is_supported() {
                    // EC-35: 下限未満では登録を実行しない。
                    let state = ConnectionState::VersionUnsupported {
                        found: connection.version,
                    };
                    warnings.push(state.describe());
                    self.set_state(state.clone());
                    return Ok(ConnectOutcome { state, warnings });
                }
                if connection.tier == Tier::Unknown {
                    // EC-15 / 設計書 R-5: 判定できない場合は前提に従って進む。
                    warnings.push(
                        "ライセンスティアを判定できませんでした。EE 固有項目は使える前提で進めます。登録が失敗した場合は該当項目を外して再実行してください".into(),
                    );
                }
                if connection.tier == Tier::Free {
                    warnings.push(
                        "ライセンスティアが Free です。エピック・イテレーション・ウェイト・複数担当者は登録に使えません".into(),
                    );
                }
                let state = ConnectionState::Connected(connection);
                self.set_state(state.clone());
                Ok(ConnectOutcome { state, warnings })
            }
            Err(error) => {
                // EC-11: 認証の失敗では秘密情報を破棄し、再入力を求める。
                if error.kind == ErrorKind::Authentication {
                    self.secrets.write().expect("施錠できる").clear_all();
                }
                self.set_state(ConnectionState::Disconnected);
                Err(ConnectError::Request(error))
            }
        }
    }

    /// 通信が届かなくなったことを記録する (EC-10、EC-19)。
    ///
    /// 定義の閲覧と展開は続けられるため、未接続とは別の状態として残す。
    pub fn mark_unreachable(&self) {
        let username = match self.state() {
            ConnectionState::Connected(c) => c.username,
            ConnectionState::Unreachable { username } => username,
            _ => String::new(),
        };
        self.set_state(ConnectionState::Unreachable { username });
    }

    /// 認証が失効したことを記録する (EC-11)。
    pub fn mark_authentication_lost(&self) {
        self.secrets.write().expect("施錠できる").clear_all();
        self.set_state(ConnectionState::Disconnected);
    }

    /// ログで伏せる対象を集める (NFR-07)。
    pub fn redaction_targets(&self) -> Vec<String> {
        self.secrets.read().expect("施錠できる").redaction_targets()
    }
}

fn to_proxy_choice(settings: &AppSettings, credentials: Option<(String, String)>) -> ProxyChoice {
    match settings.proxy.mode {
        ProxyMode::FollowSystem => ProxyChoice::FollowSystem,
        ProxyMode::Direct => ProxyChoice::Direct,
        ProxyMode::Manual => match (&settings.proxy.host, settings.proxy.port) {
            (Some(host), Some(port)) => ProxyChoice::Manual {
                url: format!("http://{host}:{port}"),
                credentials,
            },
            // ホストと港が揃っていなければ OS の構成に従う。
            _ => ProxyChoice::FollowSystem,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{client_with, ok, status};
    use gitlabregister_infra::store::ProxySettings;

    fn service() -> ConnectionService {
        ConnectionService::new(Arc::new(RwLock::new(SecretHolder::new())))
    }

    fn settings(url: &str) -> AppSettings {
        AppSettings {
            gitlab_url: Some(url.into()),
            proxy: ProxySettings::default(),
        }
    }

    /// FR-01: 接続に成功すると利用者名とバージョンが状態に入る。
    #[tokio::test]
    async fn connection_connect_sets_connected_state() {
        let s = service();
        assert_eq!(s.state(), ConnectionState::Disconnected);
        assert!(!s.state().can_register());

        let client = client_with(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            ok(r#"{"plan":"premium"}"#),
        ]);
        let outcome = s.connect(&client).await.expect("接続できる");
        assert!(outcome.state.can_register());
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        assert!(
            s.state().describe().contains("taro"),
            "{}",
            s.state().describe()
        );
        assert!(s.state().describe().contains("19.2.1"));
    }

    /// EC-35: 下限未満のバージョンでは登録できない状態になる。
    #[tokio::test]
    async fn connection_unsupported_version_blocks_registration() {
        let s = service();
        let client = client_with(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.1.9-ee"}"#),
            status(403, "{}"),
        ]);
        let outcome = s.connect(&client).await.expect("接続はできる");
        assert!(!outcome.state.can_register(), "登録できてはいけない");
        assert!(matches!(
            outcome.state,
            ConnectionState::VersionUnsupported { .. }
        ));
        // 下限と実際のバージョンの両方を示す (EC-35)。
        let message = outcome.warnings.join(" ");
        assert!(message.contains("19.1.9"), "{message}");
        assert!(message.contains("19.2"), "{message}");
    }

    /// EC-11: 認証の失敗では状態が未接続に戻り、秘密情報が破棄される。
    #[tokio::test]
    async fn connection_authentication_failure_clears_secrets() {
        let secrets = Arc::new(RwLock::new(SecretHolder::new()));
        secrets.write().expect("施錠できる").set_token("glpat-x");
        let s = ConnectionService::new(secrets.clone());

        let client = client_with(vec![status(401, r#"{"message":"401 Unauthorized"}"#)]);
        let err = s.connect(&client).await.expect_err("失敗する");
        assert!(matches!(err, ConnectError::Request(_)), "{err}");
        assert_eq!(s.state(), ConnectionState::Disconnected);
        assert!(
            !secrets.read().expect("施錠できる").has_token(),
            "トークンが残っている"
        );
    }

    /// EC-15: ティアを判定できない場合は注意を出すが、EE 固有項目は使える。
    #[tokio::test]
    async fn connection_unknown_tier_warns_but_allows_ee_fields() {
        let s = service();
        let client = client_with(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            status(403, "{}"),
        ]);
        let outcome = s.connect(&client).await.expect("接続できる");
        assert!(outcome.state.allows_ee_fields());
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("判定できません")),
            "{:?}",
            outcome.warnings
        );
    }

    /// EC-15: Free では EE 固有項目を使えないことを知らせる。
    #[tokio::test]
    async fn connection_free_tier_disallows_ee_fields() {
        let s = service();
        let client = client_with(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            ok(r#"{"plan":"free"}"#),
        ]);
        let outcome = s.connect(&client).await.expect("接続できる");
        assert!(!outcome.state.allows_ee_fields());
        assert!(
            outcome.warnings.iter().any(|w| w.contains("Free")),
            "{:?}",
            outcome.warnings
        );
    }

    /// EC-19: 到達不能では登録できないが、未接続とは区別して利用者名を残す。
    #[tokio::test]
    async fn connection_mark_unreachable_keeps_username() {
        let s = service();
        let client = client_with(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            ok(r#"{"plan":"premium"}"#),
        ]);
        s.connect(&client).await.expect("接続できる");
        s.mark_unreachable();
        assert!(!s.state().can_register());
        assert!(!s.state().can_fetch_candidates());
        assert!(
            s.state().describe().contains("taro"),
            "{}",
            s.state().describe()
        );
    }

    /// EC-11: 実行中に失効した場合も秘密情報を破棄して未接続へ戻す。
    #[tokio::test]
    async fn connection_mark_authentication_lost_clears_secrets() {
        let secrets = Arc::new(RwLock::new(SecretHolder::new()));
        secrets.write().expect("施錠できる").set_token("glpat-x");
        let s = ConnectionService::new(secrets.clone());
        s.mark_authentication_lost();
        assert_eq!(s.state(), ConnectionState::Disconnected);
        assert!(!secrets.read().expect("施錠できる").has_token());
    }

    /// FR-01: 接続先 URL が未入力なら通信口を作らない。
    #[test]
    fn connection_prepare_transport_requires_url() {
        let s = service();
        let err = s
            .prepare_transport(&AppSettings::default(), "glpat-x", None)
            .expect_err("拒否される");
        assert!(matches!(err, ConnectError::UrlMissing), "{err}");
    }

    /// NFR-06: HTTPS でない接続先は拒否する。
    #[test]
    fn connection_prepare_transport_rejects_non_https() {
        let s = service();
        let err = s
            .prepare_transport(&settings("http://gitlab.example.com"), "glpat-x", None)
            .expect_err("拒否される");
        assert!(matches!(err, ConnectError::Setup(_)), "{err}");
    }

    /// FR-01 / NFR-05: 入力したトークンは SecretHolder へ渡り、サービスは保持しない。
    #[test]
    fn connection_prepare_transport_stores_token_in_holder_only() {
        let secrets = Arc::new(RwLock::new(SecretHolder::new()));
        let s = ConnectionService::new(secrets.clone());
        s.prepare_transport(&settings("https://gitlab.example.com"), "glpat-x", None)
            .expect("作れる");
        assert!(secrets.read().expect("施錠できる").has_token());
        // NFR-07: 伏せる対象として取り出せる。
        assert!(s.redaction_targets().contains(&"glpat-x".to_string()));
    }

    /// NFR-17: プロキシの資格情報も SecretHolder へ渡る。
    #[test]
    fn connection_prepare_transport_stores_proxy_credentials() {
        let secrets = Arc::new(RwLock::new(SecretHolder::new()));
        let s = ConnectionService::new(secrets.clone());
        let mut settings = settings("https://gitlab.example.com");
        settings.proxy.mode = ProxyMode::Manual;
        settings.proxy.host = Some("proxy.example.com".into());
        settings.proxy.port = Some(8080);
        settings.proxy.requires_credentials = true;

        s.prepare_transport(
            &settings,
            "glpat-x",
            Some(("taro".into(), "p@ssw0rd".into())),
        )
        .expect("作れる");
        assert!(secrets.read().expect("施錠できる").has_proxy_credentials());
        assert!(s.redaction_targets().contains(&"p@ssw0rd".to_string()));
    }

    /// NFR-17: 手動指定でホストが欠けていれば OS の構成に従う。
    #[test]
    fn connection_manual_proxy_without_host_falls_back_to_system() {
        let mut settings = settings("https://gitlab.example.com");
        settings.proxy.mode = ProxyMode::Manual;
        settings.proxy.host = None;
        assert_eq!(to_proxy_choice(&settings, None), ProxyChoice::FollowSystem);
    }

    /// NFR-17: 3 つの指定がそれぞれ対応する選択へ変換される。
    #[test]
    fn connection_proxy_mode_maps_to_choice() {
        let mut settings = settings("https://gitlab.example.com");
        settings.proxy.mode = ProxyMode::FollowSystem;
        assert_eq!(to_proxy_choice(&settings, None), ProxyChoice::FollowSystem);
        settings.proxy.mode = ProxyMode::Direct;
        assert_eq!(to_proxy_choice(&settings, None), ProxyChoice::Direct);
        settings.proxy.mode = ProxyMode::Manual;
        settings.proxy.host = Some("proxy.example.com".into());
        settings.proxy.port = Some(3128);
        assert_eq!(
            to_proxy_choice(&settings, None),
            ProxyChoice::Manual {
                url: "http://proxy.example.com:3128".into(),
                credentials: None
            }
        );
    }

    /// 未接続では登録も候補取得もできないが、EE 固有項目の入力は許す。
    #[test]
    fn connection_disconnected_capabilities() {
        let state = ConnectionState::Disconnected;
        assert!(!state.can_register());
        assert!(!state.can_fetch_candidates());
        assert!(state.allows_ee_fields());
        assert_eq!(state.describe(), "未接続");
    }
}
