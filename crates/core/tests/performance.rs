//! 性能要件の計測 (NFR-01、NFR-04、設計書 第 10 節 R-4)。
//!
//! 設計書 第 8.4 節の見積もりを実測に置き換えるための試験である。
//! 通信を含まないコア層の計算のみを対象とする。

use chrono::{Days, NaiveDate, NaiveDateTime};
use gitlabregister_core::business_day::BusinessDayCalendar;
use gitlabregister_core::expand::{expand, MAX_CANDIDATES};
use gitlabregister_core::history::HistoryIndex;
use gitlabregister_core::model::*;
use std::collections::BTreeSet;
use std::time::Instant;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
}

fn dt(date: NaiveDate) -> NaiveDateTime {
    date.and_hms_opt(9, 0, 0).expect("有効な時刻")
}

/// 多様なパターンを混ぜた定義を n 件作る。
fn schedules(n: usize) -> Vec<Schedule> {
    (0..n)
        .map(|i| {
            let pattern = match i % 7 {
                0 => RecurrencePattern::Daily,
                1 => RecurrencePattern::EveryBusinessDay,
                2 => RecurrencePattern::EveryNDays {
                    interval: 3,
                    anchor: d(2026, 1, 1),
                },
                3 => RecurrencePattern::Weekly {
                    interval: 2,
                    weekdays: BTreeSet::from([WeekdayKey::Tue, WeekdayKey::Fri]),
                    anchor: Some(d(2026, 10, 5)),
                },
                4 => RecurrencePattern::MonthlyByDay {
                    day: MonthlyDay::Day(31),
                    on_missing: MissingDayPolicy::ClampToLastDay,
                },
                5 => RecurrencePattern::MonthlyByWeekday {
                    ordinal: WeekOrdinal::Second,
                    weekday: WeekdayKey::Tue,
                },
                _ => RecurrencePattern::Yearly { month: 4, day: 1 },
            };
            Schedule {
                id: ScheduleId(format!("s{i}")),
                name: format!("定義{i}"),
                project: "group/project".into(),
                enabled: true,
                active_period: ActivePeriod::default(),
                pattern,
                adjustment: BusinessDayAdjustment::NextBusinessDay,
                issue: IssueTemplate {
                    title: "{{年}}年{{月}}月{{日}}日 {{定義名}}".into(),
                    description: "期限 {{期限}} / 開始 {{開始日}}".into(),
                    start_offset_days: Some(0),
                    due_offset_days: Some(7),
                    ..IssueTemplate::default()
                },
            }
        })
        .collect()
}

fn holidays(count: usize) -> Vec<Holiday> {
    let base = d(2020, 1, 1);
    (0..count)
        .map(|i| Holiday {
            date: base + Days::new(i as u64 * 11),
            name: format!("休業日{i}"),
        })
        .collect()
}

/// 一意な (定義 ID, 発生日) を `count` 件作る。
///
/// 定義 ID と発生日の剰余が相関すると索引が潰れて件数を測れないため、
/// 定義 ID は商、発生日は剰余から作って積が `count` になるようにする。
/// 発生日は 2025-06-01 から 500 日分 (2026-10-13 まで) とする。
/// 展開の試験期間 2026-10-01〜2026-12-31 のうち前半だけが登録済みになるため、
/// 除外される発生日と登録候補になる発生日の双方が計測に含まれる。
const HISTORY_DATES_PER_SCHEDULE: usize = 500;

fn history(count: usize) -> Vec<HistoryEntry> {
    let base = d(2025, 6, 1);
    (0..count)
        .map(|i| {
            let schedule_index = i / HISTORY_DATES_PER_SCHEDULE;
            let date = base + Days::new((i % HISTORY_DATES_PER_SCHEDULE) as u64);
            HistoryEntry {
                schedule_id: ScheduleId(format!("s{schedule_index}")),
                schedule_name: format!("定義{schedule_index}"),
                occurrence_date: date,
                registered_at: dt(date),
                project: "group/project".into(),
                issue_reference: format!("#{i}"),
            }
        })
        .collect()
}

/// NFR-01: 定義 50 件・期間 92 日の展開が、通信を含まない状態で 3 秒以内。
#[test]
fn nfr01_expand_50_schedules_over_92_days_within_3s() {
    let schedules = schedules(50);
    let holidays = holidays(1_000);
    let index = HistoryIndex::build(&history(100_000));
    // 2026-10-01 から 92 日後は 2026-12-31。
    let range = DateRange::new(d(2026, 10, 1), d(2026, 12, 31)).expect("有効な期間");

    let started = Instant::now();
    let result = expand(&schedules, &holidays, &index, &range);
    let elapsed = started.elapsed();

    println!(
        "NFR-01 実測: {:?} / 登録候補 {} 件 / 除外 {} 件",
        elapsed,
        result.candidates.len(),
        result.excluded.len()
    );
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "NFR-01 (3 秒以内) を超えた: {elapsed:?}"
    );
    assert!(!result.candidates.is_empty());
    assert!(
        !result.excluded.is_empty(),
        "履歴による除外の経路を通っていない。計測が軽く出る"
    );
}

/// NFR-04 / 設計書 R-4: 履歴 100,000 件の索引構築が NFR-01 の予算を圧迫しない。
#[test]
fn nfr04_history_index_build_100k_within_1s() {
    let entries = history(100_000);
    let started = Instant::now();
    let index = HistoryIndex::build(&entries);
    let elapsed = started.elapsed();

    println!(
        "R-4 実測: 索引構築 {:?} / 一意キー {} 件",
        elapsed,
        index.len()
    );
    assert_eq!(
        index.len(),
        100_000,
        "索引が一意キーに潰れている。計測対象が 100,000 件になっていない"
    );
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "履歴 100,000 件の索引構築が 1 秒を超えた: {elapsed:?}"
    );
}

/// NFR-04: 定義 200 件を保持した状態でも NFR-01 の条件を満たす。
#[test]
fn nfr04_expand_200_schedules_within_3s() {
    let schedules = schedules(200);
    let holidays = holidays(1_000);
    let index = HistoryIndex::build(&history(100_000));
    let range = DateRange::new(d(2026, 10, 1), d(2026, 12, 31)).expect("有効な期間");

    let started = Instant::now();
    let result = expand(&schedules, &holidays, &index, &range);
    let elapsed = started.elapsed();

    println!(
        "NFR-04 実測: {:?} / 登録候補 {} 件",
        elapsed,
        result.candidates.len()
    );
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "定義 200 件で NFR-01 (3 秒以内) を超えた: {elapsed:?}"
    );
    // 上限超過の警告が立ち、一括登録は実行できない (EC-05)。
    assert!(result.candidates.len() > MAX_CANDIDATES);
    assert!(!result.can_register());
}

/// 営業日判定が繰り返し呼ばれても線形に収まることの確認。
#[test]
fn business_day_lookup_is_constant_time_enough() {
    let calendar = BusinessDayCalendar::new(&holidays(1_000));
    let started = Instant::now();
    let mut business_days = 0;
    let mut cursor = d(2026, 1, 1);
    for _ in 0..100_000 {
        if calendar.is_business_day(cursor) {
            business_days += 1;
        }
        cursor = cursor.succ_opt().expect("有効な日付");
    }
    let elapsed = started.elapsed();
    println!("営業日判定 100,000 回: {elapsed:?} / 営業日 {business_days} 件");
    assert!(elapsed.as_secs_f64() < 1.0, "営業日判定が遅い: {elapsed:?}");
}
