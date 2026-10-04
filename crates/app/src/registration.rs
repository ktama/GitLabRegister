//! 一括登録・単発登録・失敗分の再実行 (FR-06、FR-09、FR-10、FR-12、FR-20、EC-08、EC-10、EC-15、NFR-02)。

use chrono::{NaiveDate, NaiveDateTime};
use gitlabregister_core::expand::RegistrationCandidate;
use gitlabregister_core::model::{HistoryEntry, ScheduleId};
use gitlabregister_infra::gitlab::client::{GitLabClient, NewIssue, Tier};
use gitlabregister_infra::gitlab::transport::HttpTransport;
use gitlabregister_infra::gitlab::{ErrorKind, GitLabError};
use gitlabregister_infra::history_store::{HistoryStore, InProgressMark};
use gitlabregister_infra::store::ConfigStore;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 同時に進める登録の数 (NFR-02、設計書 8.4)。
///
/// 1 件が最悪 2 通信になるため (ADR-05)、逐次では 100 件で 200 秒となり
/// NFR-02 の 180 秒を超える。2 件ずつ並行に進めて 100 秒に収める。
pub const CONCURRENCY: usize = 2;

/// 登録する 1 件。プレビュー上の編集を反映した内容を持つ (FR-08)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationItem {
    pub schedule_id: ScheduleId,
    pub schedule_name: String,
    /// 営業日調整後の発生日。履歴のキーになる (設計書 5.2)。
    pub occurrence_date: NaiveDate,
    pub project: String,
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
    pub assignee_ids: Vec<u64>,
    pub milestone: Option<String>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub confidential: bool,
    pub epic_id: Option<u64>,
    /// 指定があれば作成後に別操作で設定する (ADR-05)。
    pub iteration_id: Option<String>,
    pub weight: Option<u32>,
}

impl RegistrationItem {
    /// 展開結果から作る。Issue の項目は定義から引き継ぐ。
    pub fn from_candidate(
        candidate: &RegistrationCandidate,
        template: &gitlabregister_core::model::IssueTemplate,
        assignee_ids: Vec<u64>,
        epic_id: Option<u64>,
        iteration_id: Option<String>,
    ) -> Self {
        Self {
            schedule_id: candidate.occurrence.schedule_id.clone(),
            schedule_name: candidate.schedule_name.clone(),
            occurrence_date: candidate.occurrence.date,
            project: candidate.project.clone(),
            title: candidate.title.clone(),
            description: candidate.description.clone(),
            labels: template.labels.clone(),
            assignee_ids,
            milestone: template.milestone.clone(),
            start_date: candidate.occurrence.start_date,
            due_date: candidate.occurrence.due_date,
            confidential: template.confidential,
            epic_id,
            iteration_id,
            weight: template.weight,
        }
    }

    /// 画面とログに出す対象の識別情報 (NFR-16、FR-17)。
    pub fn describe(&self) -> String {
        format!(
            "{} {} {}",
            self.schedule_name, self.occurrence_date, self.project
        )
    }

    fn to_new_issue(&self, tier: Tier) -> (NewIssue, Vec<&'static str>) {
        let mut dropped = Vec::new();
        let allow_ee = tier.allows_ee_fields();
        // EC-15: ティアが許さない場合は EE 固有項目を外して登録する。
        let assignee_ids = if allow_ee || self.assignee_ids.len() <= 1 {
            self.assignee_ids.clone()
        } else {
            dropped.push("複数担当者");
            self.assignee_ids.iter().take(1).copied().collect()
        };
        let epic_id = if allow_ee {
            self.epic_id
        } else {
            if self.epic_id.is_some() {
                dropped.push("エピック");
            }
            None
        };
        let weight = if allow_ee {
            self.weight
        } else {
            if self.weight.is_some() {
                dropped.push("ウェイト");
            }
            None
        };
        if !allow_ee && self.iteration_id.is_some() {
            dropped.push("イテレーション");
        }
        (
            NewIssue {
                project_id: self.project.clone(),
                title: self.title.clone(),
                description: self.description.clone(),
                labels: self.labels.clone(),
                assignee_ids,
                milestone: self.milestone.clone(),
                start_date: self.start_date,
                due_date: self.due_date,
                confidential: self.confidential,
                epic_id,
                weight,
            },
            dropped,
        )
    }
}

/// 1 件の結果 (FR-10)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOutcome {
    pub item: RegistrationItem,
    pub status: ItemStatus,
}

/// 1 件の成否。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemStatus {
    /// 作成に成功した。
    Succeeded {
        issue_reference: String,
        web_url: String,
        /// イテレーションの設定だけが失敗した場合の理由 (FR-06)。
        iteration_unset: Option<String>,
        /// EC-15 により登録から外した項目。
        dropped_fields: Vec<String>,
    },
    /// 作成に失敗した。
    Failed { kind: ErrorKind, message: String },
    /// 中止または全体停止により処理しなかった (FR-09、EC-10)。
    NotAttempted,
}

impl ItemStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, ItemStatus::Succeeded { .. })
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, ItemStatus::Failed { .. })
    }
}

/// 一括登録の結果 (FR-10)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub outcomes: Vec<ItemOutcome>,
    /// 全体を止めた理由 (EC-10、EC-11)。
    pub aborted: Option<AbortReason>,
    /// 利用者が中止した (FR-09)。
    pub cancelled: bool,
}

/// 全体を止めた理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// 通信が届かなくなった (EC-10)。
    Unreachable { message: String },
    /// 認証が失効した (EC-11)。
    AuthenticationLost { message: String },
}

impl BatchOutcome {
    pub fn succeeded(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.status.is_success())
            .count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.status.is_failure())
            .count()
    }

    pub fn not_attempted(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, ItemStatus::NotAttempted))
            .count()
    }

    /// 失敗した行のみを取り出す (FR-20)。
    pub fn failed_items(&self) -> Vec<RegistrationItem> {
        self.outcomes
            .iter()
            .filter(|o| o.status.is_failure())
            .map(|o| o.item.clone())
            .collect()
    }

    /// 失敗分の再実行を実行できるか (FR-20)。
    pub fn can_retry(&self) -> bool {
        self.failed() > 0
    }
}

/// 進捗の通知 (FR-09)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub completed: usize,
    pub total: usize,
}

/// 進捗の受け取り口。
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// 中止の指示を伝える札 (FR-09)。
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 登録を進める。
pub struct RegistrationService {
    config: ConfigStore,
    history: HistoryStore,
    concurrency: usize,
}

impl RegistrationService {
    pub fn new(config: ConfigStore) -> Self {
        let history = HistoryStore::new(config.paths().history());
        Self {
            config,
            history,
            concurrency: CONCURRENCY,
        }
    }

    /// 同時実行数を変えて作る。NFR-02 の計測と、逐次での比較に使う。
    pub fn with_concurrency(config: ConfigStore, concurrency: usize) -> Self {
        let history = HistoryStore::new(config.paths().history());
        Self {
            config,
            history,
            concurrency: concurrency.max(1),
        }
    }

    /// 選択された登録候補を 1 件ずつ登録する (FR-09)。
    ///
    /// 件単位で成否を判定し、再試行しない分類は即座に失敗として記録して次へ進む。
    /// 全体を止めるのは到達不能と認証エラーの 2 つに限る (設計書 8.1)。
    pub async fn register_batch<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        items: &[RegistrationItem],
        tier: Tier,
        now: NaiveDateTime,
        cancel: &CancellationToken,
        progress: Option<ProgressSink>,
    ) -> BatchOutcome {
        // EC-08: 一括登録の開始を印として残し、正常終了時に消す。
        let _ = self.config.mark_registration_started(&InProgressMark {
            started_at: now,
            planned_count: items.len(),
        });

        let total = items.len();
        let completed = AtomicUsize::new(0);
        let abort: Mutex<Option<AbortReason>> = Mutex::new(None);
        let results: Mutex<Vec<Option<ItemOutcome>>> = Mutex::new(vec![None; total]);

        let mut index = 0usize;
        while index < total {
            if cancel.is_cancelled() || abort.lock().expect("施錠できる").is_some() {
                break;
            }
            // 同時実行数の分だけまとめて進める。
            let end = (index + self.concurrency).min(total);
            let batch: Vec<usize> = (index..end).collect();
            let futures: Vec<_> = batch
                .iter()
                .map(|i| self.register_one(client, &items[*i], tier, now))
                .collect();
            let outcomes = join_all(futures).await;

            for (position, outcome) in batch.iter().zip(outcomes) {
                if let ItemStatus::Failed { kind, message } = &outcome.status {
                    if kind.aborts_batch() {
                        let reason = match kind {
                            ErrorKind::Authentication => AbortReason::AuthenticationLost {
                                message: message.clone(),
                            },
                            _ => AbortReason::Unreachable {
                                message: message.clone(),
                            },
                        };
                        *abort.lock().expect("施錠できる") = Some(reason);
                    }
                }
                results.lock().expect("施錠できる")[*position] = Some(outcome);
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(sink) = &progress {
                    sink(Progress {
                        completed: done,
                        total,
                    });
                }
            }
            index = end;
        }

        let aborted = abort.lock().expect("施錠できる").clone();
        let cancelled = cancel.is_cancelled();
        let outcomes = results
            .into_inner()
            .expect("施錠できる")
            .into_iter()
            .enumerate()
            .map(|(i, slot)| {
                slot.unwrap_or_else(|| ItemOutcome {
                    item: items[i].clone(),
                    status: ItemStatus::NotAttempted,
                })
            })
            .collect();

        // 中止・全体停止でも印は消す。印は「中断されたまま終わった」ことを表すためのもので、
        // 利用者が意図して止めた場合と区別する (EC-08)。
        let _ = self.config.clear_registration_mark();

        BatchOutcome {
            outcomes,
            aborted,
            cancelled,
        }
    }

    /// 失敗した行のみを再実行する (FR-20)。
    ///
    /// 対象は前回の結果のうち失敗した行である。呼び出し側に別途渡させると、
    /// 前回の結果と対象が食い違う余地が生まれるため、ここで前回の結果から求める。
    pub async fn retry_failed<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        previous: &BatchOutcome,
        tier: Tier,
        now: NaiveDateTime,
        cancel: &CancellationToken,
        progress: Option<ProgressSink>,
    ) -> BatchOutcome {
        let items = previous.failed_items();
        let retried = self
            .register_batch(client, &items, tier, now, cancel, progress)
            .await;

        // 既に成功している行は処理せず、前回の結果をそのまま残す。
        let mut merged = previous.clone();
        for outcome in retried.outcomes {
            if let Some(slot) = merged.outcomes.iter_mut().find(|o| {
                o.item.schedule_id == outcome.item.schedule_id
                    && o.item.occurrence_date == outcome.item.occurrence_date
            }) {
                if !slot.status.is_success() {
                    *slot = outcome;
                }
            } else {
                merged.outcomes.push(outcome);
            }
        }
        merged.aborted = retried.aborted;
        merged.cancelled = retried.cancelled;
        merged
    }

    /// 定義を介さず 1 件登録する (FR-12)。履歴には記録しない。
    pub async fn register_single<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        item: &RegistrationItem,
        tier: Tier,
    ) -> Result<ItemStatus, GitLabError> {
        let (issue, dropped) = item.to_new_issue(tier);
        let created = client.create_issue(&issue).await?;
        let mut iteration_unset = None;
        if let Some(iteration) = &item.iteration_id {
            if tier.allows_ee_fields() {
                if let Err(e) = client.set_iteration(created.id, iteration).await {
                    iteration_unset = Some(e.message);
                }
            }
        }
        Ok(ItemStatus::Succeeded {
            issue_reference: format!("#{}", created.iid),
            web_url: created.web_url,
            iteration_unset,
            dropped_fields: dropped.into_iter().map(|s| s.to_string()).collect(),
        })
    }

    /// 1 件を登録し、成功したら履歴へ追記する (ADR-04、設計書 7.3)。
    async fn register_one<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        item: &RegistrationItem,
        tier: Tier,
        now: NaiveDateTime,
    ) -> ItemOutcome {
        let (issue, dropped) = item.to_new_issue(tier);
        let created = match client.create_issue(&issue).await {
            Ok(created) => created,
            Err(error) => {
                return ItemOutcome {
                    item: item.clone(),
                    status: ItemStatus::Failed {
                        kind: error.kind,
                        message: error.message,
                    },
                }
            }
        };

        // 設計書 7.3: 作成が成功した時点で履歴へ追記する。
        // イテレーションの設定結果は履歴の有無に影響しない。
        let _ = self.history.append(&HistoryEntry {
            schedule_id: item.schedule_id.clone(),
            schedule_name: item.schedule_name.clone(),
            occurrence_date: item.occurrence_date,
            registered_at: now,
            project: item.project.clone(),
            issue_reference: format!("#{}", created.iid),
        });

        let mut iteration_unset = None;
        if let Some(iteration) = &item.iteration_id {
            if tier.allows_ee_fields() {
                if let Err(error) = client.set_iteration(created.id, iteration).await {
                    // FR-06: Issue の作成は成功として扱い、未設定であることを併記する。
                    iteration_unset = Some(error.message);
                }
            }
        }

        ItemOutcome {
            item: item.clone(),
            status: ItemStatus::Succeeded {
                issue_reference: format!("#{}", created.iid),
                web_url: created.web_url,
                iteration_unset,
                dropped_fields: dropped.into_iter().map(|s| s.to_string()).collect(),
            },
        }
    }
}

/// 複数の未来を同時に進める。
///
/// 外部の依存を増やさないため、同時実行数の分だけ手で合流させる。
/// 同時実行数は 2 を既定とするため、この形で足りる (NFR-02)。
async fn join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut iter = futures.into_iter();
    match (iter.next(), iter.next()) {
        (None, _) => Vec::new(),
        (Some(a), None) => vec![a.await],
        (Some(a), Some(b)) => {
            let mut rest: Vec<F> = iter.collect();
            if rest.is_empty() {
                let (ra, rb) = tokio::join!(a, b);
                vec![ra, rb]
            } else {
                // 3 件以上は 2 件ずつに分けて進める。
                let (ra, rb) = tokio::join!(a, b);
                let mut out = vec![ra, rb];
                let tail = std::mem::take(&mut rest);
                out.extend(Box::pin(join_all(tail)).await);
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{client_with, ok, status};
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
        service: RegistrationService,
    }

    fn fixture() -> Fixture {
        fixture_with_concurrency(CONCURRENCY)
    }

    fn fixture_with_concurrency(concurrency: usize) -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let root = dir.path().join("GitLabRegister");
        let config = ConfigStore::new(AppPaths::with_root(root.clone()));
        let service = RegistrationService::with_concurrency(
            ConfigStore::new(AppPaths::with_root(root)),
            concurrency,
        );
        Fixture {
            _dir: dir,
            config,
            service,
        }
    }

    fn item(name: &str, day: u32) -> RegistrationItem {
        RegistrationItem {
            schedule_id: ScheduleId(format!("id-{name}")),
            schedule_name: name.into(),
            occurrence_date: d(2026, 10, day),
            project: "group/alpha".into(),
            title: format!("{name} の作業"),
            description: String::new(),
            labels: vec![],
            assignee_ids: vec![],
            milestone: None,
            start_date: None,
            due_date: Some(d(2026, 10, day + 7)),
            confidential: false,
            epic_id: None,
            iteration_id: None,
            weight: None,
        }
    }

    fn created(
        iid: u64,
    ) -> Result<gitlabregister_infra::gitlab::transport::HttpResponse, TransportError> {
        ok(&format!(
            r#"{{"id":{},"iid":{iid},"web_url":"https://gitlab.example.com/-/issues/{iid}"}}"#,
            1000 + iid
        ))
    }

    fn iteration_ok(
    ) -> Result<gitlabregister_infra::gitlab::transport::HttpResponse, TransportError> {
        ok(r#"{"data":{"issueSetIteration":{"errors":[]}}}"#)
    }

    /// FR-09: 選択された件数だけ登録され、履歴に追記される。
    #[tokio::test]
    async fn registration_registers_all_items() {
        let f = fixture();
        let client = client_with(vec![created(1), created(2), created(3)]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3)];

        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 3);
        assert_eq!(outcome.failed(), 0);
        assert!(outcome.aborted.is_none());

        // ADR-04: 成功した分が履歴に追記されている。
        let history = HistoryStore::new(f.config.paths().history())
            .load()
            .expect("読める");
        assert_eq!(history.entries.len(), 3);
    }

    /// FR-09: 1 件の処理完了ごとに進捗が通知される。
    #[tokio::test]
    async fn registration_reports_progress_per_item() {
        let f = fixture();
        let client = client_with(vec![created(1), created(2), created(3)]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3)];
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_store = seen.clone();
        let sink: ProgressSink = Arc::new(move |p| {
            sink_store.lock().expect("施錠できる").push(p);
        });

        f.service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                Some(sink),
            )
            .await;

        let progress = seen.lock().expect("施錠できる").clone();
        assert_eq!(
            progress.len(),
            3,
            "1 件ごとに通知されていない: {progress:?}"
        );
        assert_eq!(progress.last().expect("ある").completed, 3);
        assert!(progress.iter().all(|p| p.total == 3));
    }

    /// FR-10: 件ごとの成否が区別して返り、失敗理由が載る。
    #[tokio::test]
    async fn registration_reports_per_item_outcome() {
        let f = fixture();
        let client = client_with(vec![
            created(1),
            status(400, r#"{"message":{"milestone":["not found"]}}"#),
            created(3),
        ]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3)];

        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 2);
        assert_eq!(outcome.failed(), 1);
        let failed = outcome
            .outcomes
            .iter()
            .find(|o| o.status.is_failure())
            .expect("失敗がある");
        assert_eq!(failed.item.schedule_name, "B");
        match &failed.status {
            ItemStatus::Failed { kind, message } => {
                assert_eq!(*kind, ErrorKind::Validation);
                assert!(message.contains("milestone"), "{message}");
            }
            other => panic!("失敗として扱われていない: {other:?}"),
        }
        // 失敗した分は履歴に残らない。
        let history = HistoryStore::new(f.config.paths().history())
            .load()
            .expect("読める");
        assert_eq!(history.entries.len(), 2);
    }

    /// 設計書 8.1: 入力エラーでは全体を止めず他の件を続ける。
    #[tokio::test]
    async fn registration_validation_error_does_not_abort() {
        let f = fixture();
        let client = client_with(vec![
            status(400, r#"{"message":"invalid"}"#),
            created(2),
            created(3),
        ]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3)];
        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(outcome.aborted.is_none(), "{:?}", outcome.aborted);
        assert_eq!(outcome.succeeded(), 2);
        assert_eq!(outcome.not_attempted(), 0);
    }

    /// EC-10: 通信が切断されたら処理を止め、登録済み件数と未登録件数が分かる。
    #[tokio::test]
    async fn registration_abort_on_unreachable() {
        let f = fixture_with_concurrency(1);
        let client = client_with(vec![
            created(1),
            Err(TransportError::Timeout),
            created(3),
            created(4),
        ]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3), item("D", 4)];

        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(matches!(
            outcome.aborted,
            Some(AbortReason::Unreachable { .. })
        ));
        assert_eq!(outcome.succeeded(), 1);
        assert_eq!(outcome.failed(), 1);
        assert_eq!(outcome.not_attempted(), 2, "残りは処理しない");
    }

    /// EC-11: 認証が失効したら全体を止める。
    #[tokio::test]
    async fn registration_abort_on_authentication_lost() {
        let f = fixture_with_concurrency(1);
        let client = client_with(vec![
            status(401, r#"{"message":"401 Unauthorized"}"#),
            created(2),
        ]);
        let items = vec![item("A", 1), item("B", 2)];
        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(matches!(
            outcome.aborted,
            Some(AbortReason::AuthenticationLost { .. })
        ));
        assert_eq!(outcome.not_attempted(), 1);
    }

    /// FR-09: 中止すると未処理分は登録せず、登録済み分は取り消さない。
    #[tokio::test]
    async fn registration_cancel_stops_remaining() {
        let f = fixture_with_concurrency(1);
        let client = client_with(vec![created(1), created(2), created(3)]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3)];
        let cancel = CancellationToken::new();
        // 最初の 1 件を進めたあとに中止する。
        let sink_cancel = cancel.clone();
        let sink: ProgressSink = Arc::new(move |p| {
            if p.completed == 1 {
                sink_cancel.cancel();
            }
        });

        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &cancel,
                Some(sink),
            )
            .await;
        assert!(outcome.cancelled);
        assert_eq!(outcome.succeeded(), 1, "登録済み分は残る");
        assert_eq!(outcome.not_attempted(), 2);
        // 登録済み分の履歴は取り消さない。
        let history = HistoryStore::new(f.config.paths().history())
            .load()
            .expect("読める");
        assert_eq!(history.entries.len(), 1);
    }

    /// FR-06 / ADR-05: イテレーション指定があると作成後に別操作で設定する。
    #[tokio::test]
    async fn registration_iteration_set_after_create() {
        let f = fixture();
        let client = client_with(vec![created(1), iteration_ok()]);
        let mut i = item("A", 1);
        i.iteration_id = Some("gid://gitlab/Iteration/55".into());

        let outcome = f
            .service
            .register_batch(
                &client,
                &[i],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 1);
        match &outcome.outcomes[0].status {
            ItemStatus::Succeeded {
                iteration_unset, ..
            } => assert!(iteration_unset.is_none(), "{iteration_unset:?}"),
            other => panic!("成功していない: {other:?}"),
        }
        // 2 通信になる (ADR-05)。
        assert_eq!(client.transport().sent_count(), 2);
    }

    /// FR-06: イテレーションの設定だけが失敗した場合は成功扱いで、未設定であることを併記する。
    #[tokio::test]
    async fn registration_iteration_failure_keeps_success() {
        let f = fixture();
        let client = client_with(vec![
            created(1),
            ok(r#"{"data":{"issueSetIteration":{"errors":["Iteration not found"]}}}"#),
        ]);
        let mut i = item("A", 1);
        i.iteration_id = Some("gid://gitlab/Iteration/55".into());

        let outcome = f
            .service
            .register_batch(
                &client,
                &[i],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 1, "作成は成功として扱う");
        match &outcome.outcomes[0].status {
            ItemStatus::Succeeded {
                iteration_unset, ..
            } => {
                let reason = iteration_unset.as_ref().expect("未設定の理由がある");
                assert!(reason.contains("Iteration"), "{reason}");
            }
            other => panic!("成功していない: {other:?}"),
        }
        // 設計書 7.3: 履歴には記録される。
        let history = HistoryStore::new(f.config.paths().history())
            .load()
            .expect("読める");
        assert_eq!(history.entries.len(), 1);
    }

    /// ADR-05: イテレーション指定がなければ 1 通信で済ませる。
    #[tokio::test]
    async fn registration_without_iteration_uses_single_request() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        f.service
            .register_batch(
                &client,
                &[item("A", 1)],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(client.transport().sent_count(), 1, "無駄な往復をしている");
    }

    /// EC-15: ティアが許さない場合は EE 固有項目を外して登録し、外した項目名を返す。
    #[tokio::test]
    async fn registration_tier_drops_ee_fields() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        let mut i = item("A", 1);
        i.epic_id = Some(42);
        i.weight = Some(3);
        i.assignee_ids = vec![7, 9];
        i.iteration_id = Some("gid://gitlab/Iteration/55".into());

        let outcome = f
            .service
            .register_batch(
                &client,
                &[i],
                Tier::Free,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 1);
        match &outcome.outcomes[0].status {
            ItemStatus::Succeeded { dropped_fields, .. } => {
                for expected in ["複数担当者", "エピック", "ウェイト", "イテレーション"]
                {
                    assert!(
                        dropped_fields.iter().any(|d| d == expected),
                        "{expected} が外されていない: {dropped_fields:?}"
                    );
                }
            }
            other => panic!("成功していない: {other:?}"),
        }
        // EE 固有項目を送っていない。
        let body = client
            .transport()
            .last_request()
            .expect("送信された")
            .body
            .expect("本文がある");
        assert!(body.get("epic_id").is_none(), "{body}");
        assert!(body.get("weight").is_none(), "{body}");
        assert_eq!(body["assignee_ids"], serde_json::json!([7]));
        // イテレーションの設定操作も行わない。
        assert_eq!(client.transport().sent_count(), 1);
    }

    /// FR-20: 失敗した行のみを再実行し、成功済みの行は処理しない。
    #[tokio::test]
    async fn registration_retry_failed_only() {
        let f = fixture();
        let client = client_with(vec![created(1), status(400, r#"{"message":"invalid"}"#)]);
        let items = vec![item("A", 1), item("B", 2)];
        let first = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(first.failed(), 1);
        assert!(first.can_retry());

        let failed = first.failed_items();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].schedule_name, "B");

        let retry_client = client_with(vec![created(2)]);
        let merged = f
            .service
            .retry_failed(
                &retry_client,
                &first,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(merged.succeeded(), 2, "全件成功として表示されない");
        assert_eq!(merged.failed(), 0);
        // 成功済みの行に対して登録処理をしていない。
        assert_eq!(retry_client.transport().sent_count(), 1);
    }

    /// FR-20: 失敗が 0 件なら再実行は実行できない。
    #[tokio::test]
    async fn registration_retry_not_available_without_failures() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        let outcome = f
            .service
            .register_batch(
                &client,
                &[item("A", 1)],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(!outcome.can_retry());
        assert!(outcome.failed_items().is_empty());
    }

    /// FR-12: 単発登録は履歴に記録しない。
    #[tokio::test]
    async fn registration_single_does_not_record_history() {
        let f = fixture();
        let client = client_with(vec![created(9)]);
        let status = f
            .service
            .register_single(&client, &item("単発", 5), Tier::PremiumOrAbove)
            .await
            .expect("登録できる");
        assert!(status.is_success());
        match status {
            ItemStatus::Succeeded {
                issue_reference, ..
            } => assert_eq!(issue_reference, "#9"),
            other => panic!("成功していない: {other:?}"),
        }
        let history = HistoryStore::new(f.config.paths().history())
            .load()
            .expect("読める");
        assert!(history.entries.is_empty(), "単発登録が履歴に残っている");
    }

    /// FR-12: 単発登録が失敗した場合は理由を返す。
    #[tokio::test]
    async fn registration_single_returns_error() {
        let f = fixture();
        let client = client_with(vec![status(403, r#"{"message":"403 Forbidden"}"#)]);
        let err = f
            .service
            .register_single(&client, &item("単発", 5), Tier::PremiumOrAbove)
            .await
            .expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Authorization);
    }

    /// EC-08: 一括登録の開始で印を残し、終了時に消す。
    #[tokio::test]
    async fn registration_clears_in_progress_mark_on_finish() {
        let f = fixture();
        let client = client_with(vec![created(1)]);
        f.service
            .register_batch(
                &client,
                &[item("A", 1)],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(
            f.config
                .interrupted_registration()
                .expect("読める")
                .is_none(),
            "印が残っている"
        );
    }

    /// EC-08: 全体停止で終わった場合も印は消す。意図した中断と区別しないため。
    #[tokio::test]
    async fn registration_clears_mark_even_when_aborted() {
        let f = fixture_with_concurrency(1);
        let client = client_with(vec![Err(TransportError::Timeout)]);
        let outcome = f
            .service
            .register_batch(
                &client,
                &[item("A", 1)],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(outcome.aborted.is_some());
        assert!(f
            .config
            .interrupted_registration()
            .expect("読める")
            .is_none());
    }

    /// NFR-02: 既定の同時実行数は 2 で、設計書 8.4 の値と一致する。
    #[test]
    fn registration_default_concurrency_is_two() {
        assert_eq!(CONCURRENCY, 2);
    }

    /// NFR-02: 同時実行数 2 では 2 件が同じ組で進む。
    #[tokio::test]
    async fn registration_concurrency_processes_two_at_a_time() {
        let f = fixture_with_concurrency(2);
        // 1 件目で全体停止になる応答を置く。同じ組の 2 件目は処理され、3 件目以降は処理されない。
        let client = client_with(vec![
            Err(TransportError::Timeout),
            created(2),
            created(3),
            created(4),
        ]);
        let items = vec![item("A", 1), item("B", 2), item("C", 3), item("D", 4)];
        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(outcome.aborted.is_some());
        assert_eq!(outcome.succeeded(), 1, "同じ組の 2 件目は処理される");
        assert_eq!(outcome.not_attempted(), 2, "次の組は処理しない");
    }

    /// 3 件以上でも同時実行数の組に分けてすべて処理される。
    #[tokio::test]
    async fn registration_concurrency_handles_odd_counts() {
        let f = fixture_with_concurrency(2);
        let client = client_with(vec![
            created(1),
            created(2),
            created(3),
            created(4),
            created(5),
        ]);
        let items: Vec<RegistrationItem> = (1..=5).map(|i| item(&format!("S{i}"), i)).collect();
        let outcome = f
            .service
            .register_batch(
                &client,
                &items,
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.succeeded(), 5);
        assert_eq!(outcome.outcomes.len(), 5);
    }

    /// NFR-16: 失敗の表示に使う対象の識別情報が揃っている。
    #[test]
    fn registration_item_describe_includes_name_date_project() {
        let described = item("月次報告", 5).describe();
        assert!(described.contains("月次報告"), "{described}");
        assert!(described.contains("2026-10-05"), "{described}");
        assert!(described.contains("group/alpha"), "{described}");
    }

    /// 0 件の一括登録は何もせず成功 0 件で返る。
    #[tokio::test]
    async fn registration_empty_batch() {
        let f = fixture();
        let client = client_with(vec![]);
        let outcome = f
            .service
            .register_batch(
                &client,
                &[],
                Tier::PremiumOrAbove,
                now(),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert_eq!(outcome.outcomes.len(), 0);
        assert!(outcome.aborted.is_none());
        assert!(!outcome.can_retry());
    }
}
