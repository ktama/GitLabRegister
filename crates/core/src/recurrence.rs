//! 繰り返しパターンから発生日を算出する (FR-04、FR-21、EC-20〜EC-22、EC-30、EC-31)。
//!
//! この層は時計を参照しない。基準日が必要な場合は呼び出し側が引数で渡す (ADR-06、EC-24)。

use crate::business_day::BusinessDayCalendar;
use crate::model::{
    ActivePeriod, DateRange, MissingDayPolicy, MonthlyDay, RecurrencePattern, WeekOrdinal,
    WeekdayKey,
};
use chrono::{Datelike, Days, NaiveDate};

/// 指定期間に対してパターンを適用し、営業日調整前の発生日を昇順で返す。
///
/// 期間の判定は調整前の発生日に対して行う。
/// 調整によって期間の外へ出る場合も発生日として残すのは FR-05 の受け入れ基準による。
pub fn occurrences(
    pattern: &RecurrencePattern,
    active_period: &ActivePeriod,
    range: &DateRange,
    calendar: &BusinessDayCalendar,
) -> Vec<NaiveDate> {
    let raw = match pattern {
        RecurrencePattern::Daily => range.days().collect(),
        RecurrencePattern::EveryBusinessDay => range
            .days()
            .filter(|d| calendar.is_business_day(*d))
            .collect(),
        RecurrencePattern::EveryNDays { interval, anchor } => {
            every_n_days(*interval, *anchor, range)
        }
        RecurrencePattern::Weekly {
            interval,
            weekdays,
            anchor,
        } => weekly(*interval, weekdays, *anchor, range),
        RecurrencePattern::MonthlyByDay { day, on_missing } => {
            monthly_by_day(*day, *on_missing, range)
        }
        RecurrencePattern::MonthlyByWeekday { ordinal, weekday } => {
            monthly_by_weekday(*ordinal, *weekday, range)
        }
        RecurrencePattern::Yearly { month, day } => yearly(*month, *day, range),
    };

    let mut dates: Vec<NaiveDate> = raw
        .into_iter()
        .filter(|d| active_period.contains(*d))
        .collect();
    dates.sort_unstable();
    dates.dedup();
    dates
}

fn every_n_days(interval: u16, anchor: NaiveDate, range: &DateRange) -> Vec<NaiveDate> {
    if interval == 0 {
        return Vec::new();
    }
    range
        .days()
        .filter(|d| *d >= anchor)
        .filter(|d| {
            let elapsed = (*d - anchor).num_days();
            elapsed % i64::from(interval) == 0
        })
        .collect()
}

/// 毎週・N 週ごと。
///
/// 起算日を含む週を 1 週目として数え、1 週目から N 週ごとの週に発生させる (FR-04)。
/// 週の区切りは月曜から日曜までとする。
/// 発生する週の判定は起算日のみに依存し、期間の指定では変化しない。
/// 起算日より前の日付は発生日としない (EC-31)。
fn weekly(
    interval: u8,
    weekdays: &std::collections::BTreeSet<WeekdayKey>,
    anchor: Option<NaiveDate>,
    range: &DateRange,
) -> Vec<NaiveDate> {
    if interval == 0 || weekdays.is_empty() {
        return Vec::new();
    }
    // 毎週 (N = 1) は起算日を要さない。N が 2 以上で起算日がなければ算出しない。
    let anchor_week_start = match (interval, anchor) {
        (1, None) => None,
        (_, Some(a)) => Some(week_start(a)),
        (_, None) => return Vec::new(),
    };

    range
        .days()
        .filter(|d| weekdays.contains(&WeekdayKey::from_date(*d)))
        .filter(|d| match (anchor, anchor_week_start) {
            (Some(a), Some(aws)) => {
                if *d < a {
                    return false;
                }
                let weeks = (week_start(*d) - aws).num_days() / 7;
                weeks >= 0 && weeks % i64::from(interval) == 0
            }
            _ => true,
        })
        .collect()
}

/// その日を含む週の月曜日。
fn week_start(date: NaiveDate) -> NaiveDate {
    let offset = date.weekday().num_days_from_monday();
    date - Days::new(u64::from(offset))
}

fn monthly_by_day(day: MonthlyDay, on_missing: MissingDayPolicy, range: &DateRange) -> Vec<NaiveDate> {
    months_in(range)
        .filter_map(|(year, month)| match day {
            MonthlyDay::LastDay => Some(last_day_of_month(year, month)),
            MonthlyDay::Day(target) => {
                match NaiveDate::from_ymd_opt(year, month, u32::from(target)) {
                    Some(d) => Some(d),
                    // その月に存在しない日付 (EC-20)
                    None => match on_missing {
                        MissingDayPolicy::ClampToLastDay => Some(last_day_of_month(year, month)),
                        MissingDayPolicy::SkipMonth => None,
                    },
                }
            }
        })
        .filter(|d| range.contains(*d))
        .collect()
}

/// 毎月 第 N 曜日。その月に第 N 曜日が存在しない場合は算出しない (EC-21)。
fn monthly_by_weekday(
    ordinal: WeekOrdinal,
    weekday: WeekdayKey,
    range: &DateRange,
) -> Vec<NaiveDate> {
    months_in(range)
        .filter_map(|(year, month)| {
            let first = NaiveDate::from_ymd_opt(year, month, 1)?;
            let last = last_day_of_month(year, month);
            let mut matches = Vec::new();
            let mut cursor = first;
            while cursor <= last {
                if WeekdayKey::from_date(cursor) == weekday {
                    matches.push(cursor);
                }
                cursor = cursor.succ_opt()?;
            }
            matches.get(ordinal.index() as usize).copied()
        })
        .filter(|d| range.contains(*d))
        .collect()
}

/// 毎年。展開対象の年に指定日が存在しない場合は算出しない (EC-22、閏年でない年の 2 月 29 日)。
fn yearly(month: u8, day: u8, range: &DateRange) -> Vec<NaiveDate> {
    (range.start.year()..=range.end.year())
        .filter_map(|year| NaiveDate::from_ymd_opt(year, u32::from(month), u32::from(day)))
        .filter(|d| range.contains(*d))
        .collect()
}

/// 期間に含まれる (年, 月) を昇順で返す。
fn months_in(range: &DateRange) -> impl Iterator<Item = (i32, u32)> + '_ {
    let start = (range.start.year(), range.start.month());
    let end = (range.end.year(), range.end.month());
    std::iter::successors(Some(start), move |(y, m)| {
        let next = if *m == 12 { (*y + 1, 1) } else { (*y, *m + 1) };
        if next <= end {
            Some(next)
        } else {
            None
        }
    })
}

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|d| d.pred_opt())
        .expect("翌月 1 日の前日は常に存在する")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Holiday;
    use std::collections::BTreeSet;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn range(from: (i32, u32, u32), to: (i32, u32, u32)) -> DateRange {
        DateRange::new(d(from.0, from.1, from.2), d(to.0, to.1, to.2)).expect("有効な期間")
    }

    fn no_holidays() -> BusinessDayCalendar {
        BusinessDayCalendar::new(&[])
    }

    fn run(pattern: &RecurrencePattern, r: DateRange) -> Vec<NaiveDate> {
        occurrences(pattern, &ActivePeriod::default(), &r, &no_holidays())
    }

    fn weekdays(keys: &[WeekdayKey]) -> BTreeSet<WeekdayKey> {
        keys.iter().copied().collect()
    }

    // ---- 毎日・毎営業日・N 日ごと (FR-04 パターン 1〜3) ----

    #[test]
    fn daily_covers_every_day_in_range() {
        let got = run(&RecurrencePattern::Daily, range((2026, 10, 1), (2026, 10, 5)));
        assert_eq!(got.len(), 5);
        assert_eq!(got.first(), Some(&d(2026, 10, 1)));
        assert_eq!(got.last(), Some(&d(2026, 10, 5)));
    }

    #[test]
    fn daily_every_business_day_excludes_weekend_and_holiday() {
        let cal = BusinessDayCalendar::new(&[Holiday {
            date: d(2026, 10, 1),
            name: "会社休業日".into(),
        }]);
        // 2026-10-01 は木曜、10-03 は土曜、10-04 は日曜。
        let got = occurrences(
            &RecurrencePattern::EveryBusinessDay,
            &ActivePeriod::default(),
            &range((2026, 10, 1), (2026, 10, 5)),
            &cal,
        );
        assert_eq!(got, vec![d(2026, 10, 2), d(2026, 10, 5)]);
    }

    #[test]
    fn daily_every_n_days_counts_from_anchor() {
        let got = run(
            &RecurrencePattern::EveryNDays {
                interval: 3,
                anchor: d(2026, 10, 1),
            },
            range((2026, 10, 1), (2026, 10, 10)),
        );
        assert_eq!(
            got,
            vec![d(2026, 10, 1), d(2026, 10, 4), d(2026, 10, 7), d(2026, 10, 10)]
        );
    }

    #[test]
    fn daily_every_n_days_ignores_dates_before_anchor() {
        let got = run(
            &RecurrencePattern::EveryNDays {
                interval: 3,
                anchor: d(2026, 10, 5),
            },
            range((2026, 10, 1), (2026, 10, 8)),
        );
        assert_eq!(got, vec![d(2026, 10, 5), d(2026, 10, 8)]);
    }

    // ---- 毎週・N 週ごと (FR-04 パターン 4、EC-31) ----

    /// FR-04: 毎週・月曜と木曜・1 週ごと。2026-10 の月曜と木曜の全日付。
    #[test]
    fn weekly_every_week_mon_thu() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 1,
                weekdays: weekdays(&[WeekdayKey::Mon, WeekdayKey::Thu]),
                anchor: None,
            },
            range((2026, 10, 1), (2026, 10, 31)),
        );
        // 2026-10-01 は木曜。木: 1,8,15,22,29 / 月: 5,12,19,26
        assert_eq!(
            got,
            vec![
                d(2026, 10, 1),
                d(2026, 10, 5),
                d(2026, 10, 8),
                d(2026, 10, 12),
                d(2026, 10, 15),
                d(2026, 10, 19),
                d(2026, 10, 22),
                d(2026, 10, 26),
                d(2026, 10, 29),
            ]
        );
    }

    /// FR-04: 起算日 2026-10-05 (月曜)・2 週ごと・火曜。2 か月分で 4 件。
    #[test]
    fn weekly_biweekly_tuesday_two_months() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 2,
                weekdays: weekdays(&[WeekdayKey::Tue]),
                anchor: Some(d(2026, 10, 5)),
            },
            range((2026, 10, 1), (2026, 11, 30)),
        );
        assert_eq!(
            got,
            vec![
                d(2026, 10, 6),
                d(2026, 10, 20),
                d(2026, 11, 3),
                d(2026, 11, 17)
            ]
        );
    }

    /// FR-04: 発生週の判定は起算日のみに依存し、展開期間の指定では変化しない。
    #[test]
    fn weekly_biweekly_independent_of_range() {
        let pattern = RecurrencePattern::Weekly {
            interval: 2,
            weekdays: weekdays(&[WeekdayKey::Tue]),
            anchor: Some(d(2026, 10, 5)),
        };
        let november_only = run(&pattern, range((2026, 11, 1), (2026, 11, 30)));
        assert_eq!(november_only, vec![d(2026, 11, 3), d(2026, 11, 17)]);

        let two_months = run(&pattern, range((2026, 10, 1), (2026, 11, 30)));
        let november_part: Vec<_> = two_months
            .into_iter()
            .filter(|d| d.month() == 11)
            .collect();
        assert_eq!(november_only, november_part);
    }

    /// FR-04: 起算日 2026-10-05・2 週ごと・火曜と金曜。
    #[test]
    fn weekly_biweekly_tuesday_and_friday() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 2,
                weekdays: weekdays(&[WeekdayKey::Tue, WeekdayKey::Fri]),
                anchor: Some(d(2026, 10, 5)),
            },
            range((2026, 10, 1), (2026, 10, 31)),
        );
        assert_eq!(
            got,
            vec![
                d(2026, 10, 6),
                d(2026, 10, 9),
                d(2026, 10, 20),
                d(2026, 10, 23)
            ]
        );
    }

    /// FR-04: 起算日より前の期間は 0 件。
    #[test]
    fn weekly_biweekly_before_anchor_is_empty() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 2,
                weekdays: weekdays(&[WeekdayKey::Tue]),
                anchor: Some(d(2026, 10, 5)),
            },
            range((2026, 9, 1), (2026, 9, 30)),
        );
        assert!(got.is_empty(), "起算日より前は発生しない: {got:?}");
    }

    /// EC-31: 起算日が未来。起算日以降の発生日のみを算出する。
    #[test]
    fn weekly_biweekly_future_anchor() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 2,
                weekdays: weekdays(&[WeekdayKey::Mon]),
                anchor: Some(d(2026, 12, 7)),
            },
            range((2026, 10, 1), (2026, 11, 30)),
        );
        assert!(got.is_empty(), "起算日より前は発生しない: {got:?}");
    }

    // ---- 毎月 (FR-04 パターン 5〜6、EC-20、EC-21) ----

    /// FR-04: 毎月 25 日を 12 か月分展開すると 12 件。
    #[test]
    fn monthly_day25_twelve_months() {
        let got = run(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(25),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            range((2026, 1, 1), (2026, 12, 31)),
        );
        assert_eq!(got.len(), 12);
        assert!(got.iter().all(|d| d.day() == 25));
    }

    /// FR-04: 毎月 第 2 火曜を 3 か月分。
    #[test]
    fn monthly_second_tuesday() {
        let got = run(
            &RecurrencePattern::MonthlyByWeekday {
                ordinal: WeekOrdinal::Second,
                weekday: WeekdayKey::Tue,
            },
            range((2026, 10, 1), (2026, 12, 31)),
        );
        assert_eq!(
            got,
            vec![d(2026, 10, 13), d(2026, 11, 10), d(2026, 12, 8)]
        );
    }

    /// FR-04 / EC-20: 毎月 31 日・末日に繰り上げ。2026-02 は 2026-02-28。
    #[test]
    fn monthly_day31_clamp_to_last_day() {
        let got = run(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(31),
                on_missing: MissingDayPolicy::ClampToLastDay,
            },
            range((2026, 2, 1), (2026, 2, 28)),
        );
        assert_eq!(got, vec![d(2026, 2, 28)]);
    }

    /// FR-04 / EC-20: 毎月 31 日・その月は発生させない。
    #[test]
    fn monthly_day31_skip_month() {
        let got = run(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(31),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            range((2026, 2, 1), (2026, 2, 28)),
        );
        assert!(got.is_empty(), "存在しない日付は発生しない: {got:?}");
    }

    #[test]
    fn monthly_last_day() {
        let got = run(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::LastDay,
                on_missing: MissingDayPolicy::SkipMonth,
            },
            range((2026, 1, 1), (2026, 3, 31)),
        );
        assert_eq!(
            got,
            vec![d(2026, 1, 31), d(2026, 2, 28), d(2026, 3, 31)]
        );
    }

    /// EC-21: その月に第 5 金曜が存在しない月は算出しない。
    #[test]
    fn monthly_fifth_friday_missing_month() {
        let got = run(
            &RecurrencePattern::MonthlyByWeekday {
                ordinal: WeekOrdinal::Fifth,
                weekday: WeekdayKey::Fri,
            },
            range((2026, 1, 1), (2026, 12, 31)),
        );
        // 第 5 金曜がある月のみ。全 12 か月より少ない。
        assert!(
            got.len() < 12,
            "第 5 金曜のない月が除かれていない: {got:?}"
        );
        assert!(got.iter().all(|d| WeekdayKey::from_date(*d) == WeekdayKey::Fri));
        // 算出された日はすべてその月の 5 番目の金曜 = 29 日以降。
        assert!(got.iter().all(|d| d.day() >= 29));
    }

    // ---- 毎年 (FR-04 パターン 7、EC-22) ----

    #[test]
    fn yearly_same_month_and_day_each_year() {
        let got = run(
            &RecurrencePattern::Yearly { month: 4, day: 1 },
            range((2026, 1, 1), (2028, 12, 31)),
        );
        assert_eq!(
            got,
            vec![d(2026, 4, 1), d(2027, 4, 1), d(2028, 4, 1)]
        );
    }

    /// EC-22: 毎年 2 月 29 日。閏年でない年は算出しない。2028 は閏年。
    #[test]
    fn yearly_feb29_non_leap_year() {
        let got = run(
            &RecurrencePattern::Yearly { month: 2, day: 29 },
            range((2026, 1, 1), (2028, 12, 31)),
        );
        assert_eq!(got, vec![d(2028, 2, 29)]);
    }

    // ---- 有効期間 (FR-21) ----

    /// FR-21: 有効期間 2026-04-01〜2026-09-30 の毎月 1 日は 6 件。
    #[test]
    fn active_period_limits_occurrences() {
        let got = occurrences(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(1),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            &ActivePeriod {
                start: Some(d(2026, 4, 1)),
                end: Some(d(2026, 9, 30)),
            },
            &range((2026, 1, 1), (2026, 12, 31)),
            &no_holidays(),
        );
        assert_eq!(
            got,
            vec![
                d(2026, 4, 1),
                d(2026, 5, 1),
                d(2026, 6, 1),
                d(2026, 7, 1),
                d(2026, 8, 1),
                d(2026, 9, 1),
            ]
        );
    }

    /// FR-21: 有効期間を指定しない定義は期間による制限を受けない。
    #[test]
    fn active_period_absent_no_limit() {
        let got = run(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(1),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            range((2026, 1, 1), (2026, 12, 31)),
        );
        assert_eq!(got.len(), 12);
    }

    /// FR-21: 有効期間の開始日のみを設定した場合、開始日以降のみ算出する。
    #[test]
    fn active_period_start_only() {
        let got = occurrences(
            &RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(1),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            &ActivePeriod {
                start: Some(d(2026, 7, 1)),
                end: None,
            },
            &range((2026, 1, 1), (2026, 12, 31)),
            &no_holidays(),
        );
        assert_eq!(got.len(), 6);
        assert_eq!(got.first(), Some(&d(2026, 7, 1)));
    }

    /// FR-04: 2 週以上で起算日を指定しない場合は算出しない (保存時に拒否するが、
    /// 取り込んだデータで起こりうるため算出側でも 0 件とする)。
    #[test]
    fn weekly_interval_without_anchor_yields_nothing() {
        let got = run(
            &RecurrencePattern::Weekly {
                interval: 2,
                weekdays: weekdays(&[WeekdayKey::Tue]),
                anchor: None,
            },
            range((2026, 10, 1), (2026, 10, 31)),
        );
        assert!(got.is_empty(), "起算日のない隔週は算出しない: {got:?}");
    }
}
