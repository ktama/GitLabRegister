//! 登録履歴による重複判定 (FR-11、FR-19、FR-23、EC-26、EC-30)。
//!
//! 重複判定のキーは (定義 ID, 営業日調整後の発生日) とする (設計書 5.2)。
//! 定義 ID は不変であるため、名称やパターンを変更しても過去の履歴との対応は保たれる (EC-30)。

use crate::model::{HistoryEntry, ScheduleId};
use chrono::NaiveDate;
use std::collections::HashSet;

/// 登録済み判定のための索引。
///
/// 設計書 8.4 のとおり、履歴 100,000 件でも (定義 ID, 発生日) の集合に収まる。
#[derive(Debug, Clone, Default)]
pub struct HistoryIndex {
    registered: HashSet<(ScheduleId, NaiveDate)>,
}

impl HistoryIndex {
    pub fn build(entries: &[HistoryEntry]) -> Self {
        Self {
            registered: entries
                .iter()
                .map(|e| (e.schedule_id.clone(), e.occurrence_date))
                .collect(),
        }
    }

    pub fn is_registered(&self, schedule_id: &ScheduleId, date: NaiveDate) -> bool {
        self.registered.contains(&(schedule_id.clone(), date))
    }

    pub fn len(&self) -> usize {
        self.registered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registered.is_empty()
    }

    /// 登録成功した発生日を索引に加える。一括登録中の進行に合わせて呼ぶ (FR-09、EC-08)。
    pub fn record(&mut self, schedule_id: &ScheduleId, date: NaiveDate) {
        self.registered.insert((schedule_id.clone(), date));
    }

    /// 履歴の個別削除に合わせて索引から外す (FR-23)。
    pub fn forget(&mut self, schedule_id: &ScheduleId, date: NaiveDate) {
        self.registered.remove(&(schedule_id.clone(), date));
    }

    /// 発生日の一覧を、登録済みのものと未登録のものに分ける (FR-11)。
    ///
    /// 戻り値は (未登録 = 登録候補になる発生日, 登録済み = 除外された発生日)。
    pub fn partition(
        &self,
        schedule_id: &ScheduleId,
        dates: &[NaiveDate],
    ) -> (Vec<NaiveDate>, Vec<NaiveDate>) {
        dates
            .iter()
            .copied()
            .partition(|d| !self.is_registered(schedule_id, *d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("有効な日付")
    }

    fn entry(id: &str, name: &str, date: NaiveDate) -> HistoryEntry {
        HistoryEntry {
            schedule_id: ScheduleId(id.into()),
            schedule_name: name.into(),
            occurrence_date: date,
            registered_at: date.and_hms_opt(9, 0, 0).expect("有効な時刻"),
            project: "group/project".into(),
            issue_reference: "#1".into(),
        }
    }

    /// FR-11: 登録済みの発生日は登録候補に含まれない。
    #[test]
    fn history_excludes_registered() {
        let index = HistoryIndex::build(&[entry("X", "定義X", d(2026, 10, 5))]);
        let (candidates, excluded) =
            index.partition(&ScheduleId("X".into()), &[d(2026, 10, 5), d(2026, 10, 12)]);
        assert_eq!(candidates, vec![d(2026, 10, 12)]);
        assert_eq!(excluded, vec![d(2026, 10, 5)]);
    }

    /// FR-11: 定義 X のみ登録済みなら、定義 Y の同じ発生日は登録候補に残る。
    #[test]
    fn history_keeps_other_schedule() {
        let index = HistoryIndex::build(&[entry("X", "定義X", d(2026, 10, 5))]);
        let (candidates, excluded) = index.partition(&ScheduleId("Y".into()), &[d(2026, 10, 5)]);
        assert_eq!(candidates, vec![d(2026, 10, 5)]);
        assert!(excluded.is_empty());
    }

    /// FR-23: 履歴を削除した発生日は再び登録候補になる。
    #[test]
    fn history_forget_makes_date_candidate_again() {
        let mut index = HistoryIndex::build(&[entry("X", "定義X", d(2026, 10, 5))]);
        assert!(index.is_registered(&ScheduleId("X".into()), d(2026, 10, 5)));
        index.forget(&ScheduleId("X".into()), d(2026, 10, 5));
        let (candidates, _) = index.partition(&ScheduleId("X".into()), &[d(2026, 10, 5)]);
        assert_eq!(candidates, vec![d(2026, 10, 5)]);
    }

    /// FR-09: 一括登録の進行に合わせて索引へ加えると、同じ発生日は除外される。
    #[test]
    fn history_record_excludes_on_next_expansion() {
        let mut index = HistoryIndex::default();
        let id = ScheduleId("X".into());
        index.record(&id, d(2026, 10, 5));
        let (candidates, excluded) = index.partition(&id, &[d(2026, 10, 5)]);
        assert!(candidates.is_empty());
        assert_eq!(excluded, vec![d(2026, 10, 5)]);
    }

    /// EC-30: 定義 ID が不変なため、名称やパターンを変えても履歴の対応は保たれる。
    #[test]
    fn history_key_survives_pattern_change() {
        // 履歴に残る定義名は登録時点のもの。索引のキーは定義 ID のみに依存する。
        let index = HistoryIndex::build(&[entry("X", "旧名称", d(2026, 10, 5))]);
        assert!(index.is_registered(&ScheduleId("X".into()), d(2026, 10, 5)));
    }

    /// EC-26: 定義を削除しても履歴から定義名が読める (非正規化して保持)。
    #[test]
    fn history_entry_keeps_schedule_name() {
        let entries = [entry("X", "削除された定義", d(2026, 10, 5))];
        assert_eq!(entries[0].schedule_name, "削除された定義");
    }

    /// 同じキーを二重に記録しても 1 件として扱う。
    #[test]
    fn history_duplicate_entries_collapse() {
        let index = HistoryIndex::build(&[
            entry("X", "定義X", d(2026, 10, 5)),
            entry("X", "定義X", d(2026, 10, 5)),
        ]);
        assert_eq!(index.len(), 1);
    }
}
