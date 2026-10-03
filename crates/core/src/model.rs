//! ドメイン型。設計書 第 5.1 節のエンティティに対応する。

use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 展開対象の期間。開始日と終了日の両端を含む (FR-07)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl DateRange {
    /// 開始日が終了日より後の場合は `None` を返す (FR-07)。
    pub fn new(start: NaiveDate, end: NaiveDate) -> Option<Self> {
        if start > end {
            None
        } else {
            Some(Self { start, end })
        }
    }

    pub fn contains(&self, date: NaiveDate) -> bool {
        self.start <= date && date <= self.end
    }

    pub fn days(&self) -> impl Iterator<Item = NaiveDate> {
        let (start, end) = (self.start, self.end);
        std::iter::successors(Some(start), move |d| {
            d.succ_opt().filter(|next| *next <= end)
        })
    }
}

/// 定義の有効期間 (FR-21)。両端は任意で、未指定なら期間による制限を受けない。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePeriod {
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
}

impl ActivePeriod {
    pub fn contains(&self, date: NaiveDate) -> bool {
        self.start.is_none_or(|s| s <= date) && self.end.is_none_or(|e| date <= e)
    }

    pub fn is_consistent(&self) -> bool {
        match (self.start, self.end) {
            (Some(s), Some(e)) => s <= e,
            _ => true,
        }
    }
}

/// 毎月 日付指定の対象日 (FR-04 パターン 5)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonthlyDay {
    /// 1 日から 31 日のいずれか。
    Day(u8),
    /// 末日。
    LastDay,
}

/// 指定日がその月に存在しない場合の扱い (FR-04、EC-20)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissingDayPolicy {
    /// その月の末日に繰り上げる。
    ClampToLastDay,
    /// その月は発生させない。
    SkipMonth,
}

/// 毎月 第 N 曜日指定の序数 (FR-04 パターン 6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeekOrdinal {
    First,
    Second,
    Third,
    Fourth,
    Fifth,
}

impl WeekOrdinal {
    pub fn index(self) -> u32 {
        match self {
            WeekOrdinal::First => 0,
            WeekOrdinal::Second => 1,
            WeekOrdinal::Third => 2,
            WeekOrdinal::Fourth => 3,
            WeekOrdinal::Fifth => 4,
        }
    }
}

/// 繰り返しパターン (FR-04 の 7 種類)。
///
/// 列挙型にすることで、パターンを追加したときに処理の取りこぼしが
/// コンパイル時に検出される (ADR-01 の決め手)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecurrencePattern {
    /// 毎日。
    Daily,
    /// 毎営業日。
    EveryBusinessDay,
    /// N 日ごと。N は 1 以上 365 以下。起算日を指定する。
    EveryNDays { interval: u16, anchor: NaiveDate },
    /// 毎週・N 週ごと。N は 1 以上 52 以下。N が 2 以上の場合は起算日が必須 (FR-04)。
    Weekly {
        interval: u8,
        weekdays: BTreeSet<WeekdayKey>,
        anchor: Option<NaiveDate>,
    },
    /// 毎月 日付指定。
    MonthlyByDay {
        day: MonthlyDay,
        on_missing: MissingDayPolicy,
    },
    /// 毎月 第 N 曜日指定。
    MonthlyByWeekday {
        ordinal: WeekOrdinal,
        weekday: WeekdayKey,
    },
    /// 毎年。
    Yearly { month: u8, day: u8 },
}

/// 曜日。`chrono::Weekday` は順序と直列化の都合で直接使わず、この型を介する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WeekdayKey {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl WeekdayKey {
    pub fn from_date(date: NaiveDate) -> Self {
        match date.weekday() {
            Weekday::Mon => WeekdayKey::Mon,
            Weekday::Tue => WeekdayKey::Tue,
            Weekday::Wed => WeekdayKey::Wed,
            Weekday::Thu => WeekdayKey::Thu,
            Weekday::Fri => WeekdayKey::Fri,
            Weekday::Sat => WeekdayKey::Sat,
            Weekday::Sun => WeekdayKey::Sun,
        }
    }

    /// 土曜と日曜は休日カレンダーの登録なしに非営業日として扱う (FR-13)。
    pub fn is_weekend(self) -> bool {
        matches!(self, WeekdayKey::Sat | WeekdayKey::Sun)
    }
}

/// 発生日が営業日でない場合の調整方法 (FR-05 の 4 種類)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BusinessDayAdjustment {
    /// 調整しない。
    None,
    /// 直前の営業日に前倒しする。
    PreviousBusinessDay,
    /// 直後の営業日に後ろ倒しする。
    NextBusinessDay,
    /// その回を発生させない。
    SkipOccurrence,
}

/// 休日 (設計書 5.1 Holiday)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holiday {
    pub date: NaiveDate,
    pub name: String,
}

/// 定義の識別子。不変であり、名称やパターンの変更では変わらない (設計書 5.2、EC-30)。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScheduleId(pub String);

/// 登録する Issue の内容 (FR-06)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueTemplate {
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    /// 発生日からの日数オフセット。未指定は開始日を設定しないことを表す。
    pub start_offset_days: Option<u16>,
    /// 発生日からの日数オフセット。未指定は期限を設定しないことを表す。
    pub due_offset_days: Option<u16>,
    pub confidential: bool,
    pub epic: Option<String>,
    pub iteration: Option<String>,
    pub weight: Option<u32>,
}

/// 繰り返しタスク定義 (設計書 5.1 Schedule)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: ScheduleId,
    pub name: String,
    pub project: String,
    pub enabled: bool,
    pub active_period: ActivePeriod,
    pub pattern: RecurrencePattern,
    pub adjustment: BusinessDayAdjustment,
    pub issue: IssueTemplate,
}

/// 登録履歴の 1 件 (設計書 5.1 HistoryEntry)。
///
/// `schedule_name` は非正規化して持つ。定義を削除しても履歴から名称が読める (EC-26)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub schedule_id: ScheduleId,
    pub schedule_name: String,
    /// 営業日調整後の発生日。重複判定のキーの一部 (設計書 5.2)。
    pub occurrence_date: NaiveDate,
    pub registered_at: NaiveDateTime,
    pub project: String,
    pub issue_reference: String,
}

/// 展開で算出された 1 回分の発生 (設計書 6.1 の登録候補の素)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub schedule_id: ScheduleId,
    /// 営業日調整後の発生日。
    pub date: NaiveDate,
    /// 営業日調整前の発生日。調整の影響を画面で説明するために残す。
    pub original_date: NaiveDate,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
}
