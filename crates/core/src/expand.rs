//! 展開の入口。発生日の算出から登録候補の組み立てまでをつなぐ (FR-07、FR-05、FR-11、EC-05、EC-23、EC-24)。
//!
//! この関数群は時計を参照しない。期間のプリセットに必要な基準日は引数で受け取る (ADR-06、EC-24)。

use crate::business_day::BusinessDayCalendar;
use crate::history::HistoryIndex;
use crate::model::{DateRange, Holiday, Occurrence, Schedule, ScheduleId};
use crate::offset;
use crate::recurrence;
use crate::template::{self, RenderContext};
use chrono::{Datelike, Days, NaiveDate};

/// 1 回の一括登録で選択できる登録候補の上限 (NFR-04、EC-05)。
pub const MAX_CANDIDATES: usize = 500;

/// 期間指定を補助するプリセット (FR-07)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangePreset {
    ThisWeek,
    ThisMonth,
    NextMonth,
    ThisQuarter,
}

/// プリセットを基準日から実際の期間へ変換する。
///
/// 基準日を引数で受け取るため、表示中に日付が変わっても結果は変化しない (EC-24)。
pub fn resolve_preset(preset: RangePreset, today: NaiveDate) -> DateRange {
    let (start, end) = match preset {
        RangePreset::ThisWeek => {
            let monday = today - Days::new(u64::from(today.weekday().num_days_from_monday()));
            (monday, monday + Days::new(6))
        }
        RangePreset::ThisMonth => (
            first_day(today.year(), today.month()),
            last_day(today.year(), today.month()),
        ),
        RangePreset::NextMonth => {
            let (y, m) = next_month(today.year(), today.month());
            (first_day(y, m), last_day(y, m))
        }
        RangePreset::ThisQuarter => {
            let start_month = ((today.month() - 1) / 3) * 3 + 1;
            let end_month = start_month + 2;
            (
                first_day(today.year(), start_month),
                last_day(today.year(), end_month),
            )
        }
    };
    DateRange::new(start, end).expect("プリセットは常に開始日が終了日以前になる")
}

/// 登録候補 1 件。差し込みを適用済みのタイトルと説明を持つ (FR-15)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationCandidate {
    pub occurrence: Occurrence,
    pub schedule_name: String,
    pub project: String,
    pub title: String,
    pub description: String,
}

/// 展開で検出した注意事項。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpansionWarning {
    /// EC-23: 営業日調整が有効だが、その年の休日が登録されていない。
    YearWithoutHolidays { year: i32, candidate_count: usize },
    /// EC-05: 登録候補が上限を超えた。
    TooManyCandidates { count: usize, limit: usize },
}

/// 展開の結果 (設計書 6.1 の「展開する」の出力)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpansionResult {
    pub candidates: Vec<RegistrationCandidate>,
    /// 履歴によって除外された発生日 (FR-11)。
    pub excluded: Vec<(ScheduleId, NaiveDate)>,
    pub warnings: Vec<ExpansionWarning>,
}

impl ExpansionResult {
    /// 一括登録を実行できるか。上限超過時は実行できない (EC-05)。
    pub fn can_register(&self) -> bool {
        !self.candidates.is_empty()
            && !self
                .warnings
                .iter()
                .any(|w| matches!(w, ExpansionWarning::TooManyCandidates { .. }))
    }
}

/// 有効な定義を展開し、営業日調整と履歴による除外を適用して登録候補を組み立てる。
///
/// 無効化された定義は対象から外す (FR-14)。
pub fn expand(
    schedules: &[Schedule],
    holidays: &[Holiday],
    history: &HistoryIndex,
    range: &DateRange,
) -> ExpansionResult {
    let calendar = BusinessDayCalendar::new(holidays);
    let mut result = ExpansionResult::default();

    for schedule in schedules.iter().filter(|s| s.enabled) {
        let raw =
            recurrence::occurrences(&schedule.pattern, &schedule.active_period, range, &calendar);

        // 営業日調整を適用する。調整の結果同じ日付に重なった場合は 1 件に束ねる (FR-05)。
        let mut adjusted: Vec<(NaiveDate, NaiveDate)> = Vec::new();
        for original in raw {
            if let Some(date) = calendar.adjust(original, schedule.adjustment) {
                if !adjusted.iter().any(|(d, _)| *d == date) {
                    adjusted.push((date, original));
                }
            }
        }

        let dates: Vec<NaiveDate> = adjusted.iter().map(|(d, _)| *d).collect();
        let (pending, excluded) = history.partition(&schedule.id, &dates);
        result
            .excluded
            .extend(excluded.into_iter().map(|d| (schedule.id.clone(), d)));

        for date in pending {
            let original_date = adjusted
                .iter()
                .find(|(d, _)| *d == date)
                .map(|(_, o)| *o)
                .unwrap_or(date);
            let dates = offset::calculate(
                date,
                schedule.issue.start_offset_days,
                schedule.issue.due_offset_days,
            );
            let context = RenderContext {
                occurrence_date: date,
                start_date: dates.start_date,
                due_date: dates.due_date,
                schedule_name: &schedule.name,
            };
            result.candidates.push(RegistrationCandidate {
                occurrence: Occurrence {
                    schedule_id: schedule.id.clone(),
                    date,
                    original_date,
                    start_date: dates.start_date,
                    due_date: dates.due_date,
                },
                schedule_name: schedule.name.clone(),
                project: schedule.project.clone(),
                title: template::render(&schedule.issue.title, &context),
                description: template::render(&schedule.issue.description, &context),
            });
        }
    }

    result.candidates.sort_by_key(|c| c.occurrence.date);
    result.warnings.extend(holiday_warnings(
        schedules,
        &calendar,
        range,
        &result.candidates,
    ));
    if result.candidates.len() > MAX_CANDIDATES {
        result.warnings.push(ExpansionWarning::TooManyCandidates {
            count: result.candidates.len(),
            limit: MAX_CANDIDATES,
        });
    }
    result
}

/// EC-23: 営業日調整を使う定義があり、期間に休日未登録の年が含まれる場合に警告する。
fn holiday_warnings(
    schedules: &[Schedule],
    calendar: &BusinessDayCalendar,
    range: &DateRange,
    candidates: &[RegistrationCandidate],
) -> Vec<ExpansionWarning> {
    use crate::model::BusinessDayAdjustment;
    let uses_adjustment = schedules
        .iter()
        .filter(|s| s.enabled)
        .any(|s| !matches!(s.adjustment, BusinessDayAdjustment::None));
    if !uses_adjustment {
        return Vec::new();
    }
    (range.start.year()..=range.end.year())
        .filter(|y| !calendar.has_holidays_in_year(*y))
        .map(|year| ExpansionWarning::YearWithoutHolidays {
            year,
            candidate_count: candidates
                .iter()
                .filter(|c| c.occurrence.date.year() == year)
                .count(),
        })
        .collect()
}

fn first_day(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).expect("月の 1 日は常に存在する")
}

fn last_day(year: i32, month: u32) -> NaiveDate {
    let (y, m) = next_month(year, month);
    first_day(y, m)
        .pred_opt()
        .expect("翌月 1 日の前日は常に存在する")
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ActivePeriod, BusinessDayAdjustment, IssueTemplate, MissingDayPolicy, MonthlyDay,
        RecurrencePattern,
    };

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn range(from: (i32, u32, u32), to: (i32, u32, u32)) -> DateRange {
        DateRange::new(d(from.0, from.1, from.2), d(to.0, to.1, to.2)).expect("有効な期間")
    }

    fn schedule(
        id: &str,
        pattern: RecurrencePattern,
        adjustment: BusinessDayAdjustment,
    ) -> Schedule {
        Schedule {
            id: ScheduleId(id.into()),
            name: format!("定義{id}"),
            project: "group/project".into(),
            enabled: true,
            active_period: ActivePeriod::default(),
            pattern,
            adjustment,
            issue: IssueTemplate {
                title: "{{年}}年{{月}}月 作業".into(),
                due_offset_days: Some(7),
                ..IssueTemplate::default()
            },
        }
    }

    fn monthly_first() -> RecurrencePattern {
        RecurrencePattern::MonthlyByDay {
            day: MonthlyDay::Day(1),
            on_missing: MissingDayPolicy::SkipMonth,
        }
    }

    /// FR-05: 毎日・後ろ倒しで土曜分と日曜分が同じ翌営業日に重なり、登録候補は 1 件になる。
    #[test]
    fn expand_adjust_collision_is_deduplicated() {
        // 2026-10-31 は土曜、2026-11-01 は日曜、2026-11-02 は月曜。
        let s = schedule(
            "A",
            RecurrencePattern::Daily,
            BusinessDayAdjustment::NextBusinessDay,
        );
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 31), (2026, 11, 1)),
        );
        assert_eq!(
            got.candidates.len(),
            1,
            "重なった発生日が束ねられていない: {:?}",
            got.candidates
                .iter()
                .map(|c| c.occurrence.date)
                .collect::<Vec<_>>()
        );
        assert_eq!(got.candidates[0].occurrence.date, d(2026, 11, 2));
    }

    /// EC-24: 期間のプリセットは渡された基準日から計算する。基準日が変わらなければ結果も変わらない。
    #[test]
    fn expand_uses_injected_today() {
        // 2026-10-05 は月曜。
        let today = d(2026, 10, 5);
        assert_eq!(
            resolve_preset(RangePreset::ThisWeek, today),
            range((2026, 10, 5), (2026, 10, 11))
        );
        assert_eq!(
            resolve_preset(RangePreset::ThisMonth, today),
            range((2026, 10, 1), (2026, 10, 31))
        );
        assert_eq!(
            resolve_preset(RangePreset::NextMonth, today),
            range((2026, 11, 1), (2026, 11, 30))
        );
        assert_eq!(
            resolve_preset(RangePreset::ThisQuarter, today),
            range((2026, 10, 1), (2026, 12, 31))
        );
        // 翌日を基準にしても今月の範囲は同じ。基準日のみが結果を決める。
        assert_eq!(
            resolve_preset(RangePreset::ThisMonth, d(2026, 10, 6)),
            resolve_preset(RangePreset::ThisMonth, today)
        );
    }

    /// FR-07: 無効化された定義の登録候補は含まれない (FR-14)。
    #[test]
    fn expand_skips_disabled_schedule() {
        let mut s = schedule("A", monthly_first(), BusinessDayAdjustment::None);
        s.enabled = false;
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 1), (2026, 12, 31)),
        );
        assert!(got.candidates.is_empty());
    }

    /// FR-07: 複数の定義から算出された候補が 1 つの一覧になる。
    #[test]
    fn expand_merges_multiple_schedules() {
        let a = schedule("A", monthly_first(), BusinessDayAdjustment::None);
        let b = schedule(
            "B",
            RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(15),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            BusinessDayAdjustment::None,
        );
        let got = expand(
            &[a, b],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 1), (2026, 10, 31)),
        );
        assert_eq!(got.candidates.len(), 2);
        // 発生日の昇順で並ぶ。
        assert_eq!(got.candidates[0].occurrence.date, d(2026, 10, 1));
        assert_eq!(got.candidates[1].occurrence.date, d(2026, 10, 15));
    }

    /// FR-15: 展開時に差し込み項目が置き換わる。
    #[test]
    fn expand_renders_placeholders() {
        let s = schedule("A", monthly_first(), BusinessDayAdjustment::None);
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 1), (2026, 10, 31)),
        );
        assert_eq!(got.candidates[0].title, "2026年10月 作業");
        // FR-06: 期限オフセット 7。
        assert_eq!(got.candidates[0].occurrence.due_date, Some(d(2026, 10, 8)));
    }

    /// FR-11: 履歴にある発生日は候補から外れ、除外一覧に載る。
    #[test]
    fn expand_excludes_registered_dates() {
        let s = schedule("A", monthly_first(), BusinessDayAdjustment::None);
        let mut history = HistoryIndex::default();
        history.record(&ScheduleId("A".into()), d(2026, 10, 1));
        let got = expand(&[s], &[], &history, &range((2026, 10, 1), (2026, 11, 30)));
        assert_eq!(
            got.candidates
                .iter()
                .map(|c| c.occurrence.date)
                .collect::<Vec<_>>(),
            vec![d(2026, 11, 1)]
        );
        assert_eq!(got.excluded, vec![(ScheduleId("A".into()), d(2026, 10, 1))]);
    }

    /// FR-05: 営業日調整で期間外に出た発生日も候補として残る。調整前の日付を併せて保持する。
    #[test]
    fn expand_keeps_candidate_adjusted_outside_range() {
        let s = schedule(
            "A",
            monthly_first(),
            BusinessDayAdjustment::PreviousBusinessDay,
        );
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 11, 1), (2026, 11, 30)),
        );
        assert_eq!(got.candidates.len(), 1);
        assert_eq!(got.candidates[0].occurrence.date, d(2026, 10, 30));
        assert_eq!(got.candidates[0].occurrence.original_date, d(2026, 11, 1));
    }

    /// EC-23: 営業日調整を使う定義があり、休日未登録の年が含まれると警告する。
    #[test]
    fn expand_warns_year_without_holidays() {
        let s = schedule("A", monthly_first(), BusinessDayAdjustment::NextBusinessDay);
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 1), (2026, 10, 31)),
        );
        assert!(got
            .warnings
            .iter()
            .any(|w| matches!(w, ExpansionWarning::YearWithoutHolidays { year: 2026, .. })));
    }

    /// EC-23: 営業日調整を使わない定義だけなら休日の警告は出ない。
    #[test]
    fn expand_no_holiday_warning_without_adjustment() {
        let s = schedule("A", monthly_first(), BusinessDayAdjustment::None);
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 10, 1), (2026, 10, 31)),
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// EC-05: 登録候補が上限を超えると警告し、一括登録を実行できない状態にする。
    #[test]
    fn expand_warns_too_many_candidates() {
        let s = schedule("A", RecurrencePattern::Daily, BusinessDayAdjustment::None);
        // 2026-01-01 から 2027-12-31 は 730 日。
        let got = expand(
            &[s],
            &[],
            &HistoryIndex::default(),
            &range((2026, 1, 1), (2027, 12, 31)),
        );
        assert_eq!(got.candidates.len(), 730);
        assert!(got.warnings.iter().any(|w| matches!(
            w,
            ExpansionWarning::TooManyCandidates {
                count: 730,
                limit: 500
            }
        )));
        assert!(!got.can_register(), "上限超過では一括登録を実行できない");
    }

    /// FR-07: 登録候補が 0 件のときは一括登録を実行できない。
    #[test]
    fn expand_cannot_register_when_empty() {
        let got = ExpansionResult::default();
        assert!(!got.can_register());
    }

    /// FR-07: 期間の逆転は期間の生成時点で拒否する。
    #[test]
    fn expand_rejects_reversed_range() {
        assert!(DateRange::new(d(2026, 10, 31), d(2026, 10, 1)).is_none());
        // 同一日は有効。
        assert!(DateRange::new(d(2026, 10, 1), d(2026, 10, 1)).is_some());
    }
}
