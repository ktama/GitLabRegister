//! 営業日の判定と発生日の調整 (FR-05、FR-13、EC-23)。

use crate::model::{BusinessDayAdjustment, Holiday, WeekdayKey};
use chrono::{Datelike, NaiveDate};
use std::collections::{BTreeSet, HashSet};

/// 前倒し・後ろ倒しの探索上限。
///
/// FR-05 は探索範囲に期間外の日付も含めると定めるが上限は定めていない。
/// 連続する非営業日が 366 日続くことは暦上ありえないため、
/// 休日カレンダーの誤登録で無限に探索しないための安全弁として置く。
const MAX_SEARCH_DAYS: u32 = 366;

/// 土日と休日カレンダーから営業日を判定する。
#[derive(Debug, Clone, Default)]
pub struct BusinessDayCalendar {
    holidays: HashSet<NaiveDate>,
    years_with_holidays: BTreeSet<i32>,
}

impl BusinessDayCalendar {
    pub fn new(holidays: &[Holiday]) -> Self {
        Self {
            holidays: holidays.iter().map(|h| h.date).collect(),
            years_with_holidays: holidays.iter().map(|h| h.date.year()).collect(),
        }
    }

    /// 土曜・日曜のいずれでもなく、休日カレンダーにも登録されていない日 (要件書 用語定義)。
    pub fn is_business_day(&self, date: NaiveDate) -> bool {
        !WeekdayKey::from_date(date).is_weekend() && !self.holidays.contains(&date)
    }

    /// その年に休日の登録があるか。登録がない年を警告するために使う (EC-23)。
    pub fn has_holidays_in_year(&self, year: i32) -> bool {
        self.years_with_holidays.contains(&year)
    }

    /// 調整方法に従って発生日を移動する。
    ///
    /// 戻り値が `None` のとき、その回は発生しない (`SkipOccurrence`、または探索上限超過)。
    pub fn adjust(
        &self,
        date: NaiveDate,
        adjustment: BusinessDayAdjustment,
    ) -> Option<NaiveDate> {
        match adjustment {
            BusinessDayAdjustment::None => Some(date),
            BusinessDayAdjustment::SkipOccurrence => {
                if self.is_business_day(date) {
                    Some(date)
                } else {
                    None
                }
            }
            BusinessDayAdjustment::PreviousBusinessDay => self.search(date, Direction::Backward),
            BusinessDayAdjustment::NextBusinessDay => self.search(date, Direction::Forward),
        }
    }

    fn search(&self, from: NaiveDate, direction: Direction) -> Option<NaiveDate> {
        let mut cursor = from;
        for _ in 0..=MAX_SEARCH_DAYS {
            if self.is_business_day(cursor) {
                return Some(cursor);
            }
            cursor = match direction {
                Direction::Forward => cursor.succ_opt()?,
                Direction::Backward => cursor.pred_opt()?,
            };
        }
        None
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn holiday(y: i32, m: u32, day: u32, name: &str) -> Holiday {
        Holiday {
            date: d(y, m, day),
            name: name.to_string(),
        }
    }

    #[test]
    fn business_day_weekend_is_not_business_day() {
        let cal = BusinessDayCalendar::new(&[]);
        // 2026-11-01 は日曜、2026-10-31 は土曜。
        assert!(!cal.is_business_day(d(2026, 11, 1)));
        assert!(!cal.is_business_day(d(2026, 10, 31)));
        assert!(cal.is_business_day(d(2026, 10, 30)));
    }

    #[test]
    fn business_day_registered_holiday_is_not_business_day() {
        let cal = BusinessDayCalendar::new(&[holiday(2026, 11, 3, "文化の日")]);
        assert!(!cal.is_business_day(d(2026, 11, 3)));
    }

    /// FR-05: 毎月 1 日・直後の営業日に後ろ倒し。2026-11-01 は日曜、2026-11-02 は営業日。
    #[test]
    fn business_day_adjust_next_business_day() {
        let cal = BusinessDayCalendar::new(&[]);
        assert_eq!(
            cal.adjust(d(2026, 11, 1), BusinessDayAdjustment::NextBusinessDay),
            Some(d(2026, 11, 2))
        );
    }

    /// FR-05: 毎月 1 日・直前の営業日に前倒し。探索は期間外の 2026-10-30 に出る。
    #[test]
    fn business_day_adjust_previous_business_day_outside_range() {
        let cal = BusinessDayCalendar::new(&[]);
        assert_eq!(
            cal.adjust(d(2026, 11, 1), BusinessDayAdjustment::PreviousBusinessDay),
            Some(d(2026, 10, 30))
        );
    }

    /// FR-05: その回を発生させない。
    #[test]
    fn business_day_adjust_skip_occurrence() {
        let cal = BusinessDayCalendar::new(&[]);
        assert_eq!(
            cal.adjust(d(2026, 11, 1), BusinessDayAdjustment::SkipOccurrence),
            None
        );
        // 営業日ならそのまま残る。
        assert_eq!(
            cal.adjust(d(2026, 11, 2), BusinessDayAdjustment::SkipOccurrence),
            Some(d(2026, 11, 2))
        );
    }

    /// FR-05: 調整しない。営業日でない日もそのまま発生日とする。
    #[test]
    fn business_day_adjust_none() {
        let cal = BusinessDayCalendar::new(&[]);
        assert_eq!(
            cal.adjust(d(2026, 11, 1), BusinessDayAdjustment::None),
            Some(d(2026, 11, 1))
        );
    }

    /// 休日をまたいで探索する。2026-11-01 が日曜、2026-11-02 を休日登録すると 2026-11-04 へ。
    /// (2026-11-03 も休日として登録する)
    #[test]
    fn business_day_adjust_skips_consecutive_non_business_days() {
        let cal = BusinessDayCalendar::new(&[
            holiday(2026, 11, 2, "会社休業日"),
            holiday(2026, 11, 3, "文化の日"),
        ]);
        assert_eq!(
            cal.adjust(d(2026, 11, 1), BusinessDayAdjustment::NextBusinessDay),
            Some(d(2026, 11, 4))
        );
    }

    /// EC-23: 休日の登録がない年を判別できる。
    #[test]
    fn calendar_reports_year_without_holidays() {
        let cal = BusinessDayCalendar::new(&[holiday(2026, 11, 3, "文化の日")]);
        assert!(cal.has_holidays_in_year(2026));
        assert!(!cal.has_holidays_in_year(2027));
    }
}
