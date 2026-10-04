//! 登録履歴の永続化 (ADR-04、FR-11、FR-19、FR-23、EC-08、EC-27)。
//!
//! 1 行 1 件の追記形式で保存する。
//! 登録成功ごとに追記し、1 件ごとにディスクへ確定させる (EC-08、EC-10)。
//! 削除は全体を一時ファイルへ書き直して原子的に置き換える (FR-23)。

use crate::atomic::{write_atomic, StoreError};
use chrono::NaiveDate;
use gitlabregister_core::model::{HistoryEntry, ScheduleId};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 一括登録が実行中であることを示す印の中身 (EC-08、設計書 7.2)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InProgressMark {
    /// 一括登録を開始した時刻。
    pub started_at: chrono::NaiveDateTime,
    /// 登録しようとした件数。
    pub planned_count: usize,
}

/// 読み込み結果。解釈できなかった行を併せて返す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedHistory {
    pub entries: Vec<HistoryEntry>,
    /// 解釈できなかった行の行番号 (1 始まり)。人が手で編集した際の取りこぼしを報告する。
    pub malformed_lines: Vec<usize>,
}

/// 履歴ファイルの読み書き。
#[derive(Debug, Clone)]
pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 履歴をすべて読み込む。ファイルがなければ空を返す (EC-07)。
    ///
    /// 解釈できない行は読み飛ばし、行番号を返す。1 行の破損で全履歴を失わせないためである。
    pub fn load(&self) -> Result<LoadedHistory, StoreError> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedHistory::default())
            }
            Err(e) => {
                return Err(StoreError::Io {
                    path: self.path.clone(),
                    source: e,
                })
            }
        };
        let mut loaded = LoadedHistory::default();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| StoreError::Io {
                path: self.path.clone(),
                source: e,
            })?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<HistoryEntry>(&line) {
                Ok(entry) => loaded.entries.push(entry),
                Err(_) => loaded.malformed_lines.push(index + 1),
            }
        }
        Ok(loaded)
    }

    /// 1 件を追記し、ディスクへ確定させる (ADR-04)。
    pub fn append(&self, entry: &HistoryEntry) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| StoreError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let line = serde_json::to_string(entry).map_err(|e| StoreError::Malformed {
            path: self.path.clone(),
            detail: e.to_string(),
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| StoreError::Io {
                path: self.path.clone(),
                source: e,
            })?;
        writeln!(file, "{line}").map_err(|e| StoreError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        // 1 件ごとに確定させる。強制終了しても追記済みの分は残る (EC-08)。
        file.sync_data().map_err(|e| StoreError::Io {
            path: self.path.clone(),
            source: e,
        })
    }

    /// 指定した (定義 ID, 発生日) の記録を削除する (FR-23、EC-27)。
    ///
    /// 全体を書き直して原子的に置き換える。戻り値は削除した件数。
    pub fn remove(&self, keys: &[(ScheduleId, NaiveDate)]) -> Result<usize, StoreError> {
        let loaded = self.load()?;
        let before = loaded.entries.len();
        let kept: Vec<&HistoryEntry> = loaded
            .entries
            .iter()
            .filter(|e| {
                !keys
                    .iter()
                    .any(|(id, date)| e.schedule_id == *id && e.occurrence_date == *date)
            })
            .collect();
        let removed = before - kept.len();
        if removed == 0 {
            return Ok(0);
        }
        self.rewrite(&kept)?;
        Ok(removed)
    }

    /// 履歴の全件を指定した内容で置き換える。取り込み (FR-18) で使う。
    pub fn replace_all(&self, entries: &[HistoryEntry]) -> Result<(), StoreError> {
        let refs: Vec<&HistoryEntry> = entries.iter().collect();
        self.rewrite(&refs)
    }

    fn rewrite(&self, entries: &[&HistoryEntry]) -> Result<(), StoreError> {
        let mut text = Vec::new();
        for entry in entries {
            let line = serde_json::to_string(entry).map_err(|e| StoreError::Malformed {
                path: self.path.clone(),
                detail: e.to_string(),
            })?;
            text.extend_from_slice(line.as_bytes());
            text.push(b'\n');
        }
        write_atomic(&self.path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use gitlabregister_core::history::HistoryIndex;

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
            issue_reference: format!("#{name}"),
        }
    }

    fn store(dir: &tempfile::TempDir) -> HistoryStore {
        HistoryStore::new(dir.path().join("history.jsonl"))
    }

    /// EC-07: ファイルがなければ空を返す。
    #[test]
    fn history_store_load_missing_file_is_empty() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let loaded = store(&dir).load().expect("失敗しない");
        assert!(loaded.entries.is_empty());
        assert!(loaded.malformed_lines.is_empty());
    }

    /// ADR-04: 1 件ごとの追記で保存され、読み戻せる。
    #[test]
    fn history_store_append_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        s.append(&entry("X", "定義X", d(2026, 10, 12)))
            .expect("追記");
        let loaded = s.load().expect("読める");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].occurrence_date, d(2026, 10, 5));
        assert_eq!(loaded.entries[1].occurrence_date, d(2026, 10, 12));
    }

    /// ADR-04 / FR-18: 1 行 1 件で、人が読める。
    #[test]
    fn history_store_file_is_line_per_entry_and_readable() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "月次報告", d(2026, 10, 5)))
            .expect("追記");
        s.append(&entry("Y", "週次進捗", d(2026, 10, 6)))
            .expect("追記");
        let text = fs::read_to_string(s.path()).expect("読める");
        assert_eq!(text.lines().count(), 2, "1 行 1 件になっていない: {text}");
        assert!(text.contains("月次報告"));
        assert!(text.contains("週次進捗"));
    }

    /// EC-08: 追記済みの分は、以降の処理が中断しても残る。
    ///
    /// プロセスの強制終了は試験できないため、追記のたびにファイルへ反映されていることを確かめる。
    #[test]
    fn history_store_append_is_durable_per_entry() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        // ここで中断したとみなし、別の読み手で確認する。
        let reread = HistoryStore::new(s.path()).load().expect("読める");
        assert_eq!(reread.entries.len(), 1);
    }

    /// FR-23 / EC-27: 指定した記録のみを削除し、他は残す。
    #[test]
    fn history_store_remove_only_specified_entries() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        for date in [d(2026, 10, 5), d(2026, 10, 12), d(2026, 10, 19)] {
            s.append(&entry("X", "定義X", date)).expect("追記");
        }
        let removed = s
            .remove(&[(ScheduleId("X".into()), d(2026, 10, 12))])
            .expect("削除");
        assert_eq!(removed, 1);
        let loaded = s.load().expect("読める");
        assert_eq!(
            loaded
                .entries
                .iter()
                .map(|e| e.occurrence_date)
                .collect::<Vec<_>>(),
            vec![d(2026, 10, 5), d(2026, 10, 19)]
        );
    }

    /// FR-23: 削除した発生日は、索引から外れて再び登録候補になる。
    #[test]
    fn history_store_removed_date_becomes_candidate_again() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        let before = HistoryIndex::build(&s.load().expect("読める").entries);
        assert!(before.is_registered(&ScheduleId("X".into()), d(2026, 10, 5)));

        s.remove(&[(ScheduleId("X".into()), d(2026, 10, 5))])
            .expect("削除");
        let after = HistoryIndex::build(&s.load().expect("読める").entries);
        assert!(!after.is_registered(&ScheduleId("X".into()), d(2026, 10, 5)));
    }

    /// 存在しないキーを指定しても他の記録を壊さない。
    #[test]
    fn history_store_remove_absent_key_changes_nothing() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        let removed = s
            .remove(&[(ScheduleId("Z".into()), d(2026, 1, 1))])
            .expect("削除");
        assert_eq!(removed, 0);
        assert_eq!(s.load().expect("読める").entries.len(), 1);
    }

    /// 1 行が破損しても他の履歴を失わない。破損した行番号を報告する。
    #[test]
    fn history_store_skips_malformed_line_and_reports_it() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        let mut text = fs::read_to_string(s.path()).expect("読める");
        text.push_str("{ this is not json\n");
        text.push_str(
            &serde_json::to_string(&entry("X", "定義X", d(2026, 10, 12))).expect("直列化"),
        );
        text.push('\n');
        fs::write(s.path(), text).expect("書ける");

        let loaded = s.load().expect("読める");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.malformed_lines, vec![2]);
    }

    /// 空行は読み飛ばし、破損として数えない (人が手で編集する前提)。
    #[test]
    fn history_store_ignores_blank_lines() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        let line = serde_json::to_string(&entry("X", "定義X", d(2026, 10, 5))).expect("直列化");
        fs::write(s.path(), format!("\n{line}\n\n")).expect("書ける");
        let loaded = s.load().expect("読める");
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.malformed_lines.is_empty());
    }

    /// FR-18: 取り込みで全件を置き換えられる。
    #[test]
    fn history_store_replace_all() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        s.replace_all(&[entry("Y", "定義Y", d(2026, 11, 1))])
            .expect("置き換え");
        let loaded = s.load().expect("読める");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].schedule_id, ScheduleId("Y".into()));
    }

    /// 削除後も一時ファイルを残さない (設計書 5.3)。
    #[test]
    fn history_store_remove_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let s = store(&dir);
        s.append(&entry("X", "定義X", d(2026, 10, 5)))
            .expect("追記");
        s.remove(&[(ScheduleId("X".into()), d(2026, 10, 5))])
            .expect("削除");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("読める")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "一時ファイルが残っている: {leftovers:?}"
        );
    }
}
