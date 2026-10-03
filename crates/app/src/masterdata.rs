//! 登録先プロジェクト・休日カレンダー・登録履歴の管理と、取り込み・書き出し
//! (FR-02、FR-13、FR-18、FR-19、FR-22、FR-23、EC-04、EC-27)。

use chrono::NaiveDate;
use gitlabregister_core::model::{Holiday, HistoryEntry, ScheduleId};
use gitlabregister_infra::atomic::StoreError;
use gitlabregister_infra::history_store::HistoryStore;
use gitlabregister_infra::store::{
    ConfigStore, ExportRefusal, HolidayImportOutcome, ImportConflictPolicy, Project,
    ScheduleImportOutcome,
};
use std::path::Path;

/// プロジェクトの削除で示す確認の内容 (FR-02)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDeleteConfirmation {
    pub project: String,
    /// このプロジェクトを登録先にしている定義の件数。
    pub referencing_schedules: usize,
}

impl ProjectDeleteConfirmation {
    /// 確認なしで削除してよいか。参照している定義がなければ確認は不要。
    pub fn needs_confirmation(&self) -> bool {
        self.referencing_schedules > 0
    }

    pub fn describe(&self) -> String {
        format!(
            "プロジェクト {} を削除します。このプロジェクトを登録先にしている定義が {} 件あります",
            self.project, self.referencing_schedules
        )
    }
}

/// 履歴の削除で示す確認の内容 (EC-27)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDeleteConfirmation {
    pub count: usize,
}

impl HistoryDeleteConfirmation {
    pub fn describe(&self) -> String {
        format!(
            "登録履歴 {} 件を削除します。削除した発生日は次回の展開で再び登録候補になります。GitLab-EE 上の Issue は削除されません",
            self.count
        )
    }
}

/// 履歴の絞り込み条件 (FR-19)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    pub schedule_name: Option<String>,
    pub occurrence_from: Option<NaiveDate>,
    pub occurrence_to: Option<NaiveDate>,
    pub project: Option<String>,
}

impl HistoryFilter {
    fn matches(&self, entry: &HistoryEntry) -> bool {
        if let Some(name) = &self.schedule_name {
            if entry.schedule_name != *name {
                return false;
            }
        }
        if let Some(from) = self.occurrence_from {
            if entry.occurrence_date < from {
                return false;
            }
        }
        if let Some(to) = self.occurrence_to {
            if entry.occurrence_date > to {
                return false;
            }
        }
        if let Some(project) = &self.project {
            if entry.project != *project {
                return false;
            }
        }
        true
    }
}

/// 登録先・休日・履歴の管理。
pub struct MasterDataService {
    config: ConfigStore,
    history: HistoryStore,
}

impl MasterDataService {
    pub fn new(config: ConfigStore) -> Self {
        let history = HistoryStore::new(config.paths().history());
        Self { config, history }
    }

    // ---- 登録先プロジェクト (FR-02) ----

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        self.config.load_projects()
    }

    /// 登録先を追加する。同じ識別子があれば表示名を更新する (FR-02)。
    pub fn add_project(&self, project: &Project) -> Result<(), StoreError> {
        let mut current = self.config.load_projects()?;
        match current.iter_mut().find(|p| p.id == project.id) {
            Some(existing) => existing.display_name = project.display_name.clone(),
            None => current.push(project.clone()),
        }
        self.config.save_projects(&current)
    }

    /// 削除の前に、参照している定義の件数を調べる (FR-02)。
    pub fn project_delete_confirmation(
        &self,
        project_id: &str,
    ) -> Result<ProjectDeleteConfirmation, StoreError> {
        let projects = self.config.load_projects()?;
        let name = projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| project_id.to_string());
        let schedules = self.config.load_schedules()?;
        Ok(ProjectDeleteConfirmation {
            project: name.clone(),
            referencing_schedules: schedules
                .iter()
                .filter(|s| s.project == project_id || s.project == name)
                .count(),
        })
    }

    /// 登録先を削除する (FR-02)。定義は変更しない。
    pub fn delete_project(&self, project_id: &str) -> Result<(), StoreError> {
        let mut current = self.config.load_projects()?;
        current.retain(|p| p.id != project_id);
        self.config.save_projects(&current)
    }

    // ---- 休日カレンダー (FR-13、FR-22、EC-04) ----

    pub fn list_holidays(&self) -> Result<Vec<Holiday>, StoreError> {
        self.config.load_holidays()
    }

    /// 休日を 1 件登録する。同一日付は拒否する (FR-13、EC-04)。
    pub fn add_holiday(&self, holiday: &Holiday) -> Result<(), HolidayError> {
        let mut current = self.config.load_holidays().map_err(HolidayError::Store)?;
        if current.iter().any(|h| h.date == holiday.date) {
            return Err(HolidayError::DuplicateDate(holiday.date));
        }
        current.push(holiday.clone());
        current.sort_by_key(|h| h.date);
        self.config
            .save_holidays(&current)
            .map_err(HolidayError::Store)
    }

    /// 休日を削除する (FR-13)。
    pub fn delete_holiday(&self, date: NaiveDate) -> Result<(), StoreError> {
        let mut current = self.config.load_holidays()?;
        current.retain(|h| h.date != date);
        self.config.save_holidays(&current)
    }

    /// 休日を取り込む (FR-22、EC-04)。
    pub fn import_holidays(&self, source: &Path) -> Result<HolidayImportOutcome, StoreError> {
        self.config.import_holidays(source)
    }

    /// 休日を書き出す。0 件なら実行しない (FR-22)。
    pub fn export_holidays(
        &self,
        destination: &Path,
    ) -> Result<Result<usize, ExportRefusal>, StoreError> {
        let holidays = self.config.load_holidays()?;
        self.config.export_holidays(&holidays, destination)
    }

    // ---- 定義の取り込みと書き出し (FR-18) ----

    pub fn export_schedules(&self, destination: &Path) -> Result<usize, StoreError> {
        let schedules = self.config.load_schedules()?;
        self.config.export_schedules(&schedules, destination)?;
        Ok(schedules.len())
    }

    pub fn import_schedules(
        &self,
        source: &Path,
        policy: ImportConflictPolicy,
    ) -> Result<ScheduleImportOutcome, StoreError> {
        self.config.import_schedules(source, policy)
    }

    // ---- 登録履歴 (FR-19、FR-23、EC-27) ----

    /// 履歴を絞り込んで返す。登録日時の降順で並べる (FR-19)。
    pub fn list_history(
        &self,
        filter: &HistoryFilter,
    ) -> Result<Vec<HistoryEntry>, StoreError> {
        let loaded = self.history.load()?;
        let mut entries: Vec<HistoryEntry> = loaded
            .entries
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.registered_at));
        Ok(entries)
    }

    /// 削除の前に示す確認の内容 (EC-27)。
    pub fn history_delete_confirmation(
        &self,
        keys: &[(ScheduleId, NaiveDate)],
    ) -> HistoryDeleteConfirmation {
        HistoryDeleteConfirmation { count: keys.len() }
    }

    /// 履歴を削除する (FR-23、EC-27)。GitLab-EE 上の Issue は削除しない。
    pub fn delete_history(
        &self,
        keys: &[(ScheduleId, NaiveDate)],
    ) -> Result<usize, StoreError> {
        self.history.remove(keys)
    }

    /// 履歴に解釈できない行があればその行番号を返す。
    pub fn malformed_history_lines(&self) -> Result<Vec<usize>, StoreError> {
        Ok(self.history.load()?.malformed_lines)
    }
}

/// 休日の登録を拒否した理由。
#[derive(Debug)]
pub enum HolidayError {
    /// 同一日付が既に登録されている (EC-04)。
    DuplicateDate(NaiveDate),
    Store(StoreError),
}

impl std::fmt::Display for HolidayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HolidayError::DuplicateDate(date) => {
                write!(f, "{date} は既に休日として登録されています")
            }
            HolidayError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HolidayError {}

#[cfg(test)]
mod tests {
    use super::*;
    use gitlabregister_core::model::{
        ActivePeriod, BusinessDayAdjustment, IssueTemplate, MissingDayPolicy, MonthlyDay,
        RecurrencePattern, Schedule,
    };
    use gitlabregister_infra::paths::AppPaths;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        dir_path: std::path::PathBuf,
        config: ConfigStore,
        service: MasterDataService,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let dir_path = dir.path().to_path_buf();
        let root = dir_path.join("GitLabRegister");
        Fixture {
            _dir: dir,
            dir_path,
            config: ConfigStore::new(AppPaths::with_root(root.clone())),
            service: MasterDataService::new(ConfigStore::new(AppPaths::with_root(root))),
        }
    }

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            display_name: name.into(),
        }
    }

    fn schedule(id: &str, name: &str, project: &str) -> Schedule {
        Schedule {
            id: ScheduleId(id.into()),
            name: name.into(),
            project: project.into(),
            enabled: true,
            active_period: ActivePeriod::default(),
            pattern: RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(25),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            adjustment: BusinessDayAdjustment::None,
            issue: IssueTemplate {
                title: name.into(),
                ..IssueTemplate::default()
            },
        }
    }

    fn entry(schedule: &str, project: &str, day: u32) -> HistoryEntry {
        HistoryEntry {
            schedule_id: ScheduleId(format!("id-{schedule}")),
            schedule_name: schedule.into(),
            occurrence_date: d(2026, 10, day),
            registered_at: d(2026, 10, day).and_hms_opt(9, 0, 0).expect("有効な時刻"),
            project: project.into(),
            issue_reference: format!("#{day}"),
        }
    }

    // ---- FR-02 登録先プロジェクト ----

    /// FR-02: プロジェクトを登録すると一覧に出る。
    #[test]
    fn masterdata_add_project() {
        let f = fixture();
        assert!(f.service.list_projects().expect("読める").is_empty());
        f.service
            .add_project(&project("11", "group/alpha"))
            .expect("登録できる");
        assert_eq!(f.service.list_projects().expect("読める").len(), 1);
    }

    /// FR-02: 同じ識別子で登録すると表示名を更新する。重複して増えない。
    #[test]
    fn masterdata_add_project_updates_existing() {
        let f = fixture();
        f.service
            .add_project(&project("11", "group/alpha"))
            .expect("登録できる");
        f.service
            .add_project(&project("11", "group/alpha-renamed"))
            .expect("登録できる");
        let loaded = f.service.list_projects().expect("読める");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].display_name, "group/alpha-renamed");
    }

    /// FR-02: 削除の前に参照している定義の件数を示す。
    #[test]
    fn masterdata_project_delete_confirmation_counts_schedules() {
        let f = fixture();
        f.service
            .add_project(&project("11", "group/alpha"))
            .expect("登録できる");
        f.config
            .save_schedules(&[
                schedule("s1", "月次報告", "group/alpha"),
                schedule("s2", "棚卸し", "group/alpha"),
                schedule("s3", "別件", "group/beta"),
            ])
            .expect("書ける");

        let confirmation = f
            .service
            .project_delete_confirmation("11")
            .expect("取得できる");
        assert_eq!(confirmation.referencing_schedules, 2);
        assert!(confirmation.needs_confirmation());
        assert!(confirmation.describe().contains("2 件"), "{}", confirmation.describe());
    }

    /// FR-02: 参照している定義がなければ確認は不要。
    #[test]
    fn masterdata_project_delete_without_references() {
        let f = fixture();
        f.service
            .add_project(&project("11", "group/alpha"))
            .expect("登録できる");
        let confirmation = f
            .service
            .project_delete_confirmation("11")
            .expect("取得できる");
        assert!(!confirmation.needs_confirmation());
        f.service.delete_project("11").expect("削除できる");
        assert!(f.service.list_projects().expect("読める").is_empty());
    }

    /// FR-02: 削除しても定義は変更しない。
    #[test]
    fn masterdata_delete_project_keeps_schedules() {
        let f = fixture();
        f.service
            .add_project(&project("11", "group/alpha"))
            .expect("登録できる");
        f.config
            .save_schedules(&[schedule("s1", "月次報告", "group/alpha")])
            .expect("書ける");
        f.service.delete_project("11").expect("削除できる");
        assert_eq!(f.config.load_schedules().expect("読める").len(), 1);
    }

    // ---- FR-13 休日カレンダー ----

    /// FR-13: 休日を名称付きで登録できる。
    #[test]
    fn masterdata_add_holiday() {
        let f = fixture();
        f.service
            .add_holiday(&Holiday {
                date: d(2026, 11, 3),
                name: "文化の日".into(),
            })
            .expect("登録できる");
        let loaded = f.service.list_holidays().expect("読める");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "文化の日");
    }

    /// EC-04: 同じ日付を再度登録すると拒否し、登録済みであることを示す。
    #[test]
    fn masterdata_add_holiday_rejects_duplicate() {
        let f = fixture();
        let holiday = Holiday {
            date: d(2026, 11, 3),
            name: "文化の日".into(),
        };
        f.service.add_holiday(&holiday).expect("登録できる");
        let err = f.service.add_holiday(&holiday).expect_err("拒否される");
        assert!(matches!(err, HolidayError::DuplicateDate(_)), "{err}");
        assert!(err.to_string().contains("2026-11-03"), "{err}");
        assert_eq!(f.service.list_holidays().expect("読める").len(), 1);
    }

    /// FR-13: 休日を削除できる。
    #[test]
    fn masterdata_delete_holiday() {
        let f = fixture();
        f.service
            .add_holiday(&Holiday {
                date: d(2026, 11, 3),
                name: "文化の日".into(),
            })
            .expect("登録できる");
        f.service.delete_holiday(d(2026, 11, 3)).expect("削除できる");
        assert!(f.service.list_holidays().expect("読める").is_empty());
    }

    /// FR-13: 登録した休日は日付の昇順で並ぶ。
    #[test]
    fn masterdata_holidays_are_sorted() {
        let f = fixture();
        for day in [31u32, 1, 15] {
            f.service
                .add_holiday(&Holiday {
                    date: d(2026, 12, day),
                    name: format!("休業日{day}"),
                })
                .expect("登録できる");
        }
        let dates: Vec<NaiveDate> = f
            .service
            .list_holidays()
            .expect("読める")
            .into_iter()
            .map(|h| h.date)
            .collect();
        assert_eq!(dates, vec![d(2026, 12, 1), d(2026, 12, 15), d(2026, 12, 31)]);
    }

    /// FR-22: 休日を書き出して取り込める。
    #[test]
    fn masterdata_holiday_export_import() {
        let f = fixture();
        f.service
            .add_holiday(&Holiday {
                date: d(2026, 11, 3),
                name: "文化の日".into(),
            })
            .expect("登録できる");
        let file = f.dir_path.join("holidays.txt");
        let written = f
            .service
            .export_holidays(&file)
            .expect("書き出せる")
            .expect("0 件ではない");
        assert_eq!(written, 1);

        f.service.delete_holiday(d(2026, 11, 3)).expect("削除できる");
        let outcome = f.service.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 1);
        assert_eq!(f.service.list_holidays().expect("読める")[0].name, "文化の日");
    }

    /// FR-22: 0 件では書き出しを実行しない。
    #[test]
    fn masterdata_holiday_export_refuses_when_empty() {
        let f = fixture();
        let file = f.dir_path.join("holidays.txt");
        let result = f.service.export_holidays(&file).expect("失敗しない");
        assert_eq!(result, Err(ExportRefusal::Empty));
        assert!(!file.exists());
    }

    // ---- FR-18 定義の取り込みと書き出し ----

    /// FR-18: 定義を書き出して取り込める。
    #[test]
    fn masterdata_schedule_export_import() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", "group/alpha")])
            .expect("書ける");
        let file = f.dir_path.join("schedules.json");
        assert_eq!(f.service.export_schedules(&file).expect("書き出せる"), 1);

        f.config.save_schedules(&[]).expect("書ける");
        let outcome = f
            .service
            .import_schedules(&file, ImportConflictPolicy::Skip)
            .expect("取り込める");
        assert_eq!(outcome.added, 1);
    }

    // ---- FR-19 履歴の閲覧と絞り込み ----

    /// FR-19: 定義名で絞り込める。
    #[test]
    fn masterdata_history_filter_by_schedule_name() {
        let f = fixture();
        let store = HistoryStore::new(f.config.paths().history());
        store.append(&entry("月次報告", "group/alpha", 5)).expect("追記");
        store.append(&entry("棚卸し", "group/alpha", 6)).expect("追記");

        let got = f
            .service
            .list_history(&HistoryFilter {
                schedule_name: Some("月次報告".into()),
                ..HistoryFilter::default()
            })
            .expect("読める");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].schedule_name, "月次報告");
    }

    /// FR-19: 発生日の範囲で絞り込める。
    #[test]
    fn masterdata_history_filter_by_date_range() {
        let f = fixture();
        let store = HistoryStore::new(f.config.paths().history());
        for day in [1u32, 15, 31] {
            store.append(&entry("月次報告", "group/alpha", day)).expect("追記");
        }
        let got = f
            .service
            .list_history(&HistoryFilter {
                occurrence_from: Some(d(2026, 10, 10)),
                occurrence_to: Some(d(2026, 10, 20)),
                ..HistoryFilter::default()
            })
            .expect("読める");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].occurrence_date, d(2026, 10, 15));
    }

    /// FR-19: 登録先プロジェクトで絞り込める。
    #[test]
    fn masterdata_history_filter_by_project() {
        let f = fixture();
        let store = HistoryStore::new(f.config.paths().history());
        store.append(&entry("月次報告", "group/alpha", 5)).expect("追記");
        store.append(&entry("月次報告", "group/beta", 6)).expect("追記");
        let got = f
            .service
            .list_history(&HistoryFilter {
                project: Some("group/beta".into()),
                ..HistoryFilter::default()
            })
            .expect("読める");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].project, "group/beta");
    }

    /// FR-19: 絞り込まなければ全件が登録日時の降順で返る。
    #[test]
    fn masterdata_history_without_filter_is_newest_first() {
        let f = fixture();
        let store = HistoryStore::new(f.config.paths().history());
        for day in [5u32, 12, 19] {
            store.append(&entry("月次報告", "group/alpha", day)).expect("追記");
        }
        let got = f
            .service
            .list_history(&HistoryFilter::default())
            .expect("読める");
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].occurrence_date, d(2026, 10, 19), "降順でない");
    }

    /// FR-19: 履歴が 0 件でも失敗しない。
    #[test]
    fn masterdata_history_empty() {
        let f = fixture();
        assert!(f
            .service
            .list_history(&HistoryFilter::default())
            .expect("読める")
            .is_empty());
    }

    // ---- FR-23 / EC-27 履歴の削除 ----

    /// EC-27: 削除の前に件数と、再び候補になることを示す。
    #[test]
    fn masterdata_history_delete_confirmation() {
        let f = fixture();
        let keys = vec![
            (ScheduleId("id-月次報告".into()), d(2026, 10, 5)),
            (ScheduleId("id-月次報告".into()), d(2026, 10, 12)),
        ];
        let confirmation = f.service.history_delete_confirmation(&keys);
        assert_eq!(confirmation.count, 2);
        let text = confirmation.describe();
        assert!(text.contains("2 件"), "{text}");
        assert!(text.contains("登録候補"), "{text}");
        assert!(text.contains("削除されません"), "GitLab 側が残ることが伝わらない: {text}");
    }

    /// FR-23: 選択した記録のみを削除する。
    #[test]
    fn masterdata_delete_history_selected_only() {
        let f = fixture();
        let store = HistoryStore::new(f.config.paths().history());
        for day in [5u32, 12, 19] {
            store.append(&entry("月次報告", "group/alpha", day)).expect("追記");
        }
        let removed = f
            .service
            .delete_history(&[(ScheduleId("id-月次報告".into()), d(2026, 10, 12))])
            .expect("削除できる");
        assert_eq!(removed, 1);
        let remaining: Vec<NaiveDate> = f
            .service
            .list_history(&HistoryFilter::default())
            .expect("読める")
            .into_iter()
            .map(|e| e.occurrence_date)
            .collect();
        assert_eq!(remaining, vec![d(2026, 10, 19), d(2026, 10, 5)]);
    }

    /// 履歴の破損行を報告できる。
    #[test]
    fn masterdata_reports_malformed_history_lines() {
        let f = fixture();
        std::fs::create_dir_all(f.config.paths().root()).expect("作れる");
        std::fs::write(f.config.paths().history(), "{ broken\n").expect("書ける");
        assert_eq!(
            f.service.malformed_history_lines().expect("読める"),
            vec![1]
        );
    }
}
