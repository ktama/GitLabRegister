//! 壁時計の読み取り (NFR-14、ADR-06)。
//!
//! コア層は時計を読めない。`chrono` を既定機能なしで入れているため、
//! 現在時刻を取る API が型として存在しない。現在日付と現在時刻を得るのは
//! この外殻の役目であり、取った値は引数としてコア層へ渡す。
//!
//! 時刻帯は日本標準時に固定する (NFR-14)。日本は夏時間を持たないため、
//! 固定のオフセットで足りる。OS の時刻帯設定には従わない。
//! 従うようにすると、端末の設定を変えただけで発生日の算出結果が変わりうる。

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeDelta};
use std::time::{SystemTime, UNIX_EPOCH};

/// 日本標準時のオフセット (時)。
const JST_OFFSET_HOURS: i64 = 9;

/// 現在の日本標準時。
pub fn now() -> NaiveDateTime {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // 端末の時計が 1970 年より前に設定されている場合は紀元を使う。
        .unwrap_or(0);
    DateTime::from_timestamp(seconds, 0)
        .map(|utc| utc.naive_utc() + TimeDelta::hours(JST_OFFSET_HOURS))
        .unwrap_or_else(epoch)
}

/// 現在の日本標準時の日付。展開の基準日に使う (EC-24)。
pub fn today() -> NaiveDate {
    now().date()
}

fn epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("紀元は常に有効な日時")
}
