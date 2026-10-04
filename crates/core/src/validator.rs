//! 定義の入力検証 (EC-01、EC-02、EC-06、EC-36、FR-04、FR-14/FR-21)。
//!
//! 文字数はコードポイント数で数え、サロゲートペアで表される文字を 1 文字として扱う (EC-02)。

use crate::model::{RecurrencePattern, Schedule};
use crate::template;

/// GitLab-EE 側の上限 (要件書 第 8 節)。
pub const TITLE_MAX_CHARS: usize = 255;
pub const DESCRIPTION_MAX_CHARS: usize = 1_048_576;
/// 開始日・期限オフセットの範囲 (EC-06)。
pub const OFFSET_MAX_DAYS: u16 = 3_650;
/// N 日ごとの範囲 (FR-04)。
pub const INTERVAL_DAYS_RANGE: (u16, u16) = (1, 365);
/// N 週ごとの範囲 (FR-04)。
pub const INTERVAL_WEEKS_RANGE: (u8, u8) = (1, 52);

/// 検証違反。画面には違反した項目名と理由を一覧で示す (設計書 6.1)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// 違反した項目の名称。
    pub field: &'static str,
    /// 利用者に示す理由。
    pub reason: String,
}

impl Violation {
    fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}

/// コードポイント数 (EC-02)。
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// 定義を検証する。違反がなければ空のベクタを返す。
pub fn validate_schedule(schedule: &Schedule) -> Vec<Violation> {
    let mut v = Vec::new();

    // EC-01: 名称とタイトルが空文字または空白のみ
    if schedule.name.trim().is_empty() {
        v.push(Violation::new("名称", "入力してください"));
    }
    if schedule.issue.title.trim().is_empty() {
        v.push(Violation::new("タイトル", "入力してください"));
    }

    // EC-02: 文字数の上限
    let title_len = char_count(&schedule.issue.title);
    if title_len > TITLE_MAX_CHARS {
        v.push(Violation::new(
            "タイトル",
            format!("{TITLE_MAX_CHARS} 文字以内で入力してください (現在 {title_len} 文字)"),
        ));
    }
    let description_len = char_count(&schedule.issue.description);
    if description_len > DESCRIPTION_MAX_CHARS {
        v.push(Violation::new(
            "説明",
            format!(
                "{DESCRIPTION_MAX_CHARS} 文字以内で入力してください (現在 {description_len} 文字)"
            ),
        ));
    }

    // EC-06: オフセットの範囲
    for (field, value) in [
        ("開始日オフセット", schedule.issue.start_offset_days),
        ("期限オフセット", schedule.issue.due_offset_days),
    ] {
        if let Some(n) = value {
            if n > OFFSET_MAX_DAYS {
                v.push(Violation::new(
                    field,
                    format!("0 以上 {OFFSET_MAX_DAYS} 以下で入力してください"),
                ));
            }
        }
    }

    // EC-36: 開始日オフセットが期限オフセットより大きい
    if let (Some(start), Some(due)) = (
        schedule.issue.start_offset_days,
        schedule.issue.due_offset_days,
    ) {
        if start > due {
            v.push(Violation::new(
                "開始日オフセット",
                format!("開始日が期限より後になります (開始日 {start} 日後、期限 {due} 日後)"),
            ));
        }
    }

    // FR-15: 解釈できない差し込み項目名
    for (field, text) in [
        ("タイトル", &schedule.issue.title),
        ("説明", &schedule.issue.description),
    ] {
        let unknown = template::unknown_placeholders(text);
        if !unknown.is_empty() {
            v.push(Violation::new(
                field,
                format!("解釈できない差し込み項目名: {}", unknown.join(", ")),
            ));
        }
    }

    // FR-14 / FR-21: 有効期間の前後関係
    if !schedule.active_period.is_consistent() {
        v.push(Violation::new(
            "有効期間",
            "終了日は開始日以降を指定してください",
        ));
    }

    v.extend(validate_pattern(&schedule.pattern));
    v
}

/// 繰り返しパターンの検証 (FR-04)。
pub fn validate_pattern(pattern: &RecurrencePattern) -> Vec<Violation> {
    let mut v = Vec::new();
    match pattern {
        RecurrencePattern::Daily | RecurrencePattern::EveryBusinessDay => {}
        RecurrencePattern::EveryNDays { interval, .. } => {
            let (lo, hi) = INTERVAL_DAYS_RANGE;
            if *interval < lo || *interval > hi {
                v.push(Violation::new(
                    "間隔 (日)",
                    format!("{lo} 以上 {hi} 以下で入力してください"),
                ));
            }
        }
        RecurrencePattern::Weekly {
            interval,
            weekdays,
            anchor,
        } => {
            let (lo, hi) = INTERVAL_WEEKS_RANGE;
            if *interval < lo || *interval > hi {
                v.push(Violation::new(
                    "間隔 (週)",
                    format!("{lo} 以上 {hi} 以下で入力してください"),
                ));
            }
            if weekdays.is_empty() {
                v.push(Violation::new("曜日", "1 つ以上選択してください"));
            }
            // FR-04: N が 2 以上の場合は起算日が必須
            if *interval >= 2 && anchor.is_none() {
                v.push(Violation::new(
                    "起算日",
                    "2 週ごと以上の指定では起算日が必要です",
                ));
            }
        }
        RecurrencePattern::MonthlyByDay { day, .. } => {
            if let crate::model::MonthlyDay::Day(n) = day {
                if !(1..=31).contains(n) {
                    v.push(Violation::new("日付", "1 以上 31 以下で指定してください"));
                }
            }
        }
        RecurrencePattern::MonthlyByWeekday { .. } => {}
        RecurrencePattern::Yearly { month, day } => {
            if !(1..=12).contains(month) {
                v.push(Violation::new("月", "1 以上 12 以下で指定してください"));
            }
            if !(1..=31).contains(day) {
                v.push(Violation::new("日", "1 以上 31 以下で指定してください"));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ActivePeriod, BusinessDayAdjustment, IssueTemplate, MissingDayPolicy, MonthlyDay,
        ScheduleId, WeekdayKey,
    };
    use chrono::NaiveDate;
    use std::collections::BTreeSet;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn valid_schedule() -> Schedule {
        Schedule {
            id: ScheduleId("s1".into()),
            name: "月次報告".into(),
            project: "group/project".into(),
            enabled: true,
            active_period: ActivePeriod::default(),
            pattern: RecurrencePattern::MonthlyByDay {
                day: MonthlyDay::Day(25),
                on_missing: MissingDayPolicy::SkipMonth,
            },
            adjustment: BusinessDayAdjustment::None,
            issue: IssueTemplate {
                title: "月次報告".into(),
                ..IssueTemplate::default()
            },
        }
    }

    fn fields(v: &[Violation]) -> Vec<&str> {
        v.iter().map(|x| x.field).collect()
    }

    #[test]
    fn validator_accepts_valid_schedule() {
        assert!(validate_schedule(&valid_schedule()).is_empty());
    }

    /// EC-01: 名称とタイトルが空白のみ。
    #[test]
    fn validator_empty_title_and_name() {
        let s = Schedule {
            name: "   ".into(),
            issue: IssueTemplate {
                title: "".into(),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        let v = validate_schedule(&s);
        assert!(fields(&v).contains(&"名称"));
        assert!(fields(&v).contains(&"タイトル"));
    }

    /// EC-02: タイトル 256 文字は拒否、255 文字は通る。
    #[test]
    fn validator_title_256_chars() {
        let s255 = Schedule {
            issue: IssueTemplate {
                title: "あ".repeat(255),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        assert!(validate_schedule(&s255).is_empty());

        let s256 = Schedule {
            issue: IssueTemplate {
                title: "あ".repeat(256),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        let v = validate_schedule(&s256);
        assert!(fields(&v).contains(&"タイトル"));
        assert!(
            v[0].reason.contains("256"),
            "現在の文字数を示す: {:?}",
            v[0]
        );
    }

    /// EC-02: サロゲートペアで表される文字を 1 文字として数える。
    #[test]
    fn validator_counts_codepoints() {
        // "𠀋" は UTF-16 ではサロゲートペア (2 単位) だがコードポイントは 1。
        let text = "𠀋".repeat(255);
        assert_eq!(char_count(&text), 255);
        let s = Schedule {
            issue: IssueTemplate {
                title: text,
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        assert!(
            validate_schedule(&s).is_empty(),
            "サロゲートペアを 2 文字と数えていない"
        );
    }

    /// EC-02: 説明の上限。
    #[test]
    fn validator_description_limit() {
        let s = Schedule {
            issue: IssueTemplate {
                title: "確認".into(),
                description: "a".repeat(DESCRIPTION_MAX_CHARS + 1),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        assert!(fields(&validate_schedule(&s)).contains(&"説明"));
    }

    /// EC-06: オフセットが範囲外。
    #[test]
    fn validator_offset_out_of_range() {
        let s = Schedule {
            issue: IssueTemplate {
                title: "確認".into(),
                start_offset_days: Some(0),
                due_offset_days: Some(3_651),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        assert!(fields(&validate_schedule(&s)).contains(&"期限オフセット"));
    }

    /// EC-36: 開始日オフセットが期限オフセットより大きい。
    #[test]
    fn validator_start_after_due() {
        let s = Schedule {
            issue: IssueTemplate {
                title: "確認".into(),
                start_offset_days: Some(10),
                due_offset_days: Some(7),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        let v = validate_schedule(&s);
        assert!(fields(&v).contains(&"開始日オフセット"));
        assert!(v
            .iter()
            .any(|x| x.reason.contains("10") && x.reason.contains("7")));
    }

    /// EC-36: 開始日と期限が等しい場合は通る。
    #[test]
    fn validator_start_equals_due_is_allowed() {
        let s = Schedule {
            issue: IssueTemplate {
                title: "確認".into(),
                start_offset_days: Some(7),
                due_offset_days: Some(7),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        assert!(validate_schedule(&s).is_empty());
    }

    /// FR-04: N 日ごと / N 週ごとが範囲外。
    #[test]
    fn validator_interval_out_of_range() {
        for interval in [0_u16, 366] {
            let v = validate_pattern(&RecurrencePattern::EveryNDays {
                interval,
                anchor: d(2026, 10, 1),
            });
            assert!(fields(&v).contains(&"間隔 (日)"), "interval={interval}");
        }
        for interval in [0_u8, 53] {
            let v = validate_pattern(&RecurrencePattern::Weekly {
                interval,
                weekdays: BTreeSet::from([WeekdayKey::Tue]),
                anchor: Some(d(2026, 10, 5)),
            });
            assert!(fields(&v).contains(&"間隔 (週)"), "interval={interval}");
        }
    }

    /// FR-04: 2 週ごとを指定して起算日を入力しない。
    #[test]
    fn validator_weekly_interval_requires_anchor() {
        let v = validate_pattern(&RecurrencePattern::Weekly {
            interval: 2,
            weekdays: BTreeSet::from([WeekdayKey::Tue]),
            anchor: None,
        });
        assert!(fields(&v).contains(&"起算日"));

        // 毎週 (1 週ごと) は起算日を要さない。
        let v1 = validate_pattern(&RecurrencePattern::Weekly {
            interval: 1,
            weekdays: BTreeSet::from([WeekdayKey::Tue]),
            anchor: None,
        });
        assert!(v1.is_empty());
    }

    /// FR-04: 曜日が未選択。
    #[test]
    fn validator_weekly_requires_weekday() {
        let v = validate_pattern(&RecurrencePattern::Weekly {
            interval: 1,
            weekdays: BTreeSet::new(),
            anchor: None,
        });
        assert!(fields(&v).contains(&"曜日"));
    }

    /// FR-15: 解釈できない差し込み項目名を保存時に拒否する。
    #[test]
    fn validator_rejects_unknown_placeholder() {
        let s = Schedule {
            issue: IssueTemplate {
                title: "{{四半期}}期の確認".into(),
                ..IssueTemplate::default()
            },
            ..valid_schedule()
        };
        let v = validate_schedule(&s);
        assert!(fields(&v).contains(&"タイトル"));
        assert!(v.iter().any(|x| x.reason.contains("四半期")));
    }

    /// FR-14 / FR-21: 有効期間の終了日が開始日より前。
    #[test]
    fn validator_active_period_reversed() {
        let s = Schedule {
            active_period: ActivePeriod {
                start: Some(d(2026, 9, 30)),
                end: Some(d(2026, 4, 1)),
            },
            ..valid_schedule()
        };
        assert!(fields(&validate_schedule(&s)).contains(&"有効期間"));
    }
}
