//! 期間指定による展開とプレビューの組み立て (FR-07、FR-11、EC-05、EC-07、EC-08、EC-23、EC-32)。
//!
//! 発生日の算出そのものはコア層が行う。本サービスは読み込み・除外・警告の組み立てを担う。
//! 現在日付は呼び出し側から受け取る (ADR-06、EC-24)。

use chrono::NaiveDate;
use gitlabregister_core::expand::{
    self, ExpansionResult, ExpansionWarning, RangePreset, RegistrationCandidate,
};
use gitlabregister_core::history::HistoryIndex;
use gitlabregister_core::model::{DateRange, Holiday, Schedule};
use gitlabregister_infra::atomic::StoreError;
use gitlabregister_infra::gitlab::client::CandidateKind;
use gitlabregister_infra::history_store::HistoryStore;
use gitlabregister_infra::store::ConfigStore;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// プレビューに出す注意事項。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewWarning {
    /// EC-23: 営業日調整が有効だが、その年の休日が登録されていない。
    YearWithoutHolidays { year: i32, candidate_count: usize },
    /// EC-05: 登録候補が上限を超えた。
    TooManyCandidates { count: usize, limit: usize },
    /// EC-32: 登録先に存在しないラベル名が指定されている。
    UnknownLabels {
        schedule_name: String,
        labels: Vec<String>,
    },
    /// EC-32: 候補を取得していないため、ラベルの突合ができなかった。
    LabelsNotVerified { project: String },
    /// EC-08: 前回の一括登録が中断された。
    InterruptedRegistration {
        started_at: chrono::NaiveDateTime,
        planned_count: usize,
    },
    /// 履歴ファイルに解釈できない行があった。
    MalformedHistoryLines { line_numbers: Vec<usize> },
}

impl PreviewWarning {
    /// 画面に出す文面。
    pub fn describe(&self) -> String {
        match self {
            PreviewWarning::YearWithoutHolidays {
                year,
                candidate_count,
            } => format!(
                "{year} 年の休日が登録されていません。土日のみを非営業日として調整しました (この年の登録候補 {candidate_count} 件)"
            ),
            PreviewWarning::TooManyCandidates { count, limit } => format!(
                "登録候補が {count} 件で上限 {limit} 件を超えました。期間を短くしてください"
            ),
            PreviewWarning::UnknownLabels {
                schedule_name,
                labels,
            } => format!(
                "定義「{schedule_name}」のラベル {} は登録先に存在しません。このまま登録すると GitLab-EE 側にラベルが新規作成されます",
                labels.join(", ")
            ),
            PreviewWarning::LabelsNotVerified { project } => format!(
                "プロジェクト {project} の候補を取得していないため、ラベルの存在を確認できませんでした"
            ),
            PreviewWarning::InterruptedRegistration {
                started_at,
                planned_count,
            } => format!(
                "前回の一括登録 ({} 開始、{planned_count} 件予定) が中断されています。GitLab-EE 上に重複した Issue がないか確認してください",
                started_at.format("%Y-%m-%d %H:%M")
            ),
            PreviewWarning::MalformedHistoryLines { line_numbers } => format!(
                "登録履歴の {} 行目を解釈できませんでした。該当行は読み飛ばしています",
                line_numbers
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// 展開の結果 (設計書 6.1 の「展開する」の出力)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    pub candidates: Vec<RegistrationCandidate>,
    /// 履歴によって除外された発生日 (FR-11)。
    pub excluded: Vec<(String, NaiveDate)>,
    pub warnings: Vec<PreviewWarning>,
    /// 有効な定義が 1 件もなかった (EC-07)。
    pub no_enabled_schedules: bool,
}

impl Preview {
    /// 一括登録を実行できるか (FR-07、EC-05)。
    pub fn can_register(&self) -> bool {
        !self.candidates.is_empty()
            && !self
                .warnings
                .iter()
                .any(|w| matches!(w, PreviewWarning::TooManyCandidates { .. }))
    }
}

/// 展開に失敗した理由。
#[derive(Debug)]
pub enum ExpandError {
    /// 期間の指定が逆転している (FR-07)。
    RangeReversed { start: NaiveDate, end: NaiveDate },
    /// 読み込みに失敗した。
    Store(StoreError),
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpandError::RangeReversed { start, end } => write!(
                f,
                "開始日 {start} が終了日 {end} より後になっています。期間を見直してください"
            ),
            ExpandError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExpandError {}

/// 展開を組み立てる。
///
/// 履歴は最初の展開まで読み込まない。起動時の読み込みを設定・プロジェクト・定義・休日に
/// 限ることで NFR-03 (起動から接続画面の操作まで 5 秒以内) の予算を使わない (ADR-07)。
pub struct ExpansionService {
    config: ConfigStore,
    history: HistoryStore,
    /// 一度読み込んだ履歴の索引を保持する。
    index: RwLock<Option<HistoryIndex>>,
    /// ラベルの突合に使う候補。プロジェクトごとに保持する (FR-16、EC-32)。
    label_candidates: RwLock<BTreeMap<String, Vec<String>>>,
}

impl ExpansionService {
    pub fn new(config: ConfigStore) -> Self {
        let history = HistoryStore::new(config.paths().history());
        Self {
            config,
            history,
            index: RwLock::new(None),
            label_candidates: RwLock::new(BTreeMap::new()),
        }
    }

    /// 期間のプリセットを基準日から求める (FR-07、EC-24)。
    pub fn resolve_preset(&self, preset: RangePreset, today: NaiveDate) -> DateRange {
        expand::resolve_preset(preset, today)
    }

    /// ラベルの突合に使う候補を取り込む (EC-32)。
    ///
    /// CandidateService が取得した結果をここへ渡す。
    pub fn set_label_candidates(&self, project: &str, labels: Vec<String>) {
        self.label_candidates
            .write()
            .expect("施錠できる")
            .insert(project.to_string(), labels);
    }

    /// 履歴の索引を捨てる。履歴を削除したあとに呼ぶ (FR-23)。
    pub fn invalidate_history(&self) {
        *self.index.write().expect("施錠できる") = None;
    }

    /// 登録に成功した発生日を索引へ加える (FR-09、FR-11)。
    pub fn record_registered(&self, schedule_id: &gitlabregister_core::model::ScheduleId, date: NaiveDate) {
        if let Some(index) = self.index.write().expect("施錠できる").as_mut() {
            index.record(schedule_id, date);
        }
    }

    /// 指定した期間を展開してプレビューを組み立てる (FR-07)。
    pub fn expand(&self, start: NaiveDate, end: NaiveDate) -> Result<Preview, ExpandError> {
        let range = DateRange::new(start, end)
            .ok_or(ExpandError::RangeReversed { start, end })?;

        let schedules: Vec<Schedule> =
            self.config.load_schedules().map_err(ExpandError::Store)?;
        let holidays: Vec<Holiday> = self.config.load_holidays().map_err(ExpandError::Store)?;
        let mut warnings = Vec::new();

        let loaded = self.history.load().map_err(ExpandError::Store)?;
        if !loaded.malformed_lines.is_empty() {
            warnings.push(PreviewWarning::MalformedHistoryLines {
                line_numbers: loaded.malformed_lines.clone(),
            });
        }
        self.ensure_index(&loaded.entries);

        let core_result = {
            let guard = self.index.read().expect("施錠できる");
            let index = guard.as_ref().expect("索引は用意済み");
            expand::expand(&schedules, &holidays, index, &range)
        };

        // EC-08: 前回の一括登録が中断されていれば知らせる。
        if let Ok(Some(mark)) = self.config.interrupted_registration() {
            warnings.push(PreviewWarning::InterruptedRegistration {
                started_at: mark.started_at,
                planned_count: mark.planned_count,
            });
        }

        warnings.extend(core_warnings_to_preview(&core_result.warnings));
        warnings.extend(self.label_warnings(&schedules, &core_result));

        Ok(Preview {
            excluded: core_result
                .excluded
                .iter()
                .map(|(id, date)| (id.0.clone(), *date))
                .collect(),
            candidates: core_result.candidates,
            warnings,
            no_enabled_schedules: !schedules.iter().any(|s| s.enabled),
        })
    }

    fn ensure_index(&self, entries: &[gitlabregister_core::model::HistoryEntry]) {
        let mut guard = self.index.write().expect("施錠できる");
        if guard.is_none() {
            *guard = Some(HistoryIndex::build(entries));
        }
    }

    /// EC-32: 定義のラベル名を候補と突合し、存在しないものを警告する。
    fn label_warnings(
        &self,
        schedules: &[Schedule],
        result: &ExpansionResult,
    ) -> Vec<PreviewWarning> {
        let candidates = self.label_candidates.read().expect("施錠できる");
        let mut warnings = Vec::new();
        let mut unverified: Vec<String> = Vec::new();

        for schedule in schedules.iter().filter(|s| s.enabled) {
            // 登録候補が出ていない定義は登録されないため突合しない。
            let has_candidate = result
                .candidates
                .iter()
                .any(|c| c.occurrence.schedule_id == schedule.id);
            if !has_candidate || schedule.issue.labels.is_empty() {
                continue;
            }
            match candidates.get(&schedule.project) {
                Some(known) => {
                    let unknown: Vec<String> = schedule
                        .issue
                        .labels
                        .iter()
                        .filter(|l| !known.iter().any(|k| k == *l))
                        .cloned()
                        .collect();
                    if !unknown.is_empty() {
                        warnings.push(PreviewWarning::UnknownLabels {
                            schedule_name: schedule.name.clone(),
                            labels: unknown,
                        });
                    }
                }
                None => {
                    if !unverified.contains(&schedule.project) {
                        unverified.push(schedule.project.clone());
                    }
                }
            }
        }
        for project in unverified {
            warnings.push(PreviewWarning::LabelsNotVerified { project });
        }
        warnings
    }

    /// 候補の突合に使う種別。CandidateService との取り決めを 1 箇所に置く。
    pub const LABEL_KIND: CandidateKind = CandidateKind::Label;
}

fn core_warnings_to_preview(warnings: &[ExpansionWarning]) -> Vec<PreviewWarning> {
    warnings
        .iter()
        .map(|w| match w {
            ExpansionWarning::YearWithoutHolidays {
                year,
                candidate_count,
            } => PreviewWarning::YearWithoutHolidays {
                year: *year,
                candidate_count: *candidate_count,
            },
            ExpansionWarning::TooManyCandidates { count, limit } => {
                PreviewWarning::TooManyCandidates {
                    count: *count,
                    limit: *limit,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use gitlabregister_core::model::{
        ActivePeriod, BusinessDayAdjustment, HistoryEntry, IssueTemplate, MissingDayPolicy,
        MonthlyDay, RecurrencePattern, ScheduleId,
    };
    use gitlabregister_infra::history_store::InProgressMark;
    use gitlabregister_infra::paths::AppPaths;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        config: ConfigStore,
        service: ExpansionService,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let config = ConfigStore::new(AppPaths::with_root(dir.path().join("GitLabRegister")));
        let service = ExpansionService::new(ConfigStore::new(AppPaths::with_root(
            dir.path().join("GitLabRegister"),
        )));
        Fixture {
            _dir: dir,
            config,
            service,
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
                title: format!("{{{{年}}}}年{{{{月}}}}月 {name}"),
                due_offset_days: Some(7),
                ..IssueTemplate::default()
            },
        }
    }

    /// FR-07: 期間を指定すると定義が展開され、プレビューに登録候補が並ぶ。
    #[test]
    fn expansion_expands_enabled_schedules() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1), schedule("s2", "棚卸し", 15)])
            .expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert_eq!(preview.candidates.len(), 2);
        assert!(preview.can_register());
        assert!(!preview.no_enabled_schedules);
        // FR-15: 差し込みが適用されている。
        assert_eq!(preview.candidates[0].title, "2026年10月 月次報告");
    }

    /// FR-07: 期間の指定が逆転していれば展開しない。
    #[test]
    fn expansion_rejects_reversed_range() {
        let f = fixture();
        let err = f
            .service
            .expand(d(2026, 10, 31), d(2026, 10, 1))
            .expect_err("拒否される");
        assert!(matches!(err, ExpandError::RangeReversed { .. }), "{err}");
    }

    /// EC-07: 定義が 1 件もない状態でも展開は成功し、0 件であることが分かる。
    #[test]
    fn expansion_first_run_has_no_schedules() {
        let f = fixture();
        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(preview.candidates.is_empty());
        assert!(preview.no_enabled_schedules);
        assert!(!preview.can_register(), "0 件では登録できない");
    }

    /// FR-14: 無効化した定義は展開されず、有効な定義が 0 件であると分かる。
    #[test]
    fn expansion_all_disabled_reports_no_enabled_schedules() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 1);
        s.enabled = false;
        f.config.save_schedules(&[s]).expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(preview.candidates.is_empty());
        assert!(preview.no_enabled_schedules);
    }

    /// FR-11: 履歴にある発生日は除外され、除外一覧に載る。
    #[test]
    fn expansion_excludes_registered_dates() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 1),
                registered_at: d(2026, 10, 1).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 11, 30))
            .expect("展開できる");
        assert_eq!(
            preview
                .candidates
                .iter()
                .map(|c| c.occurrence.date)
                .collect::<Vec<_>>(),
            vec![d(2026, 11, 1)]
        );
        assert_eq!(preview.excluded, vec![("s1".to_string(), d(2026, 10, 1))]);
    }

    /// FR-23: 履歴を削除したあとは索引を捨て、再び登録候補になる。
    #[test]
    fn expansion_invalidate_history_reflects_deletion() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        let store = HistoryStore::new(f.config.paths().history());
        store
            .append(&HistoryEntry {
                schedule_id: ScheduleId("s1".into()),
                schedule_name: "月次報告".into(),
                occurrence_date: d(2026, 10, 1),
                registered_at: d(2026, 10, 1).and_hms_opt(9, 0, 0).expect("有効な時刻"),
                project: "group/alpha".into(),
                issue_reference: "#1".into(),
            })
            .expect("追記できる");

        let before = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(before.candidates.is_empty());

        store
            .remove(&[(ScheduleId("s1".into()), d(2026, 10, 1))])
            .expect("削除できる");
        // 索引を捨てないと古い判定が残る。
        f.service.invalidate_history();
        let after = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert_eq!(after.candidates.len(), 1);
    }

    /// FR-09: 登録に成功した発生日を索引へ加えると、次の展開で除外される。
    #[test]
    fn expansion_record_registered_excludes_next_time() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        f.service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        f.service
            .record_registered(&ScheduleId("s1".into()), d(2026, 10, 1));

        let after = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(after.candidates.is_empty());
        assert_eq!(after.excluded, vec![("s1".to_string(), d(2026, 10, 1))]);
    }

    /// EC-23: 営業日調整が有効で休日未登録の年があれば警告する。
    #[test]
    fn expansion_warns_year_without_holidays() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 1);
        s.adjustment = BusinessDayAdjustment::NextBusinessDay;
        f.config.save_schedules(&[s]).expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        let warning = preview
            .warnings
            .iter()
            .find(|w| matches!(w, PreviewWarning::YearWithoutHolidays { year: 2026, .. }))
            .expect("警告がある");
        assert!(warning.describe().contains("2026"), "{}", warning.describe());
    }

    /// EC-05: 上限を超えると警告し、一括登録を実行できない。
    #[test]
    fn expansion_warns_too_many_candidates() {
        let f = fixture();
        let s = Schedule {
            pattern: RecurrencePattern::Daily,
            ..schedule("s1", "毎日の確認", 1)
        };
        f.config.save_schedules(&[s]).expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 1, 1), d(2027, 12, 31))
            .expect("展開できる");
        assert!(!preview.can_register());
        let warning = preview
            .warnings
            .iter()
            .find(|w| matches!(w, PreviewWarning::TooManyCandidates { .. }))
            .expect("警告がある");
        assert!(warning.describe().contains("500"), "{}", warning.describe());
    }

    /// EC-32: 候補にないラベル名を警告する。GitLab 側で新規作成されることを伝える。
    #[test]
    fn expansion_warns_unknown_labels() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 1);
        s.issue.labels = vec!["report".into(), "typo-label".into()];
        f.config.save_schedules(&[s]).expect("書ける");
        f.service
            .set_label_candidates("group/alpha", vec!["report".into(), "monthly".into()]);

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        let warning = preview
            .warnings
            .iter()
            .find_map(|w| match w {
                PreviewWarning::UnknownLabels { labels, .. } => Some(labels.clone()),
                _ => None,
            })
            .expect("警告がある");
        assert_eq!(warning, vec!["typo-label".to_string()]);
        assert!(
            preview
                .warnings
                .iter()
                .any(|w| w.describe().contains("新規作成")),
            "新規作成される旨が伝わらない"
        );
        // 警告があっても登録自体は実行できる (利用者が承知して実行する)。
        assert!(preview.can_register());
    }

    /// EC-32: 候補が揃っていればラベルの警告は出ない。
    #[test]
    fn expansion_no_label_warning_when_all_known() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 1);
        s.issue.labels = vec!["report".into()];
        f.config.save_schedules(&[s]).expect("書ける");
        f.service
            .set_label_candidates("group/alpha", vec!["report".into()]);

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(
            !preview
                .warnings
                .iter()
                .any(|w| matches!(w, PreviewWarning::UnknownLabels { .. })),
            "{:?}",
            preview.warnings
        );
    }

    /// EC-32: 候補を取得していないプロジェクトは、突合できなかったことを区別して伝える。
    #[test]
    fn expansion_reports_labels_not_verified() {
        let f = fixture();
        let mut s = schedule("s1", "月次報告", 1);
        s.issue.labels = vec!["report".into()];
        f.config.save_schedules(&[s]).expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        let warning = preview
            .warnings
            .iter()
            .find(|w| matches!(w, PreviewWarning::LabelsNotVerified { .. }))
            .expect("警告がある");
        assert!(
            warning.describe().contains("group/alpha"),
            "{}",
            warning.describe()
        );
    }

    /// EC-32: ラベルを指定していない定義では突合の警告を出さない。
    #[test]
    fn expansion_no_label_warning_without_labels() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(
            !preview
                .warnings
                .iter()
                .any(|w| matches!(w, PreviewWarning::LabelsNotVerified { .. })),
            "{:?}",
            preview.warnings
        );
    }

    /// EC-08: 中断の印が残っていれば、重複確認を促す警告を出す。
    #[test]
    fn expansion_warns_interrupted_registration() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        f.config
            .mark_registration_started(&InProgressMark {
                started_at: d(2026, 10, 3).and_hms_opt(14, 0, 0).expect("有効な時刻"),
                planned_count: 20,
            })
            .expect("印を残す");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        let warning = preview
            .warnings
            .iter()
            .find(|w| matches!(w, PreviewWarning::InterruptedRegistration { .. }))
            .expect("警告がある");
        assert!(warning.describe().contains("重複"), "{}", warning.describe());
        assert!(warning.describe().contains("20"), "{}", warning.describe());
    }

    /// 履歴の破損行は警告として伝え、展開は続ける。
    #[test]
    fn expansion_warns_malformed_history_lines() {
        let f = fixture();
        f.config
            .save_schedules(&[schedule("s1", "月次報告", 1)])
            .expect("書ける");
        std::fs::create_dir_all(f.config.paths().root()).expect("作れる");
        std::fs::write(f.config.paths().history(), "{ broken\n").expect("書ける");

        let preview = f
            .service
            .expand(d(2026, 10, 1), d(2026, 10, 31))
            .expect("展開できる");
        assert!(preview
            .warnings
            .iter()
            .any(|w| matches!(w, PreviewWarning::MalformedHistoryLines { .. })));
        assert_eq!(preview.candidates.len(), 1, "展開は続ける");
    }

    /// EC-24: プリセットは渡された基準日から求める。
    #[test]
    fn expansion_resolve_preset_uses_given_today() {
        let f = fixture();
        let range = f.service.resolve_preset(RangePreset::NextMonth, d(2026, 10, 5));
        assert_eq!(range.start, d(2026, 11, 1));
        assert_eq!(range.end, d(2026, 11, 30));
    }
}
