//! 原子的な書き込みと、スキーマ版を持つ文書ファイルの読み書き (設計書 5.3、5.4、EC-28)。
//!
//! 全体置換と原子的置換は、同じフォルダの一時ファイルへ書き切ってから名前を置き換える。
//! 置き換えの途中で中断しても、元のファイルか新しいファイルのどちらかが完全な状態で残る。

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// アプリが扱える文書の版 (EC-28)。
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 文書ファイルの入れ物。先頭にスキーマ版を持つ (設計書 5.4)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document<T> {
    pub schema_version: u32,
    #[serde(flatten)]
    pub body: T,
}

impl<T> Document<T> {
    pub fn current(body: T) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            body,
        }
    }
}

/// 読み書きで起こりうる失敗。
#[derive(Debug)]
pub enum StoreError {
    /// 入出力に失敗した。
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// 形式が解釈できない。
    Malformed { path: PathBuf, detail: String },
    /// アプリが扱える版より新しい (EC-28)。
    SchemaTooNew {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io { path, source } => {
                write!(f, "{} の読み書きに失敗しました: {source}", path.display())
            }
            StoreError::Malformed { path, detail } => {
                write!(f, "{} の形式を解釈できません: {detail}", path.display())
            }
            StoreError::SchemaTooNew {
                path,
                found,
                supported,
            } => write!(
                f,
                "{} は新しい版 ({found}) で保存されています。このアプリが扱えるのは版 {supported} までです",
                path.display()
            ),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl StoreError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// 中身を一時ファイルへ書き切ってから、本来の名前へ原子的に置き換える。
///
/// 一時ファイルは置き換え先と同じフォルダに作る。別のフォルダに作ると
/// 名前の置き換えがフォルダ間移動になり、原子性が失われる。
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
    }
    let temp = temp_path_for(path);
    {
        let mut file = File::create(&temp).map_err(|e| StoreError::io(&temp, e))?;
        file.write_all(contents)
            .map_err(|e| StoreError::io(&temp, e))?;
        // 名前を置き換える前に中身をディスクへ確定させる。
        file.sync_all().map_err(|e| StoreError::io(&temp, e))?;
    }
    fs::rename(&temp, path).map_err(|e| StoreError::io(path, e))
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// スキーマ版付きの文書を整形して書き出す。
///
/// 整形するのは、アプリを使わずに人が読み、テキストエディタで編集できるようにするためである (FR-18)。
pub fn save_document<T: Serialize>(path: &Path, body: &T) -> Result<(), StoreError> {
    let document = Document {
        schema_version: CURRENT_SCHEMA_VERSION,
        body,
    };
    let mut text = serde_json::to_vec_pretty(&document).map_err(|e| StoreError::Malformed {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    text.push(b'\n');
    write_atomic(path, &text)
}

/// スキーマ版付きの文書を読み込む。
///
/// ファイルがなければ `None` を返す。初回起動で設定が 1 件もない状態を呼び出し側が
/// 判別できるようにするためである (EC-07)。
pub fn load_document<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    let text = match fs::read(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreError::io(path, e)),
    };
    let document: Document<T> =
        serde_json::from_slice(&text).map_err(|e| StoreError::Malformed {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    if document.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            path: path.to_path_buf(),
            found: document.schema_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(Some(document.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Body {
        items: Vec<String>,
    }

    fn body() -> Body {
        Body {
            items: vec!["月次報告".into(), "週次進捗".into()],
        }
    }

    /// 設計書 5.3: 置き換え先が既にあっても置き換えられる。
    /// Windows の名前の置き換えが上書きになるかは推測せず、ここで確認する。
    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        write_atomic(&path, b"old").expect("1 回目");
        write_atomic(&path, b"new").expect("2 回目");
        assert_eq!(fs::read(&path).expect("読める"), b"new");
    }

    /// 設計書 5.3: 一時ファイルは置き換え先と同じフォルダに作り、置き換え後は残さない。
    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        write_atomic(&path, b"body").expect("書ける");
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

    #[test]
    fn atomic_write_creates_missing_parent_directory() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("nested").join("deeper").join("doc.json");
        write_atomic(&path, b"body").expect("親フォルダごと作れる");
        assert_eq!(fs::read(&path).expect("読める"), b"body");
    }

    #[test]
    fn schema_save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        save_document(&path, &body()).expect("書ける");
        let loaded: Body = load_document(&path).expect("読める").expect("中身がある");
        assert_eq!(loaded, body());
    }

    /// FR-18: 書き出したファイルは人が読める。定義名が文字列としてそのまま現れる。
    #[test]
    fn schema_saved_file_is_human_readable() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        save_document(&path, &body()).expect("書ける");
        let text = fs::read_to_string(&path).expect("読める");
        assert!(text.contains("月次報告"), "定義名が読めない: {text}");
        // 整形されているので 1 行に詰まっていない。
        assert!(text.lines().count() > 3, "整形されていない: {text}");
        assert!(text.contains("\"schema_version\": 1"), "版がない: {text}");
    }

    /// EC-07: ファイルがなければ `None` を返す。
    #[test]
    fn schema_load_missing_file_is_none() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let loaded: Option<Body> =
            load_document(&dir.path().join("absent.json")).expect("失敗しない");
        assert!(loaded.is_none());
    }

    /// EC-28: アプリが扱える版より新しいファイルは読み込まず、版を示して失敗する。
    #[test]
    fn schema_rejects_newer_version() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        fs::write(
            &path,
            r#"{"schema_version": 99, "items": ["新しい版"]}"#.as_bytes(),
        )
        .expect("書ける");
        let err = load_document::<Body>(&path).expect_err("拒否される");
        match err {
            StoreError::SchemaTooNew {
                found, supported, ..
            } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_SCHEMA_VERSION);
            }
            other => panic!("版の不一致として扱われていない: {other}"),
        }
    }

    /// 形式が解釈できないファイルは、既存のデータを変えずに失敗を返す。
    #[test]
    fn schema_rejects_malformed_file() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        fs::write(&path, b"{ this is not json").expect("書ける");
        let err = load_document::<Body>(&path).expect_err("拒否される");
        assert!(matches!(err, StoreError::Malformed { .. }), "{err}");
        // ファイルは触られていない。
        assert_eq!(fs::read(&path).expect("読める"), b"{ this is not json");
    }

    /// 古い版のファイルは読み込める (現行版へ変換して扱う前提、設計書 5.4)。
    #[test]
    fn schema_accepts_current_version() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let path = dir.path().join("doc.json");
        fs::write(&path, br#"{"schema_version": 1, "items": []}"#).expect("書ける");
        let loaded: Body = load_document(&path).expect("読める").expect("中身がある");
        assert!(loaded.items.is_empty());
    }
}
