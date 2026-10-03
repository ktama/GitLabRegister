//! 設定・プロジェクト・定義・休日の読み書きと、取り込み・書き出し
//! (FR-02、FR-03、FR-13、FR-18、FR-22、EC-04、EC-07、EC-08、EC-28)。

use crate::atomic::{load_document, save_document, StoreError};
use crate::paths::AppPaths;
use chrono::NaiveDate;
use gitlabregister_core::model::{Holiday, Schedule};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// プロキシの使い方 (NFR-17)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyMode {
    /// OS に設定されたプロキシ構成に従う (既定)。
    #[default]
    FollowSystem,
    /// アプリの設定で指定したプロキシを使う。
    Manual,
    /// プロキシを経由しない。
    Direct,
}

/// プロキシの宛先。利用者名とパスワードは保存しない (NFR-05、NFR-17)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    pub host: Option<String>,
    pub port: Option<u16>,
    /// プロキシが利用者名とパスワードを要求するか。要求する場合は起動のたびに入力を求める。
    pub requires_credentials: bool,
}

/// アプリの設定 (設計書 5.1 AppSettings)。
///
/// PAT とプロキシ資格情報は含めない。これらは SecretHolder だけが持つ (NFR-05)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    /// 接続先 GitLab-EE の URL。次回起動時に初期表示する (FR-01)。
    pub gitlab_url: Option<String>,
    pub proxy: ProxySettings,
}

/// 登録先プロジェクト (設計書 5.1 Project)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// GitLab-EE 上の識別子。
    pub id: String,
    pub display_name: String,
}

/// 保持した候補値 1 組 (設計書 5.1 CandidateCache)。
///
/// 取得日時を併記する。到達できない状態で「最新でない可能性」を画面に示すため (EC-19、FR-16)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateCacheEntry {
    pub project: String,
    pub kind: crate::gitlab::client::CandidateKind,
    pub items: Vec<crate::gitlab::client::Candidate>,
    pub fetched_at: chrono::NaiveDateTime,
}

/// 設定フォルダ配下のファイル群を読み書きする。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    paths: AppPaths,
}

/// 取り込み時に同一名称の定義があったときの扱い (FR-18)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportConflictPolicy {
    /// 既存の定義を置き換える。
    Overwrite,
    /// 取り込まない。
    Skip,
}

/// 定義の取り込み結果 (FR-18)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleImportOutcome {
    /// 新しく追加した件数。
    pub added: usize,
    /// 既存を置き換えた件数。
    pub overwritten: usize,
    /// 取り込まなかった定義名。
    pub skipped_names: Vec<String>,
}

/// 休日の取り込み結果 (FR-22、EC-04)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HolidayImportOutcome {
    pub added: usize,
    /// 日付として解釈できなかった行の行番号 (1 始まり)。
    pub malformed_lines: Vec<usize>,
    /// 同一日付が重複していた行の行番号 (1 始まり)。最初の 1 件のみを登録する (EC-04)。
    pub duplicate_lines: Vec<usize>,
}

/// 書き出しを実行しなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportRefusal {
    /// 対象が 0 件のため書き出さない (FR-22)。
    Empty,
}

impl ConfigStore {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    // ---- 設定 (FR-01) ----

    pub fn load_settings(&self) -> Result<AppSettings, StoreError> {
        Ok(load_document(&self.paths.settings())?.unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), StoreError> {
        save_document(&self.paths.settings(), settings)
    }

    // ---- 登録先プロジェクト (FR-02) ----

    pub fn load_projects(&self) -> Result<Vec<Project>, StoreError> {
        Ok(load_document::<ProjectsFile>(&self.paths.projects())?
            .map(|f| f.projects)
            .unwrap_or_default())
    }

    pub fn save_projects(&self, projects: &[Project]) -> Result<(), StoreError> {
        save_document(
            &self.paths.projects(),
            &ProjectsFile {
                projects: projects.to_vec(),
            },
        )
    }

    // ---- 繰り返しタスク定義 (FR-03) ----

    pub fn load_schedules(&self) -> Result<Vec<Schedule>, StoreError> {
        Ok(load_document::<SchedulesFile>(&self.paths.schedules())?
            .map(|f| f.schedules)
            .unwrap_or_default())
    }

    pub fn save_schedules(&self, schedules: &[Schedule]) -> Result<(), StoreError> {
        save_document(
            &self.paths.schedules(),
            &SchedulesFile {
                schedules: schedules.to_vec(),
            },
        )
    }

    // ---- 休日カレンダー (FR-13) ----

    pub fn load_holidays(&self) -> Result<Vec<Holiday>, StoreError> {
        Ok(load_document::<HolidaysFile>(&self.paths.holidays())?
            .map(|f| f.holidays)
            .unwrap_or_default())
    }

    pub fn save_holidays(&self, holidays: &[Holiday]) -> Result<(), StoreError> {
        save_document(
            &self.paths.holidays(),
            &HolidaysFile {
                holidays: holidays.to_vec(),
            },
        )
    }

    // ---- 候補値の保持 (FR-16、ADR-07) ----

    pub fn load_candidates(&self) -> Result<Vec<CandidateCacheEntry>, StoreError> {
        Ok(load_document::<CandidatesFile>(&self.paths.candidates())?
            .map(|f| f.candidates)
            .unwrap_or_default())
    }

    pub fn save_candidates(&self, entries: &[CandidateCacheEntry]) -> Result<(), StoreError> {
        save_document(
            &self.paths.candidates(),
            &CandidatesFile {
                candidates: entries.to_vec(),
            },
        )
    }

    // ---- 一括登録の実行中を示す印 (EC-08、設計書 7.2) ----

    pub fn mark_registration_started(
        &self,
        mark: &crate::history_store::InProgressMark,
    ) -> Result<(), StoreError> {
        save_document(&self.paths.in_progress(), mark)
    }

    pub fn clear_registration_mark(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(self.paths.in_progress()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io {
                path: self.paths.in_progress(),
                source: e,
            }),
        }
    }

    /// 前回の一括登録が中断されたかを調べる (EC-08)。
    pub fn interrupted_registration(
        &self,
    ) -> Result<Option<crate::history_store::InProgressMark>, StoreError> {
        load_document(&self.paths.in_progress())
    }

    // ---- 定義の書き出しと取り込み (FR-18) ----

    pub fn export_schedules(
        &self,
        schedules: &[Schedule],
        destination: &std::path::Path,
    ) -> Result<(), StoreError> {
        save_document(
            destination,
            &SchedulesFile {
                schedules: schedules.to_vec(),
            },
        )
    }

    /// 定義を取り込む。
    ///
    /// 形式が解釈できない場合は 1 件も反映せず失敗を返す (FR-18)。
    pub fn import_schedules(
        &self,
        source: &std::path::Path,
        policy: ImportConflictPolicy,
    ) -> Result<ScheduleImportOutcome, StoreError> {
        let incoming = load_document::<SchedulesFile>(source)?
            .map(|f| f.schedules)
            .unwrap_or_default();
        let mut current = self.load_schedules()?;
        let mut outcome = ScheduleImportOutcome::default();

        for schedule in incoming {
            match current.iter().position(|s| s.name == schedule.name) {
                Some(index) => match policy {
                    ImportConflictPolicy::Overwrite => {
                        current[index] = schedule;
                        outcome.overwritten += 1;
                    }
                    ImportConflictPolicy::Skip => outcome.skipped_names.push(schedule.name),
                },
                None => {
                    current.push(schedule);
                    outcome.added += 1;
                }
            }
        }
        if outcome.added > 0 || outcome.overwritten > 0 {
            self.save_schedules(&current)?;
        }
        Ok(outcome)
    }

    // ---- 休日の書き出しと取り込み (FR-22、EC-04) ----

    /// 休日を 1 行 1 件のテキストへ書き出す。
    ///
    /// 形式は `YYYY-MM-DD,名称`。先頭が `#` の行は注記として扱う。
    /// 0 件のときは書き出さず、その旨を返す (FR-22)。
    pub fn export_holidays(
        &self,
        holidays: &[Holiday],
        destination: &std::path::Path,
    ) -> Result<Result<usize, ExportRefusal>, StoreError> {
        if holidays.is_empty() {
            return Ok(Err(ExportRefusal::Empty));
        }
        let mut text = String::from("# 休日カレンダー: 1 行 1 件、形式は YYYY-MM-DD,名称\n");
        for holiday in holidays {
            text.push_str(&format!("{},{}\n", holiday.date, holiday.name));
        }
        crate::atomic::write_atomic(destination, text.as_bytes())?;
        Ok(Ok(holidays.len()))
    }

    /// 休日を取り込む。解釈できた行のみを登録し、できなかった行の行番号を返す (FR-22)。
    ///
    /// 同一日付が重複する場合は最初の 1 件のみを登録する (EC-04)。
    /// 既存の休日と同じ日付も重複として扱う。
    pub fn import_holidays(
        &self,
        source: &std::path::Path,
    ) -> Result<HolidayImportOutcome, StoreError> {
        let text = std::fs::read_to_string(source).map_err(|e| StoreError::Io {
            path: source.to_path_buf(),
            source: e,
        })?;
        let mut holidays = self.load_holidays()?;
        let mut seen: BTreeSet<NaiveDate> = holidays.iter().map(|h| h.date).collect();
        let mut outcome = HolidayImportOutcome::default();

        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match parse_holiday_line(trimmed) {
                Some(holiday) => {
                    if seen.insert(holiday.date) {
                        holidays.push(holiday);
                        outcome.added += 1;
                    } else {
                        outcome.duplicate_lines.push(line_number);
                    }
                }
                None => outcome.malformed_lines.push(line_number),
            }
        }
        if outcome.added > 0 {
            holidays.sort_by_key(|h| h.date);
            self.save_holidays(&holidays)?;
        }
        Ok(outcome)
    }
}

fn parse_holiday_line(line: &str) -> Option<Holiday> {
    let (date_part, name_part) = match line.split_once(',') {
        Some((d, n)) => (d.trim(), n.trim()),
        None => (line, ""),
    };
    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    Some(Holiday {
        date,
        name: name_part.to_string(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectsFile {
    projects: Vec<Project>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SchedulesFile {
    schedules: Vec<Schedule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HolidaysFile {
    holidays: Vec<Holiday>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CandidatesFile {
    candidates: Vec<CandidateCacheEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_store::InProgressMark;
    use gitlabregister_core::model::{
        ActivePeriod, BusinessDayAdjustment, IssueTemplate, MissingDayPolicy, MonthlyDay,
        RecurrencePattern, ScheduleId,
    };

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn store(dir: &tempfile::TempDir) -> ConfigStore {
        ConfigStore::new(AppPaths::with_root(dir.path().join("GitLabRegister")))
    }

    fn schedule(name: &str, day: u8) -> Schedule {
        Schedule {
            id: ScheduleId(format!("id-{name}")),
            name: name.into(),
            project: "group/project".into(),
            enabled: true,
            active_period: ActivePeriod::default(),
            pattern: RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(day),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            adjustment: BusinessDayAdjustment::NextBusinessDay,
            issue: IssueTemplate {
                title: format!("{{{{年}}}}年{{{{月}}}}月 {name}"),
                due_offset_days: Some(7),
                ..IssueTemplate::default()
            },
        }
    }

    // ---- EC-07 初回起動 ----

    /// EC-07: 設定が 1 件もない状態でも読み込みが成功し、空を返す。
    #[test]
    fn document_first_run_returns_empty() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        assert_eq!(s.load_settings().expect("読める"), AppSettings::default());
        assert!(s.load_projects().expect("読める").is_empty());
        assert!(s.load_schedules().expect("読める").is_empty());
        assert!(s.load_holidays().expect("読める").is_empty());
    }

    // ---- FR-01 FR-02 FR-03 FR-13 の読み書き ----

    /// FR-01: 接続先 URL を保存して次回に読み戻せる。PAT は保存しない。
    #[test]
    fn document_settings_round_trip_without_secrets() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let settings = AppSettings {
            gitlab_url: Some("https://gitlab.example.com".into()),
            proxy: ProxySettings {
                mode: ProxyMode::Manual,
                host: Some("proxy.example.com".into()),
                port: Some(8080),
                requires_credentials: true,
            },
        };
        s.save_settings(&settings).expect("書ける");
        assert_eq!(s.load_settings().expect("読める"), settings);

        // NFR-05: 設定ファイルに PAT を入れる場所がない。
        let text = std::fs::read_to_string(s.paths().settings()).expect("読める");
        assert!(!text.contains("token"), "{text}");
        assert!(!text.contains("password"), "{text}");
    }

    #[test]
    fn document_projects_round_trip() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let projects = vec![
            Project {
                id: "11".into(),
                display_name: "group/alpha".into(),
            },
            Project {
                id: "12".into(),
                display_name: "group/beta".into(),
            },
        ];
        s.save_projects(&projects).expect("書ける");
        assert_eq!(s.load_projects().expect("読める"), projects);
    }

    #[test]
    fn document_schedules_round_trip() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let schedules = vec![schedule("月次報告", 25), schedule("棚卸し", 1)];
        s.save_schedules(&schedules).expect("書ける");
        assert_eq!(s.load_schedules().expect("読める"), schedules);
    }

    /// FR-18: 定義ファイルは人が読める。定義名・繰り返しパターン・タイトルが判別できる。
    #[test]
    fn document_schedules_file_is_human_readable() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_schedules(&[schedule("月次報告", 25)]).expect("書ける");
        let text = std::fs::read_to_string(s.paths().schedules()).expect("読める");
        assert!(text.contains("月次報告"), "定義名が読めない: {text}");
        assert!(text.contains("MonthlyByDay"), "パターンが読めない: {text}");
        assert!(text.contains("{{年}}年{{月}}月 月次報告"), "タイトルが読めない: {text}");
    }

    /// FR-18: 書き出したファイルをテキストエディタで編集して定義を 1 件追加できる。
    #[test]
    fn transfer_edited_file_can_add_schedule() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let export = dir.path().join("export.json");
        s.export_schedules(&[schedule("月次報告", 25)], &export)
            .expect("書き出せる");

        // テキストエディタでの編集を模す。定義を 1 件複製して名称を変える。
        let text = std::fs::read_to_string(&export).expect("読める");
        let mut value: serde_json::Value = serde_json::from_str(&text).expect("解釈できる");
        let list = value["schedules"].as_array().expect("配列").clone();
        let mut added = list[0].clone();
        added["name"] = serde_json::Value::String("四半期棚卸し".into());
        added["id"] = serde_json::Value::String("id-四半期棚卸し".into());
        value["schedules"] = serde_json::Value::Array(vec![list[0].clone(), added]);
        std::fs::write(&export, serde_json::to_string_pretty(&value).expect("直列化"))
            .expect("書ける");

        let outcome = s
            .import_schedules(&export, ImportConflictPolicy::Overwrite)
            .expect("取り込める");
        assert_eq!(outcome.added, 2);
        let names: Vec<String> = s
            .load_schedules()
            .expect("読める")
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert!(names.contains(&"四半期棚卸し".to_string()), "{names:?}");
    }

    /// FR-18: 10 件書き出して 0 件の端末で取り込むと全項目一致する。
    #[test]
    fn transfer_export_then_import_on_empty_store() {
        let source_dir = tempfile::tempdir().expect("一時ディレクトリ");
        let target_dir = tempfile::tempdir().expect("一時ディレクトリ");
        let source = store(&source_dir);
        let target = store(&target_dir);
        let schedules: Vec<Schedule> = (1..=10).map(|i| schedule(&format!("定義{i}"), i)).collect();
        let file = source_dir.path().join("export.json");
        source.export_schedules(&schedules, &file).expect("書き出せる");

        let outcome = target
            .import_schedules(&file, ImportConflictPolicy::Skip)
            .expect("取り込める");
        assert_eq!(outcome.added, 10);
        assert_eq!(target.load_schedules().expect("読める"), schedules);
    }

    /// FR-18: 同一名称で上書きを選ぶと既存が置き換わる。
    #[test]
    fn transfer_import_overwrite_replaces_existing() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_schedules(&[schedule("月次報告", 25)]).expect("書ける");

        let file = dir.path().join("export.json");
        s.export_schedules(&[schedule("月次報告", 10)], &file)
            .expect("書き出せる");
        let outcome = s
            .import_schedules(&file, ImportConflictPolicy::Overwrite)
            .expect("取り込める");
        assert_eq!(outcome.overwritten, 1);
        assert_eq!(outcome.added, 0);

        let loaded = s.load_schedules().expect("読める");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].pattern,
            RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(10),
                on_missing: MissingDayPolicy::SkipMonth
            }
        );
    }

    /// FR-18: 同一名称で取り込まないを選ぶと既存は変わらず、名称が返る。
    #[test]
    fn transfer_import_skip_keeps_existing_and_reports_names() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_schedules(&[schedule("月次報告", 25)]).expect("書ける");

        let file = dir.path().join("export.json");
        s.export_schedules(&[schedule("月次報告", 10)], &file)
            .expect("書き出せる");
        let outcome = s
            .import_schedules(&file, ImportConflictPolicy::Skip)
            .expect("取り込める");
        assert_eq!(outcome.skipped_names, vec!["月次報告".to_string()]);

        let loaded = s.load_schedules().expect("読める");
        assert_eq!(
            loaded[0].pattern,
            RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(25),
                on_missing: MissingDayPolicy::SkipMonth
            },
            "既存が置き換わっている"
        );
    }

    /// FR-18: 形式が解釈できないファイルは 1 件も反映せず、既存の定義を変えない。
    #[test]
    fn transfer_import_malformed_file_changes_nothing() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_schedules(&[schedule("月次報告", 25)]).expect("書ける");
        let file = dir.path().join("broken.json");
        std::fs::write(&file, "{ this is not json").expect("書ける");

        let err = s
            .import_schedules(&file, ImportConflictPolicy::Overwrite)
            .expect_err("拒否される");
        assert!(matches!(err, StoreError::Malformed { .. }), "{err}");
        assert_eq!(s.load_schedules().expect("読める").len(), 1);
    }

    // ---- FR-22 休日の取り込みと書き出し ----

    /// FR-22: 20 件書き出して 0 件の端末で取り込むと日付と名称が一致する。
    #[test]
    fn holiday_io_export_then_import() {
        let source_dir = tempfile::tempdir().expect("一時ディレクトリ");
        let target_dir = tempfile::tempdir().expect("一時ディレクトリ");
        let source = store(&source_dir);
        let target = store(&target_dir);
        let holidays: Vec<Holiday> = (1..=20)
            .map(|i| Holiday {
                date: d(2026, 1, i),
                name: format!("休業日{i}"),
            })
            .collect();
        let file = source_dir.path().join("holidays.txt");
        let written = source
            .export_holidays(&holidays, &file)
            .expect("書き出せる")
            .expect("0 件ではない");
        assert_eq!(written, 20);

        let outcome = target.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 20);
        assert!(outcome.malformed_lines.is_empty());
        assert_eq!(target.load_holidays().expect("読める"), holidays);
    }

    /// FR-22: 日付として解釈できない行は読み飛ばし、行番号を返す。
    #[test]
    fn holiday_io_reports_malformed_lines() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let file = dir.path().join("holidays.txt");
        std::fs::write(
            &file,
            "2026-11-03,文化の日\nこれは日付ではない\n2026-12-31,年末休業\n2026-13-45,ありえない日付\n",
        )
        .expect("書ける");

        let outcome = s.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 2);
        assert_eq!(outcome.malformed_lines, vec![2, 4]);
        assert_eq!(s.load_holidays().expect("読める").len(), 2);
    }

    /// EC-04: 同一日付が重複する場合は最初の 1 件のみを登録し、重複行の行番号を返す。
    #[test]
    fn holiday_io_duplicate_dates_keep_first() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let file = dir.path().join("holidays.txt");
        std::fs::write(
            &file,
            "2026-11-03,文化の日\n2026-11-03,重複した登録\n2026-12-31,年末休業\n",
        )
        .expect("書ける");

        let outcome = s.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 2);
        assert_eq!(outcome.duplicate_lines, vec![2]);
        let loaded = s.load_holidays().expect("読める");
        let culture_day = loaded
            .iter()
            .find(|h| h.date == d(2026, 11, 3))
            .expect("登録されている");
        assert_eq!(culture_day.name, "文化の日", "最初の 1 件が残っていない");
    }

    /// EC-04: 既存の休日と同じ日付も重複として扱う。
    #[test]
    fn holiday_io_duplicate_with_existing_is_reported() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_holidays(&[Holiday {
            date: d(2026, 11, 3),
            name: "文化の日".into(),
        }])
        .expect("書ける");

        let file = dir.path().join("holidays.txt");
        std::fs::write(&file, "2026-11-03,あとから追加\n").expect("書ける");
        let outcome = s.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 0);
        assert_eq!(outcome.duplicate_lines, vec![1]);
        assert_eq!(s.load_holidays().expect("読める").len(), 1);
    }

    /// FR-22: 0 件のときは書き出しを実行しない。
    #[test]
    fn holiday_io_export_refuses_when_empty() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let file = dir.path().join("holidays.txt");
        let result = s.export_holidays(&[], &file).expect("失敗しない");
        assert_eq!(result, Err(ExportRefusal::Empty));
        assert!(!file.exists(), "書き出していないのにファイルがある");
    }

    /// 注記の行と空行は読み飛ばす。書き出したファイルをそのまま取り込める。
    #[test]
    fn holiday_io_skips_comments_and_blank_lines() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let file = dir.path().join("holidays.txt");
        std::fs::write(&file, "# 注記\n\n2026-11-03,文化の日\n").expect("書ける");
        let outcome = s.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 1);
        assert!(outcome.malformed_lines.is_empty());
    }

    /// FR-22: 名称のない行も日付として解釈できれば登録する。
    #[test]
    fn holiday_io_accepts_date_without_name() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let file = dir.path().join("holidays.txt");
        std::fs::write(&file, "2026-11-03\n").expect("書ける");
        let outcome = s.import_holidays(&file).expect("取り込める");
        assert_eq!(outcome.added, 1);
        assert_eq!(s.load_holidays().expect("読める")[0].name, "");
    }

    // ---- EC-08 中断の印 ----

    /// EC-08: 一括登録の開始で印を残し、正常終了で消す。
    #[test]
    fn in_progress_mark_is_set_and_cleared() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        assert!(s.interrupted_registration().expect("読める").is_none());

        let mark = InProgressMark {
            started_at: d(2026, 10, 3).and_hms_opt(14, 0, 0).expect("有効な時刻"),
            planned_count: 20,
        };
        s.mark_registration_started(&mark).expect("印を残す");
        assert_eq!(
            s.interrupted_registration().expect("読める"),
            Some(mark.clone())
        );

        s.clear_registration_mark().expect("消す");
        assert!(s.interrupted_registration().expect("読める").is_none());
    }

    /// EC-08: 印が残っていれば、前回が中断されたと判別できる。
    #[test]
    fn in_progress_mark_detects_interruption() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.mark_registration_started(&InProgressMark {
            started_at: d(2026, 10, 3).and_hms_opt(14, 0, 0).expect("有効な時刻"),
            planned_count: 20,
        })
        .expect("印を残す");

        // 強制終了を模して、印を消さずに別の読み手で起動する。
        let restarted = store(&dir);
        let interrupted = restarted.interrupted_registration().expect("読める");
        assert!(interrupted.is_some(), "中断を検知できていない");
        assert_eq!(interrupted.expect("ある").planned_count, 20);
    }

    /// 印がない状態で消す操作をしても失敗しない。
    #[test]
    fn in_progress_clear_without_mark_succeeds() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        store(&dir).clear_registration_mark().expect("失敗しない");
    }

    /// FR-16 / ADR-07: 候補値を保持して読み戻せる。取得日時も残る。
    #[test]
    fn document_candidates_round_trip() {
        use crate::gitlab::client::{Candidate, CandidateKind};
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        assert!(s.load_candidates().expect("読める").is_empty());

        let entries = vec![CandidateCacheEntry {
            project: "group/alpha".into(),
            kind: CandidateKind::Label,
            items: vec![Candidate {
                id: "1".into(),
                name: "report".into(),
            }],
            fetched_at: d(2026, 10, 3).and_hms_opt(14, 0, 0).expect("有効な時刻"),
        }];
        s.save_candidates(&entries).expect("書ける");
        assert_eq!(s.load_candidates().expect("読める"), entries);
    }

    /// NFR-11: すべてのファイルが 1 つのフォルダ配下にあり、コピーで複製できる。
    #[test]
    fn document_all_files_under_one_folder() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.save_settings(&AppSettings::default()).expect("書ける");
        s.save_projects(&[]).expect("書ける");
        s.save_schedules(&[schedule("月次報告", 25)]).expect("書ける");
        s.save_holidays(&[]).expect("書ける");

        let root = s.paths().root();
        let names: Vec<String> = std::fs::read_dir(root)
            .expect("読める")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"settings.json".to_string()), "{names:?}");
        assert!(names.contains(&"schedules.json".to_string()), "{names:?}");
        assert!(names.contains(&"projects.json".to_string()), "{names:?}");
        assert!(names.contains(&"holidays.json".to_string()), "{names:?}");
    }
}
