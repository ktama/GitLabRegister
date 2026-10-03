//! 繰り返しタスク定義の管理 (FR-03、FR-14、FR-21、EC-01〜EC-03、EC-06、EC-26、EC-30、EC-36)。

use chrono::NaiveDate;
use gitlabregister_core::business_day::BusinessDayCalendar;
use gitlabregister_core::model::{DateRange, Schedule, ScheduleId};
use gitlabregister_core::validator::{self, Violation};
use gitlabregister_core::{history::HistoryIndex, recurrence};
use gitlabregister_infra::atomic::StoreError;
use gitlabregister_infra::history_store::HistoryStore;
use gitlabregister_infra::store::ConfigStore;

/// 保存を拒否した理由。
#[derive(Debug)]
pub enum SaveError {
    /// 入力の検証に通らなかった (EC-01〜EC-03、EC-06、EC-36、FR-04)。
    Invalid(Vec<Violation>),
    /// 同一名称の定義が既にある (FR-03)。
    DuplicateName(String),
    /// 対象の定義が見つからない。
    NotFound(ScheduleId),
    Store(StoreError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Invalid(violations) => {
                let detail = violations
                    .iter()
                    .map(|v| format!("{}: {}", v.field, v.reason))
                    .collect::<Vec<_>>()
                    .join(" / ");
                write!(f, "入力を確認してください ({detail})")
            }
            SaveError::DuplicateName(name) => {
                write!(f, "同じ名称の定義が既にあります: {name}")
            }
            SaveError::NotFound(id) => write!(f, "定義が見つかりません: {}", id.0),
            SaveError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// 削除の前に示す確認の内容 (EC-26)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteConfirmation {
    pub schedule_name: String,
    /// この定義の登録履歴の件数。削除しても履歴は残る。
    pub history_count: usize,
}

impl DeleteConfirmation {
    pub fn describe(&self) -> String {
        format!(
            "定義「{}」を削除します。この定義の登録履歴 {} 件は削除されず、履歴の一覧から参照できます",
            self.schedule_name, self.history_count
        )
    }
}

/// パターンの変更が未登録の発生日を生む場合の確認 (EC-30)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternChangeConfirmation {
    pub schedule_name: String,
    /// 変更後に新しく登録候補になる発生日。
    pub new_occurrences: Vec<NaiveDate>,
}

impl PatternChangeConfirmation {
    pub fn describe(&self) -> String {
        format!(
            "定義「{}」のパターンを変更すると、未登録の発生日が {} 件生じます (例: {})",
            self.schedule_name,
            self.new_occurrences.len(),
            self.new_occurrences
                .iter()
                .take(3)
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// 定義の管理。
pub struct ScheduleService {
    config: ConfigStore,
    history: HistoryStore,
}

impl ScheduleService {
    pub fn new(config: ConfigStore) -> Self {
        let history = HistoryStore::new(config.paths().history());
        Self { config, history }
    }

    pub fn list(&self) -> Result<Vec<Schedule>, StoreError> {
        self.config.load_schedules()
    }

    /// 新しい定義を保存する (FR-03)。
    pub fn create(&self, schedule: &Schedule) -> Result<(), SaveError> {
        let violations = validator::validate_schedule(schedule);
        if !violations.is_empty() {
            return Err(SaveError::Invalid(violations));
        }
        let mut current = self.config.load_schedules().map_err(SaveError::Store)?;
        if current.iter().any(|s| s.name == schedule.name) {
            return Err(SaveError::DuplicateName(schedule.name.clone()));
        }
        current.push(schedule.clone());
        self.config
            .save_schedules(&current)
            .map_err(SaveError::Store)
    }

    /// 既存の定義を更新する (FR-03)。
    ///
    /// 定義 ID は変えない。名称やパターンを変えても履歴との対応は保たれる (EC-30)。
    pub fn update(&self, schedule: &Schedule) -> Result<(), SaveError> {
        let violations = validator::validate_schedule(schedule);
        if !violations.is_empty() {
            return Err(SaveError::Invalid(violations));
        }
        let mut current = self.config.load_schedules().map_err(SaveError::Store)?;
        let index = current
            .iter()
            .position(|s| s.id == schedule.id)
            .ok_or_else(|| SaveError::NotFound(schedule.id.clone()))?;
        if current
            .iter()
            .enumerate()
            .any(|(i, s)| i != index && s.name == schedule.name)
        {
            return Err(SaveError::DuplicateName(schedule.name.clone()));
        }
        current[index] = schedule.clone();
        self.config
            .save_schedules(&current)
            .map_err(SaveError::Store)
    }

    /// 定義を複製する (FR-03)。
    ///
    /// 名称の末尾に重複しない文字列を付ける。名称以外は複製元と同一にする。
    pub fn duplicate(&self, id: &ScheduleId, new_id: ScheduleId) -> Result<Schedule, SaveError> {
        let current = self.config.load_schedules().map_err(SaveError::Store)?;
        let source = current
            .iter()
            .find(|s| s.id == *id)
            .ok_or_else(|| SaveError::NotFound(id.clone()))?;
        let name = unique_name(&source.name, &current);
        let copy = Schedule {
            id: new_id,
            name,
            ..source.clone()
        };
        let mut next = current;
        next.push(copy.clone());
        self.config
            .save_schedules(&next)
            .map_err(SaveError::Store)?;
        Ok(copy)
    }

    /// 削除の前に示す確認の内容を組み立てる (EC-26)。
    pub fn delete_confirmation(
        &self,
        id: &ScheduleId,
    ) -> Result<DeleteConfirmation, SaveError> {
        let current = self.config.load_schedules().map_err(SaveError::Store)?;
        let target = current
            .iter()
            .find(|s| s.id == *id)
            .ok_or_else(|| SaveError::NotFound(id.clone()))?;
        let loaded = self.history.load().map_err(SaveError::Store)?;
        Ok(DeleteConfirmation {
            schedule_name: target.name.clone(),
            history_count: loaded
                .entries
                .iter()
                .filter(|e| e.schedule_id == *id)
                .count(),
        })
    }

    /// 定義を削除する (FR-03、EC-26)。登録履歴は削除しない。
    pub fn delete(&self, id: &ScheduleId) -> Result<(), SaveError> {
        let mut current = self.config.load_schedules().map_err(SaveError::Store)?;
        let before = current.len();
        current.retain(|s| s.id != *id);
        if current.len() == before {
            return Err(SaveError::NotFound(id.clone()));
        }
        self.config
            .save_schedules(&current)
            .map_err(SaveError::Store)
    }

    /// 有効・無効を切り替える (FR-14)。
    pub fn set_enabled(&self, id: &ScheduleId, enabled: bool) -> Result<(), SaveError> {
        let mut current = self.config.load_schedules().map_err(SaveError::Store)?;
        let target = current
            .iter_mut()
            .find(|s| s.id == *id)
            .ok_or_else(|| SaveError::NotFound(id.clone()))?;
        target.enabled = enabled;
        self.config
            .save_schedules(&current)
            .map_err(SaveError::Store)
    }

    /// パターンや起算日の変更で未登録の発生日が生じるかを調べる (EC-30)。
    ///
    /// 変更後のパターンを渡す。履歴にない発生日が現れる場合に確認の内容を返す。
    pub fn pattern_change_confirmation(
        &self,
        changed: &Schedule,
        range: &DateRange,
    ) -> Result<Option<PatternChangeConfirmation>, SaveError> {
        let holidays = self.config.load_holidays().map_err(SaveError::Store)?;
        let calendar = BusinessDayCalendar::new(&holidays);
        let loaded = self.history.load().map_err(SaveError::Store)?;
        let index = HistoryIndex::build(&loaded.entries);

        let raw = recurrence::occurrences(
            &changed.pattern,
            &changed.active_period,
            range,
            &calendar,
        );
        let mut new_occurrences = Vec::new();
        for original in raw {
            if let Some(date) = calendar.adjust(original, changed.adjustment) {
                if !index.is_registered(&changed.id, date) && !new_occurrences.contains(&date) {
                    new_occurrences.push(date);
                }
            }
        }
        if new_occurrences.is_empty() {
            return Ok(None);
        }
        Ok(Some(PatternChangeConfirmation {
            schedule_name: changed.name.clone(),
            new_occurrences,
        }))
    }
}

/// 名称の末尾に重複しない文字列を付ける。
fn unique_name(base: &str, existing: &[Schedule]) -> String {
    let candidate = format!("{base} のコピー");
    if !existing.iter().any(|s| s.name == candidate) {
        return candidate;
    }
    for n in 2..=1000 {
        let numbered = format!("{base} のコピー {n}");
        if !existing.iter().any(|s| s.name == numbered) {
            return numbered;
        }
    }
    format!("{base} のコピー {}", existing.len() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitlabregister_core::model::{
        ActivePeriod, BusinessDayAdjustment, HistoryEntry, IssueTemplate, MissingDayPolicy,
        MonthlyDay, RecurrencePattern,
    };
    use gitlabregister_infra::paths::AppPaths;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        config: ConfigStore,
        service: ScheduleService,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let root = dir.path().join("GitLabRegister");
        Fixture {
            _dir: dir,
            config: ConfigStore::new(AppPaths::with_root(root.clone())),
            service: ScheduleService::new(ConfigStore::new(AppPaths::with_root(root))),
        }
    }

    fn schedule(id: &str, name: &str, day: u8) -> Schedule {
        Schedule {
            id: ScheduleId(id.into()),
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
                ..IssueTemplate::default()
            },
        }
    }

    /// FR-03: 定義が 0 件の状態から 1 件保存できる。
    #[test]
    fn schedule_create_from_empty() {
        let f = fixture();
        assert!(f.service.list().expect("読める").is_empty());
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        assert_eq!(f.service.list().expect("読める").len(), 1);
    }

    /// FR-03: 同一名称の定義は保存できない。
    #[test]
    fn schedule_create_rejects_duplicate_name() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let err = f
            .service
            .create(&schedule("s2", "月次報告", 10))
            .expect_err("拒否される");
        assert!(matches!(err, SaveError::DuplicateName(_)), "{err}");
        assert_eq!(f.service.list().expect("読める").len(), 1);
    }

    /// EC-01: 名称やタイトルが空なら保存できず、未入力の項目名が返る。
    #[test]
    fn schedule_create_rejects_empty_fields() {
        let f = fixture();
        let mut s = schedule("s1", "   ", 25);
        s.issue.title = String::new();
        let err = f.service.create(&s).expect_err("拒否される");
        match err {
            SaveError::Invalid(violations) => {
                let fields: Vec<&str> = violations.iter().map(|v| v.field).collect();
                assert!(fields.contains(&"名称"), "{fields:?}");
                assert!(fields.contains(&"タイトル"), "{fields:?}");
            }
            other => panic!("検証エラーとして扱われていない: {other}"),
        }
    }

    /// EC-02: タイトルが 256 文字なら拒否し、現在の文字数を示す。
    #[test]
    fn schedule_create_rejects_long_title() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 25);
        s.issue.title = "あ".repeat(256);
        let err = f.service.create(&s).expect_err("拒否される");
        assert!(err.to_string().contains("256"), "{err}");
    }

    /// EC-36: 開始日が期限より後なら拒否する。
    #[test]
    fn schedule_create_rejects_start_after_due() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 25);
        s.issue.start_offset_days = Some(10);
        s.issue.due_offset_days = Some(7);
        let err = f.service.create(&s).expect_err("拒否される");
        assert!(matches!(err, SaveError::Invalid(_)), "{err}");
    }

    /// FR-03: 内容を変更して保存できる。定義 ID は変わらない。
    #[test]
    fn schedule_update_keeps_id() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let mut changed = schedule("s1", "月次報告 (改訂)", 10);
        changed.issue.title = "改訂後".into();
        f.service.update(&changed).expect("更新できる");

        let loaded = f.service.list().expect("読める");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, ScheduleId("s1".into()));
        assert_eq!(loaded[0].name, "月次報告 (改訂)");
        assert_eq!(loaded[0].issue.title, "改訂後");
    }

    /// FR-03: 他の定義と同じ名称に変更することはできない。
    #[test]
    fn schedule_update_rejects_name_taken_by_other() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        f.service
            .create(&schedule("s2", "棚卸し", 1))
            .expect("保存できる");
        let err = f
            .service
            .update(&schedule("s2", "月次報告", 1))
            .expect_err("拒否される");
        assert!(matches!(err, SaveError::DuplicateName(_)), "{err}");
    }

    /// 自分自身の名称のままの更新は通る。
    #[test]
    fn schedule_update_allows_same_name() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        f.service
            .update(&schedule("s1", "月次報告", 10))
            .expect("更新できる");
    }

    /// FR-03: 存在しない定義の更新は拒否する。
    #[test]
    fn schedule_update_rejects_missing() {
        let f = fixture();
        let err = f
            .service
            .update(&schedule("none", "ない定義", 1))
            .expect_err("拒否される");
        assert!(matches!(err, SaveError::NotFound(_)), "{err}");
    }

    /// FR-03: 複製すると名称だけが変わり、他の項目は同一になる。
    #[test]
    fn schedule_duplicate_changes_only_name() {
        let f = fixture();
        let source = schedule("s1", "月次報告", 25);
        f.service.create(&source).expect("保存できる");
        let copy = f
            .service
            .duplicate(&ScheduleId("s1".into()), ScheduleId("s2".into()))
            .expect("複製できる");

        assert_ne!(copy.name, source.name);
        assert!(copy.name.starts_with("月次報告"), "{}", copy.name);
        assert_eq!(copy.project, source.project);
        assert_eq!(copy.pattern, source.pattern);
        assert_eq!(copy.issue, source.issue);
        assert_eq!(f.service.list().expect("読める").len(), 2);
    }

    /// FR-03: 複製を繰り返しても名称が重複しない。
    #[test]
    fn schedule_duplicate_twice_has_unique_names() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let first = f
            .service
            .duplicate(&ScheduleId("s1".into()), ScheduleId("s2".into()))
            .expect("複製できる");
        let second = f
            .service
            .duplicate(&ScheduleId("s1".into()), ScheduleId("s3".into()))
            .expect("複製できる");
        assert_ne!(first.name, second.name);
        let names: Vec<String> = f
            .service
            .list()
            .expect("読める")
            .into_iter()
            .map(|s| s.name)
            .collect();
        let unique: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(names.len(), unique.len(), "名称が重複している: {names:?}");
    }

    /// EC-26: 削除の前に履歴の件数を示す。
    #[test]
    fn schedule_delete_confirmation_shows_history_count() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let store = HistoryStore::new(f.config.paths().history());
        for day in [5u32, 12] {
            store
                .append(&HistoryEntry {
                    schedule_id: ScheduleId("s1".into()),
                    schedule_name: "月次報告".into(),
                    occurrence_date: d(2026, 10, day),
                    registered_at: d(2026, 10, day).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                    project: "group/alpha".into(),
                    issue_reference: format!("#{day}"),
                })
                .expect("追記できる");
        }

        let confirmation = f
            .service
            .delete_confirmation(&ScheduleId("s1".into()))
            .expect("取得できる");
        assert_eq!(confirmation.history_count, 2);
        assert!(confirmation.describe().contains("2 件"), "{}", confirmation.describe());
        assert!(
            confirmation.describe().contains("削除されず"),
            "履歴が残ることが伝わらない"
        );
    }

    /// EC-26: 削除しても登録履歴は残る。
    #[test]
    fn schedule_delete_keeps_history() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 5),
                registered_at: d(2026, 10, 5).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");

        f.service.delete(&ScheduleId("s1".into())).expect("削除できる");
        assert!(f.service.list().expect("読める").is_empty());
        let history = store.load().expect("読める");
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].schedule_name, "月次報告");
    }

    /// FR-14: 有効・無効を切り替えられ、一覧で判別できる。
    #[test]
    fn schedule_set_enabled_toggles() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        f.service
            .set_enabled(&ScheduleId("s1".into()), false)
            .expect("切り替えられる");
        assert!(!f.service.list().expect("読める")[0].enabled);

        f.service
            .set_enabled(&ScheduleId("s1".into()), true)
            .expect("切り替えられる");
        assert!(f.service.list().expect("読める")[0].enabled);
    }

    /// FR-14: 無効化しても履歴は残る。
    #[test]
    fn schedule_disable_keeps_history() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 5),
                registered_at: d(2026, 10, 5).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");
        f.service
            .set_enabled(&ScheduleId("s1".into()), false)
            .expect("切り替えられる");
        assert_eq!(store.load().expect("読める").entries.len(), 1);
    }

    /// FR-21: 有効期間の終了日が開始日より前なら保存できない。
    #[test]
    fn schedule_rejects_reversed_active_period() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 25);
        s.active_period = ActivePeriod {
            start: Some(d(2026, 9, 30)),
            end: Some(d(2026, 4, 1)),
        };
        let err = f.service.create(&s).expect_err("拒否される");
        assert!(matches!(err, SaveError::Invalid(_)), "{err}");
    }

    /// FR-04: 2 週ごとで起算日を入力しないと保存できない。
    #[test]
    fn schedule_rejects_biweekly_without_anchor() {
        use gitlabregister_core::model::WeekdayKey;
        use std::collections::BTreeSet;
        let f = fixture();
        let mut s = schedule("s1", "隔週の確認", 1);
        s.pattern = RecurrencePattern::Weekly {
            interval: 2,
            weekdays: BTreeSet::from([WeekdayKey::Tue]),
            anchor: None,
        };
        let err = f.service.create(&s).expect_err("拒否される");
        assert!(err.to_string().contains("起算日"), "{err}");
    }

    /// EC-30: パターンの変更で未登録の発生日が生じることを保存前に示す。
    #[test]
    fn schedule_pattern_change_confirmation_lists_new_occurrences() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        // 25 日の分だけ登録済みにする。
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 25),
                registered_at: d(2026, 10, 25).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");

        let range = DateRange::new(d(2026, 10, 1), d(2026, 10, 31)).expect("有効な期間");
        // 10 日へ変更すると 2026-10-10 が未登録として現れる。
        let confirmation = f
            .service
            .pattern_change_confirmation(&schedule("s1", "月次報告", 10), &range)
            .expect("取得できる")
            .expect("確認が必要");
        assert_eq!(confirmation.new_occurrences, vec![d(2026, 10, 10)]);
        assert!(confirmation.describe().contains("2026-10-10"), "{}", confirmation.describe());
    }

    /// EC-30: 変更しても未登録の発生日が生じなければ確認は不要。
    #[test]
    fn schedule_pattern_change_no_confirmation_when_all_registered() {
        let f = fixture();
        f.service
            .create(&schedule("s1", "月次報告", 25))
            .expect("保存できる");
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 25),
                registered_at: d(2026, 10, 25).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");

        let range = DateRange::new(d(2026, 10, 1), d(2026, 10, 31)).expect("有効な期間");
        let confirmation = f
            .service
            .pattern_change_confirmation(&schedule("s1", "月次報告", 25), &range)
            .expect("取得できる");
        assert!(confirmation.is_none(), "{confirmation:?}");
    }
}
