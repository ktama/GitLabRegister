//! 発生日から開始日と期限を算出する (FR-06、EC-36)。

use chrono::{Days, NaiveDate};

/// 開始日と期限。どちらも未指定を取りうる (FR-06)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueDates {
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
}

/// 営業日調整後の発生日にオフセット日数を加算して開始日と期限を算出する。
///
/// 開始日と期限そのものには営業日調整を適用しない (FR-06)。
pub fn calculate(
    occurrence_date: NaiveDate,
    start_offset_days: Option<u16>,
    due_offset_days: Option<u16>,
) -> IssueDates {
    IssueDates {
        start_date: start_offset_days.and_then(|n| add_days(occurrence_date, n)),
        due_date: due_offset_days.and_then(|n| add_days(occurrence_date, n)),
    }
}

fn add_days(date: NaiveDate, days: u16) -> Option<NaiveDate> {
    date.checked_add_days(Days::new(u64::from(days)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    /// FR-06: 開始日オフセット 0、期限オフセット 7。発生日 2026-10-05。
    #[test]
    fn offset_start0_due7() {
        let got = calculate(d(2026, 10, 5), Some(0), Some(7));
        assert_eq!(got.start_date, Some(d(2026, 10, 5)));
        assert_eq!(got.due_date, Some(d(2026, 10, 12)));
    }

    /// FR-06: 開始日オフセット 3、期限オフセット 7。
    #[test]
    fn offset_start3_due7() {
        let got = calculate(d(2026, 10, 5), Some(3), Some(7));
        assert_eq!(got.start_date, Some(d(2026, 10, 8)));
        assert_eq!(got.due_date, Some(d(2026, 10, 12)));
    }

    /// FR-06: 開始日のみを 3 に設定し期限は未入力。
    #[test]
    fn offset_start_only() {
        let got = calculate(d(2026, 10, 5), Some(3), None);
        assert_eq!(got.start_date, Some(d(2026, 10, 8)));
        assert_eq!(got.due_date, None);
    }

    /// FR-06: 期限オフセットを 0 に設定すると発生日と同日。
    #[test]
    fn offset_due0_is_same_day() {
        let got = calculate(d(2026, 10, 5), None, Some(0));
        assert_eq!(got.due_date, Some(d(2026, 10, 5)));
    }

    /// FR-06: どちらも未指定なら日付を設定しない。
    #[test]
    fn offset_none() {
        let got = calculate(d(2026, 10, 5), None, None);
        assert_eq!(got.start_date, None);
        assert_eq!(got.due_date, None);
    }

    /// 月と年をまたぐ加算。
    #[test]
    fn offset_crosses_month_and_year() {
        let got = calculate(d(2026, 12, 28), Some(0), Some(10));
        assert_eq!(got.due_date, Some(d(2027, 1, 7)));
    }
}
