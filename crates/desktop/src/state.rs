//! アプリが抱える状態 (設計書 4.2)。
//!
//! 秘密情報は SecretHolder が持ち、ここは共有の手掛かりだけを持つ (NFR-05)。

use gitlabregister_app::boundary::Boundary;
use gitlabregister_app::registration::CancellationToken;
use gitlabregister_infra::gitlab::client::{GitLabClient, RetryPolicy};
use gitlabregister_infra::gitlab::transport::ReqwestTransport;
use gitlabregister_infra::paths::AppPaths;
use gitlabregister_infra::secret::SecretHolder;
use gitlabregister_infra::store::ConfigStore;
use gitlabregister_infra::{Level, LogRecord, Logger};
use std::sync::{Arc, Mutex, RwLock};

/// 画面から呼ばれる操作の持ち主。
pub struct AppState {
    pub boundary: Boundary,
    /// 接続後に作られる通信口。未接続では `None`。
    client: Mutex<Option<Arc<GitLabClient<ReqwestTransport>>>>,
    /// 実行中の一括登録を止めるための札 (FR-09)。
    cancel: Mutex<CancellationToken>,
    logger: Mutex<Logger>,
}

impl AppState {
    pub fn new(paths: AppPaths) -> Self {
        let secrets = Arc::new(RwLock::new(SecretHolder::new()));
        let config = ConfigStore::new(paths.clone());
        let logger = Logger::new(paths.logs_dir().join("app.log"));
        Self {
            // 秘密情報の持ち主は SecretHolder だけ。この構造体は手掛かりを持たない (NFR-05)。
            boundary: Boundary::new(config, secrets),
            client: Mutex::new(None),
            cancel: Mutex::new(CancellationToken::new()),
            logger: Mutex::new(logger),
        }
    }

    /// 接続後の通信口を差し替える。
    pub fn set_client(&self, transport: ReqwestTransport) {
        let client = GitLabClient::with_retry(transport, RetryPolicy::default());
        *self.client.lock().expect("施錠できる") = Some(Arc::new(client));
    }

    pub fn clear_client(&self) {
        *self.client.lock().expect("施錠できる") = None;
    }

    /// 通信口を取り出す。未接続なら `None`。
    pub fn client(&self) -> Option<Arc<GitLabClient<ReqwestTransport>>> {
        self.client.lock().expect("施錠できる").clone()
    }

    /// 新しい一括登録のための札を作る。前回の札は捨てる。
    pub fn new_cancellation(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.cancel.lock().expect("施錠できる") = token.clone();
        token
    }

    /// 実行中の一括登録を止める (FR-09)。
    pub fn cancel_running(&self) {
        self.cancel.lock().expect("施錠できる").cancel();
    }

    /// ログで伏せる対象を更新する (NFR-07)。接続時と PAT の再入力時に呼ぶ。
    pub fn refresh_redaction(&self) {
        let targets = self.boundary.connection.redaction_targets();
        self.logger
            .lock()
            .expect("施錠できる")
            .set_redaction_targets(targets);
    }

    /// 操作ログを 1 件記録する (FR-17)。失敗しても処理は続ける。
    pub fn log(&self, level: Level, operation: &str, target: &str, outcome: &str) {
        let record = LogRecord {
            at: crate::clock::now(),
            level,
            operation,
            target,
            outcome,
        };
        let _ = self.logger.lock().expect("施錠できる").write(&record);
    }
}
