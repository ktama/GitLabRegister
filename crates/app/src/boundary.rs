//! コマンド境界 (設計書 6.1)。
//!
//! UI とサービス層の唯一の接点。入力の検証、実行中の多重起動の抑止 (EC-09)、
//! エラーの表示用形式への変換を担う。
//! UI 層はここが返す分類だけを見て表示を切り替え、分類の判定を自分で行わない。

use crate::candidate::{CandidateList, CandidateService};
use crate::connection::{ConnectError, ConnectOutcome, ConnectionService, ConnectionState};
use crate::expansion::{ExpandError, ExpansionService, Preview};
use crate::masterdata::MasterDataService;
use crate::registration::{
    BatchOutcome, CancellationToken, ItemStatus, ProgressSink, RegistrationItem,
    RegistrationService,
};
use crate::schedule::{SaveError, ScheduleService};
use chrono::{NaiveDate, NaiveDateTime};
use gitlabregister_infra::gitlab::client::{CandidateKind, GitLabClient, Tier};
use gitlabregister_infra::gitlab::transport::HttpTransport;
use gitlabregister_infra::gitlab::GitLabError;
use gitlabregister_infra::secret::SecretHolder;
use gitlabregister_infra::store::{AppSettings, ConfigStore};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// UI へ返す失敗の分類。
///
/// UI はこの分類とメッセージだけを見る。状態コードや内部の型を解釈しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 入力を直せば解消する。
    Input,
    /// 認証をやり直す必要がある。
    Authentication,
    /// 権限が足りない。
    Permission,
    /// 対象が見つからない。
    NotFound,
    /// 接続できない。
    Unreachable,
    /// 時間をおけば解消する可能性がある。
    Temporary,
    /// 端末の読み書きで失敗した。
    Storage,
    /// 操作が重複して呼ばれた (EC-09)。
    Busy,
}

/// UI へ返す失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// 画面に出す文面。
    pub message: String,
    /// 利用者が次に取れる操作 (NFR-16)。
    pub next_action: String,
}

impl Failure {
    fn new(kind: FailureKind, message: impl Into<String>, next_action: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            next_action: next_action.into(),
        }
    }

    fn storage(error: impl std::fmt::Display) -> Self {
        Self::new(
            FailureKind::Storage,
            error.to_string(),
            "設定フォルダの状態を確認してください",
        )
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.next_action)
    }
}

impl From<SaveError> for Failure {
    fn from(error: SaveError) -> Self {
        match &error {
            SaveError::Store(e) => Failure::storage(e),
            SaveError::NotFound(_) => Failure::new(
                FailureKind::NotFound,
                error.to_string(),
                "一覧を再読み込みしてください",
            ),
            _ => Failure::new(
                FailureKind::Input,
                error.to_string(),
                "表示された項目を修正してください",
            ),
        }
    }
}

impl From<ExpandError> for Failure {
    fn from(error: ExpandError) -> Self {
        match &error {
            ExpandError::Store(e) => Failure::storage(e),
            ExpandError::RangeReversed { .. } => Failure::new(
                FailureKind::Input,
                error.to_string(),
                "開始日と終了日を入れ替えてください",
            ),
        }
    }
}

impl From<ConnectError> for Failure {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::UrlMissing => Failure::new(
                FailureKind::Input,
                "接続先 URL を入力してください",
                "接続先 URL を入力してください",
            ),
            ConnectError::Setup(e) => Failure::new(
                FailureKind::Input,
                e.to_string(),
                "接続先 URL とプロキシの設定を見直してください",
            ),
            ConnectError::Request(e) => Failure::from(e),
        }
    }
}

impl From<GitLabError> for Failure {
    fn from(error: GitLabError) -> Self {
        use gitlabregister_infra::gitlab::ErrorKind;
        let kind = match error.kind {
            ErrorKind::Authentication => FailureKind::Authentication,
            ErrorKind::Authorization | ErrorKind::ProxyAuthentication => FailureKind::Permission,
            ErrorKind::Validation => FailureKind::Input,
            ErrorKind::NotFound => FailureKind::NotFound,
            ErrorKind::Unreachable => FailureKind::Unreachable,
            ErrorKind::RateLimited { .. } | ErrorKind::Transient => FailureKind::Temporary,
        };
        Failure::new(kind, error.message, error.kind.next_action())
    }
}

impl From<gitlabregister_infra::atomic::StoreError> for Failure {
    fn from(error: gitlabregister_infra::atomic::StoreError) -> Self {
        Failure::storage(error)
    }
}

/// サービスをまとめ、UI から呼ぶ単位を提供する。
pub struct Boundary {
    pub connection: ConnectionService,
    pub schedules: ScheduleService,
    pub expansion: ExpansionService,
    pub registration: RegistrationService,
    pub candidates: CandidateService,
    pub master: MasterDataService,
    config: ConfigStore,
    /// 一括登録と単発登録が実行中かを表す (EC-09)。
    registering: AtomicBool,
}

impl Boundary {
    pub fn new(config: ConfigStore, secrets: Arc<RwLock<SecretHolder>>) -> Self {
        Self {
            connection: ConnectionService::new(secrets),
            schedules: ScheduleService::new(config.clone()),
            expansion: ExpansionService::new(config.clone()),
            registration: RegistrationService::new(config.clone()),
            candidates: CandidateService::new(config.clone()),
            master: MasterDataService::new(config.clone()),
            config,
            registering: AtomicBool::new(false),
        }
    }

    pub fn settings(&self) -> Result<AppSettings, Failure> {
        self.config.load_settings().map_err(Failure::from)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), Failure> {
        self.config.save_settings(settings).map_err(Failure::from)
    }

    /// 接続する (FR-01)。
    pub async fn connect<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
    ) -> Result<ConnectOutcome, Failure> {
        self.connection.connect(client).await.map_err(Failure::from)
    }

    /// 展開する (FR-07)。
    ///
    /// 候補を取得済みのプロジェクトのラベルを突合に使う (EC-32)。
    pub fn expand(&self, start: NaiveDate, end: NaiveDate) -> Result<Preview, Failure> {
        self.expand_with_labels(start, end)
    }

    fn expand_with_labels(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Preview, Failure> {
        // 保持している候補をラベルの突合へ渡す。取得していないプロジェクトは
        // ExpansionService 側で「突合できなかった」として扱われる。
        if let Ok(projects) = self.config.load_schedules() {
            for project in projects
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.project.clone())
                .collect::<std::collections::BTreeSet<_>>()
            {
                if let Ok(list) = self.candidates.cached(&project, CandidateKind::Label) {
                    if list.freshness != crate::candidate::Freshness::NeverFetched {
                        self.expansion.set_label_candidates(&project, list.names());
                    }
                }
            }
        }
        self.expansion.expand(start, end).map_err(Failure::from)
    }

    /// 一括登録する (FR-09、EC-09)。
    ///
    /// 実行中は新たな登録を開始しない。連打と二重起動を抑止する。
    pub async fn register_batch<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        items: &[RegistrationItem],
        now: NaiveDateTime,
        cancel: &CancellationToken,
        progress: Option<ProgressSink>,
    ) -> Result<BatchOutcome, Failure> {
        let _guard = self.begin_registration()?;
        let tier = self.current_tier();
        let outcome = self
            .registration
            .register_batch(client, items, tier, now, cancel, progress)
            .await;
        self.after_batch(&outcome);
        Ok(outcome)
    }

    /// 失敗分を再実行する (FR-20、EC-09)。
    pub async fn retry_failed<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        previous: &BatchOutcome,
        now: NaiveDateTime,
        cancel: &CancellationToken,
        progress: Option<ProgressSink>,
    ) -> Result<BatchOutcome, Failure> {
        let items = previous.failed_items();
        if items.is_empty() {
            return Err(Failure::new(
                FailureKind::Input,
                "再実行する対象がありません",
                "失敗した行がある場合のみ再実行できます",
            ));
        }
        let _guard = self.begin_registration()?;
        let tier = self.current_tier();
        let outcome = self
            .registration
            .retry_failed(client, previous, &items, tier, now, cancel, progress)
            .await;
        self.after_batch(&outcome);
        Ok(outcome)
    }

    /// 単発登録する (FR-12、EC-09)。
    pub async fn register_single<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        item: &RegistrationItem,
    ) -> Result<ItemStatus, Failure> {
        let _guard = self.begin_registration()?;
        let tier = self.current_tier();
        self.registration
            .register_single(client, item, tier)
            .await
            .map_err(Failure::from)
    }

    /// 候補を取得する (FR-16)。
    pub async fn candidates<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        project: &str,
        kind: CandidateKind,
        now: NaiveDateTime,
    ) -> Result<CandidateList, Failure> {
        self.candidates
            .ensure(client, project, kind, now)
            .await
            .map_err(Failure::from)
    }

    /// 履歴を削除する (FR-23)。削除後は展開の索引を捨てる。
    pub fn delete_history(
        &self,
        keys: &[(gitlabregister_core::model::ScheduleId, NaiveDate)],
    ) -> Result<usize, Failure> {
        let removed = self.master.delete_history(keys).map_err(Failure::from)?;
        self.expansion.invalidate_history();
        Ok(removed)
    }

    /// 登録が実行中かどうか (EC-09)。
    pub fn is_registering(&self) -> bool {
        self.registering.load(Ordering::SeqCst)
    }

    fn begin_registration(&self) -> Result<RegistrationGuard<'_>, Failure> {
        if self
            .registering
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Failure::new(
                FailureKind::Busy,
                "登録を実行中です",
                "実行中の登録が終わるまでお待ちください",
            ));
        }
        Ok(RegistrationGuard {
            flag: &self.registering,
        })
    }

    fn current_tier(&self) -> Tier {
        match self.connection.state() {
            ConnectionState::Connected(c) => c.tier,
            // 接続していない状態では登録に進めないが、要件書 第 3 節の前提に合わせる。
            _ => Tier::Unknown,
        }
    }

    /// 一括登録の後始末。索引の更新と接続状態の反映を行う。
    fn after_batch(&self, outcome: &BatchOutcome) {
        for item in outcome.outcomes.iter().filter(|o| o.status.is_success()) {
            self.expansion
                .record_registered(&item.item.schedule_id, item.item.occurrence_date);
        }
        match &outcome.aborted {
            // EC-10: 通信が届かなくなったら状態を縮退させる。
            Some(crate::registration::AbortReason::Unreachable { .. }) => {
                self.connection.mark_unreachable()
            }
            // EC-11: 認証が失効したら秘密情報を破棄して再入力を求める。
            Some(crate::registration::AbortReason::AuthenticationLost { .. }) => {
                self.connection.mark_authentication_lost()
            }
            None => {}
        }
    }
}

/// 実行中の印を外す番人。経路のどこで抜けても印が残らないようにする。
struct RegistrationGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for RegistrationGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{client_with, ok, status};
    use gitlabregister_core::model::{
        ActivePeriod, BusinessDayAdjustment, IssueTemplate, MissingDayPolicy, MonthlyDay,
        RecurrencePattern, Schedule, ScheduleId,
    };
    use gitlabregister_infra::gitlab::transport::TransportError;
    use gitlabregister_infra::paths::AppPaths;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn now() -> NaiveDateTime {
        d(2026, 10, 3).and_hms_opt(14, 0, 0).expect("有効な時刻")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        config: ConfigStore,
        boundary: Boundary,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let root = dir.path().join("GitLabRegister");
        let config = ConfigStore::new(AppPaths::with_root(root));
        Fixture {
            _dir: dir,
            config: config.clone(),
            boundary: Boundary::new(config, Arc::new(RwLock::new(SecretHolder::new()))),
        }
    }

    fn schedule(name: &str, day: u8) -> Schedule {
        Schedule {
            id: ScheduleId(format!("id-{name}")),
            name: name.into(),
            project: "group/alpha".into(),
            enabled: true,
            active_period: ActivePeriod::default(),
            pattern: RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(day),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            adjustment: BusinessDayAdjustment::None,
            issue: IssueTemplate {
                title: name.into(),
                labels: vec!["report".into()],
                ..IssueTemplate::default()
            },
        }
    }

    fn item(name: &str, day: u32) -> RegistrationItem {
        RegistrationItem {
            schedule_id: ScheduleId(format!("id-{name}")),
            schedule_name: name.into(),
            occurrence_date: d(2026, 10, day),
            project: "group/alpha".into(),
            title: name.into(),
            description: String::new(),
            labels: vec![],
            assignee_ids: vec![],
            milestone: None,
            start_date: None,
            due_date: None,
            confidential: false,
            epic_id: None,
            iteration_id: None,
            weight: None,
        }
    }

    fn created(iid: u64) -> Result<
        gitlabregister_infra::gitlab::transport::HttpResponse,
        TransportError,
    > {
        ok(&format!(
            r#"{{"id":{},"iid":{iid},"web_url":"https://gitlab.example.com/-/issues/{iid}"}}"#,
            1000 + iid
        ))
    }

    /// FR-01: 設定を保存して読み戻せる。
    #[test]
    fn boundary_settings_round_trip() {
        let f = fixture();
        let settings = AppSettings {
            gitlab_url: Some("https://gitlab.example.com".into()),
            ..AppSettings::default()
        };
        f.boundary.save_settings(&settings).expect("保存できる");
        assert_eq!(f.boundary.settings().expect("読める"), settings);
    }

    /// FR-07: 展開はサービス層へ委ね、失敗は分類付きで返る。
    #[test]
    fn boundary_expand_returns_preview() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("月次報告", 1)])
            .expect("書ける");
        let preview = f
            .boundary
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert_eq!(preview.candidates.len(), 1);
    }

    /// FR-07: 期間の逆転は入力の失敗として返る。
    #[test]
    fn boundary_expand_reversed_range_is_input_failure() {
        let f = fixture();
        let failure = f
            .boundary
            .expand(d(2026, 10, 31), d(2026, 10, 1))
            .expect_err("拒否される");
        assert_eq!(failure.kind, FailureKind::Input);
        assert!(!failure.next_action.is_empty(), "次に取れる操作がない");
    }

    /// EC-32: 保持している候補がラベルの突合に使われる。
    #[tokio::test]
    async fn boundary_expand_uses_cached_labels_for_check() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("月次報告", 1)])
            .expect("書ける");
        // 候補を取得して保持させる。"report" は含むが定義には存在しないラベルも入れる。
        let client = client_with(vec![ok(r#"[{"id":1,"name":"other"}]"#)]);
        f.boundary
            .candidates(&client, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");

        let preview = f
            .boundary
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(
            preview.warnings.iter().any(|w| matches!(
                w,
                crate::expansion::PreviewWarning::UnknownLabels { .. }
            )),
            "{:?}",
            preview.warnings
        );
    }

    /// EC-09: 登録の実行中は新たな登録を開始しない。
    #[tokio::test]
    async fn boundary_rejects_concurrent_registration() {
        let f = fixture();
        // 実行中の印を立てた状態を作る。
        let guard = f.boundary.begin_registration().expect("取得できる");
        assert!(f.boundary.is_registering());

        let client = client_with(vec![created(1)]);
        let failure = f
            .boundary
            .register_batch(
                &client,
                &[item("A", 1)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect_err("拒否される");
        assert_eq!(failure.kind, FailureKind::Busy);
        assert_eq!(client.transport().sent_count(), 0, "登録を始めてはいけない");

        drop(guard);
        assert!(!f.boundary.is_registering());
    }

    /// EC-09: 登録が終われば実行中の印は外れる。
    #[tokio::test]
    async fn boundary_clears_busy_flag_after_registration() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        f.boundary
            .register_batch(
                &client,
                &[item("A", 1)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("登録できる");
        assert!(!f.boundary.is_registering());
    }

    /// FR-09 / FR-11: 登録に成功した発生日は、次の展開で除外される。
    #[tokio::test]
    async fn boundary_registered_dates_are_excluded_next_expand() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("月次報告", 1)])
            .expect("書ける");
        let preview = f
            .boundary
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert_eq!(preview.candidates.len(), 1);

        let client = client_with(vec![created(1)]);
        let item = RegistrationItem {
            schedule_id: ScheduleId("id-月次報告".into()),
            schedule_name: "月次報告".into(),
            occurrence_date: d(2026, 10, 1),
            ..item("月次報告", 1)
        };
        f.boundary
            .register_batch(&client, &[item], now(), &CancellationToken::new(), None)
            .await
            .expect("登録できる");

        let after = f
            .boundary
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(after.candidates.is_empty(), "除外されていない");
    }

    /// EC-10: 通信が届かなくなったら接続状態を縮退させる。
    #[tokio::test]
    async fn boundary_marks_unreachable_after_abort() {
        let f = fixture();
        let client = client_with(vec![Err(TransportError::Timeout)]);
        let outcome = f
            .boundary
            .register_batch(
                &client,
                &[item("A", 1)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("結果は返る");
        assert!(outcome.aborted.is_some());
        assert!(matches!(
            f.boundary.connection.state(),
            ConnectionState::Unreachable { .. }
        ));
    }

    /// EC-11: 認証が失効したら未接続へ戻す。
    #[tokio::test]
    async fn boundary_marks_disconnected_after_authentication_lost() {
        let f = fixture();
        let client = client_with(vec![status(401, r#"{"message":"401 Unauthorized"}"#)]);
        f.boundary
            .register_batch(
                &client,
                &[item("A", 1)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("結果は返る");
        assert_eq!(f.boundary.connection.state(), ConnectionState::Disconnected);
    }

    /// FR-20: 失敗が 0 件なら再実行を拒否する。
    #[tokio::test]
    async fn boundary_retry_without_failures_is_rejected() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        let outcome = f
            .boundary
            .register_batch(
                &client,
                &[item("A", 1)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("登録できる");

        let retry_client = client_with(vec![]);
        let failure = f
            .boundary
            .retry_failed(
                &retry_client,
                &outcome,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect_err("拒否される");
        assert_eq!(failure.kind, FailureKind::Input);
    }

    /// FR-20: 失敗した行のみを再実行できる。
    #[tokio::test]
    async fn boundary_retry_failed_items() {
        let f = fixture();
        let client = client_with(vec![
            created(1),
            status(400, r#"{"message":"invalid"}"#),
        ]);
        let first = f
            .boundary
            .register_batch(
                &client,
                &[item("A", 1), item("B", 2)],
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("登録できる");
        assert_eq!(first.failed(), 1);

        let retry_client = client_with(vec![created(2)]);
        let merged = f
            .boundary
            .retry_failed(
                &retry_client,
                &first,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("再実行できる");
        assert_eq!(merged.succeeded(), 2);
        assert_eq!(retry_client.transport().sent_count(), 1);
    }

    /// FR-23: 履歴を削除すると展開の索引が捨てられ、再び候補になる。
    #[tokio::test]
    async fn boundary_delete_history_invalidates_index() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("月次報告", 1)])
            .expect("書ける");
        let client = client_with(vec![created(1)]);
        let item = RegistrationItem {
            schedule_id: ScheduleId("id-月次報告".into()),
            schedule_name: "月次報告".into(),
            occurrence_date: d(2026, 10, 1),
            ..item("月次報告", 1)
        };
        f.boundary
            .register_batch(&client, &[item], now(), &CancellationToken::new(), None)
            .await
            .expect("登録できる");
        assert!(f
            .boundary
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる")
            .candidates
            .is_empty());

        let removed = f
            .boundary
            .delete_history(&[(ScheduleId("id-月次報告".into()), d(2026, 10, 1))])
            .expect("削除できる");
        assert_eq!(removed, 1);
        assert_eq!(
            f.boundary
                .expand(d(2026, 10, 1), d(2026, 10, 31))
                .expect("展開できる")
                .candidates
                .len(),
            1,
            "削除後に候補へ戻っていない"
        );
    }

    /// NFR-16: 失敗には必ず分類と次に取れる操作が付く。
    #[test]
    fn boundary_failures_carry_kind_and_next_action() {
        let from_save: Failure = SaveError::DuplicateName("月次報告".into()).into();
        assert_eq!(from_save.kind, FailureKind::Input);
        assert!(!from_save.next_action.is_empty());

        let from_connect: Failure = ConnectError::UrlMissing.into();
        assert_eq!(from_connect.kind, FailureKind::Input);
        assert!(!from_connect.next_action.is_empty());

        use gitlabregister_infra::gitlab::ErrorKind;
        for (kind, expected) in [
            (ErrorKind::Authentication, FailureKind::Authentication),
            (ErrorKind::Authorization, FailureKind::Permission),
            (ErrorKind::ProxyAuthentication, FailureKind::Permission),
            (ErrorKind::Validation, FailureKind::Input),
            (ErrorKind::NotFound, FailureKind::NotFound),
            (ErrorKind::Unreachable, FailureKind::Unreachable),
            (ErrorKind::Transient, FailureKind::Temporary),
            (
                ErrorKind::RateLimited { retry_after: None },
                FailureKind::Temporary,
            ),
        ] {
            let failure: Failure = GitLabError::new(kind.clone(), "操作", "理由").into();
            assert_eq!(failure.kind, expected, "{kind:?}");
            assert!(!failure.next_action.is_empty(), "{kind:?}");
        }
    }

    /// 単発登録も多重起動を抑止する (EC-09)。
    #[tokio::test]
    async fn boundary_single_registration_respects_busy_flag() {
        let f = fixture();
        let guard = f.boundary.begin_registration().expect("取得できる");
        let client = client_with(vec![created(1)]);
        let failure = f
            .boundary
            .register_single(&client, &item("単発", 5))
            .await
            .expect_err("拒否される");
        assert_eq!(failure.kind, FailureKind::Busy);
        drop(guard);
    }
}
