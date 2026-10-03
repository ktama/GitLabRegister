//! Tauri コマンド。UI とサービス層の唯一の接点 (設計書 6.1)。
//!
//! ここは変換に徹する。判断はコマンド境界 (`Boundary`) が行う。
//! 失敗は分類と文面を持つ形で返し、画面は分類だけを見て表示を切り替える (NFR-16)。

use crate::state::AppState;
use chrono::{NaiveDate, NaiveDateTime};
use gitlabregister_app::boundary::{Failure, FailureKind};
use gitlabregister_app::candidate::{CandidateList, Freshness};
use gitlabregister_app::expansion::{Preview, PreviewWarning};
use gitlabregister_app::masterdata::HistoryFilter;
use gitlabregister_app::registration::{BatchOutcome, ItemStatus, Progress, RegistrationItem};
use gitlabregister_core::expand::RangePreset;
use gitlabregister_core::model::{Holiday, HistoryEntry, Schedule, ScheduleId};
use gitlabregister_infra::gitlab::client::CandidateKind;
use gitlabregister_infra::store::{AppSettings, ImportConflictPolicy, Project};
use gitlabregister_infra::Level;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{Emitter, State};

/// 画面へ返す失敗。
#[derive(Debug, Clone, Serialize)]
pub struct UiError {
    /// 分類。画面はこの値だけを見る。
    pub kind: String,
    pub message: String,
    /// 利用者が次に取れる操作 (NFR-16)。
    pub next_action: String,
}

impl From<Failure> for UiError {
    fn from(failure: Failure) -> Self {
        Self {
            kind: match failure.kind {
                FailureKind::Input => "input",
                FailureKind::Authentication => "authentication",
                FailureKind::Permission => "permission",
                FailureKind::NotFound => "not_found",
                FailureKind::Unreachable => "unreachable",
                FailureKind::Temporary => "temporary",
                FailureKind::Storage => "storage",
                FailureKind::Busy => "busy",
            }
            .into(),
            message: failure.message,
            next_action: failure.next_action,
        }
    }
}

impl UiError {
    fn input(message: impl Into<String>, next_action: impl Into<String>) -> Self {
        Self {
            kind: "input".into(),
            message: message.into(),
            next_action: next_action.into(),
        }
    }

    fn not_connected() -> Self {
        Self {
            kind: "unreachable".into(),
            message: "GitLab-EE に接続していません".into(),
            next_action: "接続画面でパーソナルアクセストークンを入力してください".into(),
        }
    }

    fn storage(error: impl std::fmt::Display) -> Self {
        Self {
            kind: "storage".into(),
            message: error.to_string(),
            next_action: "設定フォルダの状態を確認してください".into(),
        }
    }
}

type UiResult<T> = Result<T, UiError>;

/// 画面へ返す接続の状態 (設計書 7.5)。
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionView {
    /// `disconnected` / `connected` / `unreachable` / `version_unsupported`
    pub state: String,
    pub description: String,
    pub can_register: bool,
    pub can_fetch_candidates: bool,
    pub allows_ee_fields: bool,
}

fn to_view(state: &gitlabregister_app::ConnectionState) -> ConnectionView {
    use gitlabregister_app::ConnectionState as S;
    ConnectionView {
        state: match state {
            S::Disconnected => "disconnected",
            S::Connected(_) => "connected",
            S::Unreachable { .. } => "unreachable",
            S::VersionUnsupported { .. } => "version_unsupported",
        }
        .into(),
        description: state.describe(),
        can_register: state.can_register(),
        can_fetch_candidates: state.can_fetch_candidates(),
        allows_ee_fields: state.allows_ee_fields(),
    }
}

/// 画面へ返す接続の結果。
#[derive(Debug, Clone, Serialize)]
pub struct ConnectView {
    pub connection: ConnectionView,
    pub warnings: Vec<String>,
}

/// 画面へ返すプレビュー (設計書 6.1)。
#[derive(Debug, Clone, Serialize)]
pub struct PreviewView {
    pub candidates: Vec<CandidateRow>,
    pub excluded: Vec<ExcludedRow>,
    pub warnings: Vec<String>,
    pub can_register: bool,
    pub no_enabled_schedules: bool,
}

/// プレビューの 1 行 (FR-07、FR-08)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRow {
    pub schedule_id: String,
    pub schedule_name: String,
    pub project: String,
    pub occurrence_date: NaiveDate,
    /// 営業日調整の前の発生日。調整が効いたことを画面で示す。
    pub original_date: NaiveDate,
    pub title: String,
    pub description: String,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub labels: Vec<String>,
    pub assignee_ids: Vec<u64>,
    pub milestone: Option<String>,
    pub confidential: bool,
    pub epic_id: Option<u64>,
    pub iteration_id: Option<String>,
    pub weight: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExcludedRow {
    pub schedule_id: String,
    pub occurrence_date: NaiveDate,
}

impl CandidateRow {
    fn to_item(&self) -> RegistrationItem {
        RegistrationItem {
            schedule_id: ScheduleId(self.schedule_id.clone()),
            schedule_name: self.schedule_name.clone(),
            occurrence_date: self.occurrence_date,
            project: self.project.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            labels: self.labels.clone(),
            assignee_ids: self.assignee_ids.clone(),
            milestone: self.milestone.clone(),
            start_date: self.start_date,
            due_date: self.due_date,
            confidential: self.confidential,
            epic_id: self.epic_id,
            iteration_id: self.iteration_id.clone(),
            weight: self.weight,
        }
    }
}

fn to_preview_view(preview: Preview, schedules: &[Schedule]) -> PreviewView {
    let can_register = preview.can_register();
    PreviewView {
        can_register,
        no_enabled_schedules: preview.no_enabled_schedules,
        warnings: preview
            .warnings
            .iter()
            .map(PreviewWarning::describe)
            .collect(),
        excluded: preview
            .excluded
            .iter()
            .map(|(id, date)| ExcludedRow {
                schedule_id: id.clone(),
                occurrence_date: *date,
            })
            .collect(),
        candidates: preview
            .candidates
            .iter()
            .map(|c| {
                let template = schedules
                    .iter()
                    .find(|s| s.id == c.occurrence.schedule_id)
                    .map(|s| s.issue.clone())
                    .unwrap_or_default();
                CandidateRow {
                    schedule_id: c.occurrence.schedule_id.0.clone(),
                    schedule_name: c.schedule_name.clone(),
                    project: c.project.clone(),
                    occurrence_date: c.occurrence.date,
                    original_date: c.occurrence.original_date,
                    title: c.title.clone(),
                    description: c.description.clone(),
                    start_date: c.occurrence.start_date,
                    due_date: c.occurrence.due_date,
                    labels: template.labels.clone(),
                    // 担当者・エピック・イテレーションは識別子の対応付けが必要なため、
                    // 画面で候補から選んだ値を行へ入れる。定義からは引き継がない。
                    assignee_ids: Vec::new(),
                    milestone: template.milestone.clone(),
                    confidential: template.confidential,
                    epic_id: None,
                    iteration_id: None,
                    weight: template.weight,
                }
            })
            .collect(),
    }
}

/// 画面へ返す一括登録の結果 (FR-10)。
#[derive(Debug, Clone, Serialize)]
pub struct BatchView {
    pub rows: Vec<ResultRow>,
    pub succeeded: usize,
    pub failed: usize,
    pub not_attempted: usize,
    pub can_retry: bool,
    pub aborted: Option<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultRow {
    pub schedule_name: String,
    pub occurrence_date: NaiveDate,
    pub project: String,
    /// `succeeded` / `failed` / `not_attempted`
    pub status: String,
    pub issue_reference: Option<String>,
    pub web_url: Option<String>,
    /// 失敗の理由、またはイテレーション未設定の理由 (FR-06、NFR-16)。
    pub note: Option<String>,
    pub next_action: Option<String>,
    /// EC-15 により登録から外した項目。
    pub dropped_fields: Vec<String>,
}

fn to_batch_view(outcome: &BatchOutcome) -> BatchView {
    BatchView {
        succeeded: outcome.succeeded(),
        failed: outcome.failed(),
        not_attempted: outcome.not_attempted(),
        can_retry: outcome.can_retry(),
        cancelled: outcome.cancelled,
        aborted: outcome.aborted.as_ref().map(|reason| match reason {
            gitlabregister_app::AbortReason::Unreachable { message } => {
                format!("通信が届かなくなったため中断しました: {message}")
            }
            gitlabregister_app::AbortReason::AuthenticationLost { message } => {
                format!("認証が失効したため中断しました: {message}")
            }
        }),
        rows: outcome
            .outcomes
            .iter()
            .map(|o| {
                let (status, issue_reference, web_url, note, next_action, dropped) = match &o.status
                {
                    ItemStatus::Succeeded {
                        issue_reference,
                        web_url,
                        iteration_unset,
                        dropped_fields,
                    } => (
                        "succeeded",
                        Some(issue_reference.clone()),
                        Some(web_url.clone()),
                        iteration_unset
                            .as_ref()
                            .map(|r| format!("イテレーションは未設定です: {r}")),
                        None,
                        dropped_fields.clone(),
                    ),
                    ItemStatus::Failed { kind, message } => (
                        "failed",
                        None,
                        None,
                        Some(message.clone()),
                        Some(kind.next_action().to_string()),
                        Vec::new(),
                    ),
                    ItemStatus::NotAttempted => (
                        "not_attempted",
                        None,
                        None,
                        Some("中断により処理していません".into()),
                        Some("中断の理由を確認して再実行してください".into()),
                        Vec::new(),
                    ),
                };
                ResultRow {
                    schedule_name: o.item.schedule_name.clone(),
                    occurrence_date: o.item.occurrence_date,
                    project: o.item.project.clone(),
                    status: status.into(),
                    issue_reference,
                    web_url,
                    note,
                    next_action,
                    dropped_fields: dropped,
                }
            })
            .collect(),
    }
}

fn today() -> NaiveDate {
    crate::clock::today()
}

fn now() -> NaiveDateTime {
    crate::clock::now()
}

// ---- 設定と接続 ----

#[tauri::command]
pub fn load_settings(state: State<'_, AppState>) -> UiResult<AppSettings> {
    state.boundary.settings().map_err(UiError::from)
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: AppSettings) -> UiResult<()> {
    state
        .boundary
        .save_settings(&settings)
        .map_err(UiError::from)?;
    state.log(Level::Info, "設定の変更", "接続先とプロキシ", "保存");
    Ok(())
}

#[tauri::command]
pub fn connection_state(state: State<'_, AppState>) -> ConnectionView {
    to_view(&state.boundary.connection.state())
}

/// 接続する (FR-01、NFR-17)。
///
/// トークンとプロキシの資格情報はここで受け取って SecretHolder へ渡し、
/// 画面側には返さない (NFR-05)。
#[tauri::command]
pub async fn connect(
    state: State<'_, AppState>,
    settings: AppSettings,
    token: String,
    proxy_user: Option<String>,
    proxy_password: Option<String>,
) -> UiResult<ConnectView> {
    if token.trim().is_empty() {
        return Err(UiError::input(
            "パーソナルアクセストークンを入力してください",
            "GitLab-EE で発行したトークンを入力してください",
        ));
    }
    state
        .boundary
        .save_settings(&settings)
        .map_err(UiError::from)?;

    let credentials = match (proxy_user, proxy_password) {
        (Some(u), Some(p)) if !u.is_empty() => Some((u, p)),
        _ => None,
    };
    let transport = state
        .boundary
        .connection
        .prepare_transport(&settings, &token, credentials)
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.set_client(transport);
    state.refresh_redaction();

    let client = state.client().ok_or_else(UiError::not_connected)?;
    match state.boundary.connect(client.as_ref()).await {
        Ok(outcome) => {
            state.log(
                Level::Info,
                "接続",
                settings.gitlab_url.as_deref().unwrap_or("-"),
                &outcome.state.describe(),
            );
            Ok(ConnectView {
                connection: to_view(&outcome.state),
                warnings: outcome.warnings,
            })
        }
        Err(failure) => {
            state.clear_client();
            state.log(
                Level::Error,
                "接続",
                settings.gitlab_url.as_deref().unwrap_or("-"),
                &failure.message,
            );
            Err(UiError::from(failure))
        }
    }
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>) -> ConnectionView {
    state.boundary.connection.mark_authentication_lost();
    state.clear_client();
    state.refresh_redaction();
    state.log(Level::Info, "切断", "-", "秘密情報を破棄");
    to_view(&state.boundary.connection.state())
}

// ---- 繰り返しタスク定義 ----

#[tauri::command]
pub fn list_schedules(state: State<'_, AppState>) -> UiResult<Vec<Schedule>> {
    state
        .boundary
        .schedules
        .list()
        .map_err(UiError::storage)
}

#[tauri::command]
pub fn create_schedule(state: State<'_, AppState>, schedule: Schedule) -> UiResult<()> {
    state
        .boundary
        .schedules
        .create(&schedule)
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.log(Level::Info, "定義の作成", &schedule.name, "保存");
    Ok(())
}

#[tauri::command]
pub fn update_schedule(state: State<'_, AppState>, schedule: Schedule) -> UiResult<()> {
    state
        .boundary
        .schedules
        .update(&schedule)
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.log(Level::Info, "定義の更新", &schedule.name, "保存");
    Ok(())
}

#[tauri::command]
pub fn duplicate_schedule(state: State<'_, AppState>, id: String) -> UiResult<Schedule> {
    let new_id = ScheduleId(new_identifier());
    let copy = state
        .boundary
        .schedules
        .duplicate(&ScheduleId(id), new_id)
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.log(Level::Info, "定義の複製", &copy.name, "保存");
    Ok(copy)
}

#[tauri::command]
pub fn set_schedule_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> UiResult<()> {
    state
        .boundary
        .schedules
        .set_enabled(&ScheduleId(id.clone()), enabled)
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.log(
        Level::Info,
        "定義の有効・無効",
        &id,
        if enabled { "有効" } else { "無効" },
    );
    Ok(())
}

#[tauri::command]
pub fn delete_schedule_confirmation(
    state: State<'_, AppState>,
    id: String,
) -> UiResult<String> {
    state
        .boundary
        .schedules
        .delete_confirmation(&ScheduleId(id))
        .map(|c| c.describe())
        .map_err(|e| UiError::from(Failure::from(e)))
}

#[tauri::command]
pub fn delete_schedule(state: State<'_, AppState>, id: String) -> UiResult<()> {
    state
        .boundary
        .schedules
        .delete(&ScheduleId(id.clone()))
        .map_err(|e| UiError::from(Failure::from(e)))?;
    state.log(Level::Info, "定義の削除", &id, "削除");
    Ok(())
}

/// EC-30: パターンの変更で未登録の発生日が生じるかを保存前に調べる。
#[tauri::command]
pub fn pattern_change_confirmation(
    state: State<'_, AppState>,
    schedule: Schedule,
    start: NaiveDate,
    end: NaiveDate,
) -> UiResult<Option<String>> {
    let range = gitlabregister_core::model::DateRange::new(start, end).ok_or_else(|| {
        UiError::input(
            "期間の指定が逆転しています",
            "開始日と終了日を入れ替えてください",
        )
    })?;
    state
        .boundary
        .schedules
        .pattern_change_confirmation(&schedule, &range)
        .map(|c| c.map(|c| c.describe()))
        .map_err(|e| UiError::from(Failure::from(e)))
}

// ---- 展開とプレビュー ----

#[tauri::command]
pub fn resolve_preset(state: State<'_, AppState>, preset: String) -> UiResult<(NaiveDate, NaiveDate)> {
    let preset = match preset.as_str() {
        "this_week" => RangePreset::ThisWeek,
        "this_month" => RangePreset::ThisMonth,
        "next_month" => RangePreset::NextMonth,
        "this_quarter" => RangePreset::ThisQuarter,
        other => {
            return Err(UiError::input(
                format!("知らないプリセットです: {other}"),
                "今週・今月・翌月・今四半期のいずれかを選んでください",
            ))
        }
    };
    // EC-24: 基準日は呼び出し時点の日付に固定する。
    let range = state.boundary.expansion.resolve_preset(preset, today());
    Ok((range.start, range.end))
}

#[tauri::command]
pub fn expand(
    state: State<'_, AppState>,
    start: NaiveDate,
    end: NaiveDate,
) -> UiResult<PreviewView> {
    let preview = state.boundary.expand(start, end).map_err(UiError::from)?;
    let schedules = state
        .boundary
        .schedules
        .list()
        .map_err(UiError::storage)?;
    state.log(
        Level::Info,
        "展開",
        &format!("{start} 〜 {end}"),
        &format!("登録候補 {} 件", preview.candidates.len()),
    );
    Ok(to_preview_view(preview, &schedules))
}

// ---- 登録 ----

/// 一括登録する (FR-09、EC-09)。
///
/// 進捗は `registration-progress` の通知で画面へ送る。
#[tauri::command]
pub async fn register_batch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rows: Vec<CandidateRow>,
) -> UiResult<BatchView> {
    let client = state.client().ok_or_else(UiError::not_connected)?;
    let items: Vec<RegistrationItem> = rows.iter().map(CandidateRow::to_item).collect();
    let cancel = state.new_cancellation();
    let sink = progress_sink(app);

    let outcome = state
        .boundary
        .register_batch(client.as_ref(), &items, now(), &cancel, Some(sink))
        .await
        .map_err(UiError::from)?;
    log_batch(&state, "一括登録", &outcome);
    Ok(to_batch_view(&outcome))
}

/// 失敗分を再実行する (FR-20)。
#[tauri::command]
pub async fn retry_failed(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    rows: Vec<CandidateRow>,
) -> UiResult<BatchView> {
    let client = state.client().ok_or_else(UiError::not_connected)?;
    // 画面で修正された内容を使う。前回の結果は行の対応付けに使う。
    let items: Vec<RegistrationItem> = rows.iter().map(CandidateRow::to_item).collect();
    let previous = BatchOutcome {
        outcomes: items
            .iter()
            .map(|item| gitlabregister_app::ItemOutcome {
                item: item.clone(),
                status: ItemStatus::Failed {
                    kind: gitlabregister_infra::gitlab::ErrorKind::Validation,
                    message: "前回の失敗".into(),
                },
            })
            .collect(),
        aborted: None,
        cancelled: false,
    };
    let cancel = state.new_cancellation();
    let sink = progress_sink(app);
    let outcome = state
        .boundary
        .retry_failed(client.as_ref(), &previous, now(), &cancel, Some(sink))
        .await
        .map_err(UiError::from)?;
    log_batch(&state, "失敗分の再実行", &outcome);
    Ok(to_batch_view(&outcome))
}

/// 単発登録する (FR-12)。
#[tauri::command]
pub async fn register_single(
    state: State<'_, AppState>,
    row: CandidateRow,
) -> UiResult<ResultRow> {
    let client = state.client().ok_or_else(UiError::not_connected)?;
    let item = row.to_item();
    let status = state
        .boundary
        .register_single(client.as_ref(), &item)
        .await
        .map_err(UiError::from)?;
    let outcome = BatchOutcome {
        outcomes: vec![gitlabregister_app::ItemOutcome {
            item,
            status,
        }],
        aborted: None,
        cancelled: false,
    };
    log_batch(&state, "単発登録", &outcome);
    Ok(to_batch_view(&outcome).rows.remove(0))
}

#[tauri::command]
pub fn cancel_registration(state: State<'_, AppState>) {
    state.cancel_running();
    state.log(Level::Warn, "一括登録の中止", "-", "利用者が中止");
}

#[tauri::command]
pub fn is_registering(state: State<'_, AppState>) -> bool {
    state.boundary.is_registering()
}

fn progress_sink(app: tauri::AppHandle) -> gitlabregister_app::registration::ProgressSink {
    Arc::new(move |progress: Progress| {
        // 通知の失敗は無視する。画面が閉じている場合に起こりうる。
        let _ = app.emit(
            "registration-progress",
            serde_json::json!({
                "completed": progress.completed,
                "total": progress.total,
            }),
        );
    })
}

fn log_batch(state: &State<'_, AppState>, operation: &str, outcome: &BatchOutcome) {
    for row in &outcome.outcomes {
        let (level, result) = match &row.status {
            ItemStatus::Succeeded {
                issue_reference, ..
            } => (Level::Info, format!("成功 {issue_reference}")),
            ItemStatus::Failed { message, .. } => (Level::Error, format!("失敗 {message}")),
            ItemStatus::NotAttempted => (Level::Warn, "未処理".to_string()),
        };
        state.log(level, operation, &row.item.describe(), &result);
    }
}

// ---- 登録先プロジェクト ----

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> UiResult<Vec<Project>> {
    state
        .boundary
        .master
        .list_projects()
        .map_err(UiError::storage)
}

#[tauri::command]
pub async fn add_project(state: State<'_, AppState>, project_path: String) -> UiResult<Project> {
    let client = state.client().ok_or_else(UiError::not_connected)?;
    // GitLab-EE 上に存在するかを確かめてから登録する (FR-02)。
    let found = client
        .fetch_project(&project_path)
        .await
        .map_err(|e| UiError::from(Failure::from(e)))?;
    let project = Project {
        id: found.id,
        display_name: found.name,
    };
    state
        .boundary
        .master
        .add_project(&project)
        .map_err(UiError::storage)?;
    state.log(Level::Info, "登録先の追加", &project.display_name, "保存");
    Ok(project)
}

#[tauri::command]
pub fn project_delete_confirmation(
    state: State<'_, AppState>,
    id: String,
) -> UiResult<Option<String>> {
    let confirmation = state
        .boundary
        .master
        .project_delete_confirmation(&id)
        .map_err(UiError::storage)?;
    Ok(if confirmation.needs_confirmation() {
        Some(confirmation.describe())
    } else {
        None
    })
}

#[tauri::command]
pub fn delete_project(state: State<'_, AppState>, id: String) -> UiResult<()> {
    state
        .boundary
        .master
        .delete_project(&id)
        .map_err(UiError::storage)?;
    state.log(Level::Info, "登録先の削除", &id, "削除");
    Ok(())
}

// ---- 休日カレンダー ----

#[tauri::command]
pub fn list_holidays(state: State<'_, AppState>) -> UiResult<Vec<Holiday>> {
    state
        .boundary
        .master
        .list_holidays()
        .map_err(UiError::storage)
}

#[tauri::command]
pub fn add_holiday(state: State<'_, AppState>, date: NaiveDate, name: String) -> UiResult<()> {
    state
        .boundary
        .master
        .add_holiday(&Holiday {
            date,
            name: name.clone(),
        })
        .map_err(|e| UiError::input(e.to_string(), "別の日付を指定してください"))?;
    state.log(Level::Info, "休日の追加", &format!("{date} {name}"), "保存");
    Ok(())
}

#[tauri::command]
pub fn delete_holiday(state: State<'_, AppState>, date: NaiveDate) -> UiResult<()> {
    state
        .boundary
        .master
        .delete_holiday(date)
        .map_err(UiError::storage)?;
    state.log(Level::Info, "休日の削除", &date.to_string(), "削除");
    Ok(())
}

/// 休日を取り込む (FR-22、EC-04)。
#[derive(Debug, Clone, Serialize)]
pub struct HolidayImportView {
    pub added: usize,
    pub malformed_lines: Vec<usize>,
    pub duplicate_lines: Vec<usize>,
}

#[tauri::command]
pub fn import_holidays(state: State<'_, AppState>, path: String) -> UiResult<HolidayImportView> {
    let outcome = state
        .boundary
        .master
        .import_holidays(std::path::Path::new(&path))
        .map_err(UiError::storage)?;
    state.log(
        Level::Info,
        "休日の取り込み",
        &path,
        &format!("{} 件", outcome.added),
    );
    Ok(HolidayImportView {
        added: outcome.added,
        malformed_lines: outcome.malformed_lines,
        duplicate_lines: outcome.duplicate_lines,
    })
}

#[tauri::command]
pub fn export_holidays(state: State<'_, AppState>, path: String) -> UiResult<usize> {
    match state
        .boundary
        .master
        .export_holidays(std::path::Path::new(&path))
        .map_err(UiError::storage)?
    {
        Ok(count) => {
            state.log(Level::Info, "休日の書き出し", &path, &format!("{count} 件"));
            Ok(count)
        }
        Err(_) => Err(UiError::input(
            "休日が 0 件のため書き出しませんでした",
            "休日を登録してから書き出してください",
        )),
    }
}

// ---- 定義の取り込みと書き出し ----

#[tauri::command]
pub fn export_schedules(state: State<'_, AppState>, path: String) -> UiResult<usize> {
    let count = state
        .boundary
        .master
        .export_schedules(std::path::Path::new(&path))
        .map_err(UiError::storage)?;
    state.log(Level::Info, "定義の書き出し", &path, &format!("{count} 件"));
    Ok(count)
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleImportView {
    pub added: usize,
    pub overwritten: usize,
    pub skipped_names: Vec<String>,
}

#[tauri::command]
pub fn import_schedules(
    state: State<'_, AppState>,
    path: String,
    overwrite: bool,
) -> UiResult<ScheduleImportView> {
    let policy = if overwrite {
        ImportConflictPolicy::Overwrite
    } else {
        ImportConflictPolicy::Skip
    };
    let outcome = state
        .boundary
        .master
        .import_schedules(std::path::Path::new(&path), policy)
        .map_err(UiError::storage)?;
    state.log(
        Level::Info,
        "定義の取り込み",
        &path,
        &format!("追加 {} / 上書き {}", outcome.added, outcome.overwritten),
    );
    Ok(ScheduleImportView {
        added: outcome.added,
        overwritten: outcome.overwritten,
        skipped_names: outcome.skipped_names,
    })
}

// ---- 登録履歴 ----

#[tauri::command]
pub fn list_history(
    state: State<'_, AppState>,
    schedule_name: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    project: Option<String>,
) -> UiResult<Vec<HistoryEntry>> {
    state
        .boundary
        .master
        .list_history(&HistoryFilter {
            schedule_name,
            occurrence_from: from,
            occurrence_to: to,
            project,
        })
        .map_err(UiError::storage)
}

#[tauri::command]
pub fn history_delete_confirmation(
    state: State<'_, AppState>,
    keys: Vec<(String, NaiveDate)>,
) -> String {
    let keys: Vec<(ScheduleId, NaiveDate)> = keys
        .into_iter()
        .map(|(id, date)| (ScheduleId(id), date))
        .collect();
    state
        .boundary
        .master
        .history_delete_confirmation(&keys)
        .describe()
}

#[tauri::command]
pub fn delete_history(
    state: State<'_, AppState>,
    keys: Vec<(String, NaiveDate)>,
) -> UiResult<usize> {
    let keys: Vec<(ScheduleId, NaiveDate)> = keys
        .into_iter()
        .map(|(id, date)| (ScheduleId(id), date))
        .collect();
    let removed = state
        .boundary
        .delete_history(&keys)
        .map_err(UiError::from)?;
    state.log(
        Level::Info,
        "履歴の削除",
        &format!("{} 件", keys.len()),
        &format!("{removed} 件削除"),
    );
    Ok(removed)
}

// ---- 候補値 ----

#[derive(Debug, Clone, Serialize)]
pub struct CandidateView {
    pub items: Vec<gitlabregister_infra::gitlab::client::Candidate>,
    /// `just_fetched` / `cached` / `never_fetched`
    pub freshness: String,
    pub description: String,
}

fn to_candidate_view(list: CandidateList) -> CandidateView {
    CandidateView {
        freshness: match list.freshness {
            Freshness::JustFetched { .. } => "just_fetched",
            Freshness::Cached { .. } => "cached",
            Freshness::NeverFetched => "never_fetched",
        }
        .into(),
        description: list.freshness.describe(),
        items: list.items,
    }
}

fn parse_kind(kind: &str) -> UiResult<CandidateKind> {
    Ok(match kind {
        "label" => CandidateKind::Label,
        "member" => CandidateKind::Member,
        "milestone" => CandidateKind::Milestone,
        "epic" => CandidateKind::Epic,
        "iteration" => CandidateKind::Iteration,
        other => {
            return Err(UiError::input(
                format!("知らない候補の種別です: {other}"),
                "ラベル・担当者・マイルストーン・エピック・イテレーションのいずれかを指定してください",
            ))
        }
    })
}

#[tauri::command]
pub async fn candidates(
    state: State<'_, AppState>,
    project: String,
    kind: String,
) -> UiResult<CandidateView> {
    let kind = parse_kind(&kind)?;
    // EC-19: 到達できない状態では保持分を返す。
    let client = match state.client() {
        Some(client) => client,
        None => {
            return Ok(to_candidate_view(
                state
                    .boundary
                    .candidates
                    .cached(&project, kind)
                    .map_err(UiError::storage)?,
            ))
        }
    };
    let list = state
        .boundary
        .candidates(client.as_ref(), &project, kind, now())
        .await
        .map_err(UiError::from)?;
    Ok(to_candidate_view(list))
}

#[tauri::command]
pub async fn refetch_candidates(
    state: State<'_, AppState>,
    project: String,
    kind: String,
) -> UiResult<CandidateView> {
    let kind = parse_kind(&kind)?;
    let client = state.client().ok_or_else(UiError::not_connected)?;
    match state
        .boundary
        .candidates
        .refetch(client.as_ref(), &project, kind, now())
        .await
    {
        Ok(list) => Ok(to_candidate_view(list)),
        Err((fallback, error)) => {
            state.log(Level::Warn, "候補の再取得", &project, &error.message);
            // 取得できなくても保持分を返す (EC-19)。
            Ok(to_candidate_view(fallback))
        }
    }
}

/// 定義の識別子を作る。時刻と乱数を混ぜて重複を避ける。
fn new_identifier() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("s{nanos:x}")
}
