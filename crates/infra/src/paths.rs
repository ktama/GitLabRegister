//! 保存先の決定 (NFR-09、NFR-11、設計書 5.3)。
//!
//! 保存先は実行する利用者の Windows ユーザープロファイル配下の 1 つのフォルダとする。
//! フォルダをコピーすればバックアップと復元ができる (NFR-11)。

use std::path::{Path, PathBuf};

/// 設定フォルダの名前。
pub const APP_FOLDER_NAME: &str = "GitLabRegister";

/// 設定フォルダ配下のファイル名 (設計書 5.3)。
pub const SETTINGS_FILE: &str = "settings.json";
pub const PROJECTS_FILE: &str = "projects.json";
pub const SCHEDULES_FILE: &str = "schedules.json";
pub const HOLIDAYS_FILE: &str = "holidays.json";
pub const HISTORY_FILE: &str = "history.jsonl";
pub const CANDIDATES_FILE: &str = "candidates.json";
pub const LOGS_DIR: &str = "logs";
/// WebView2 がブラウザのデータを置くフォルダ (NFR-05、NFR-09、設計書 8.3)。
pub const WEBVIEW_DATA_DIR: &str = "webview";
/// 一括登録の実行中を示す印 (EC-08、設計書 7.2)。
pub const IN_PROGRESS_FILE: &str = "registration-in-progress.json";

/// 設定フォルダとその配下のファイルの場所。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

/// 保存先を決められなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsError {
    /// ユーザープロファイルの場所を示す環境変数が読めない。
    UserProfileUnavailable { variable: &'static str },
}

impl std::fmt::Display for PathsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathsError::UserProfileUnavailable { variable } => write!(
                f,
                "設定の保存先を決められません。環境変数 {variable} が読めません"
            ),
        }
    }
}

impl std::error::Error for PathsError {}

impl AppPaths {
    /// Windows のユーザープロファイル配下から設定フォルダを決める。
    ///
    /// `APPDATA` を使う。利用者ごとに分かれ、他の Windows ユーザーから読めない場所であるため
    /// NFR-09 を満たす。
    pub fn from_environment() -> Result<Self, PathsError> {
        let appdata = std::env::var_os("APPDATA").ok_or(PathsError::UserProfileUnavailable {
            variable: "APPDATA",
        })?;
        Ok(Self::with_root(Path::new(&appdata).join(APP_FOLDER_NAME)))
    }

    /// 任意の場所を設定フォルダとして使う。試験と、利用者が場所を指定する場合に使う。
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn settings(&self) -> PathBuf {
        self.root.join(SETTINGS_FILE)
    }

    pub fn projects(&self) -> PathBuf {
        self.root.join(PROJECTS_FILE)
    }

    pub fn schedules(&self) -> PathBuf {
        self.root.join(SCHEDULES_FILE)
    }

    pub fn holidays(&self) -> PathBuf {
        self.root.join(HOLIDAYS_FILE)
    }

    pub fn history(&self) -> PathBuf {
        self.root.join(HISTORY_FILE)
    }

    pub fn candidates(&self) -> PathBuf {
        self.root.join(CANDIDATES_FILE)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join(LOGS_DIR)
    }

    /// WebView2 のユーザーデータフォルダ。
    ///
    /// 設定フォルダ配下に固定することで、ブラウザのデータが
    /// 利用者プロファイルの外や実行ファイルの隣へ出ることを防ぐ (NFR-09)。
    pub fn webview_data_dir(&self) -> PathBuf {
        self.root.join(WEBVIEW_DATA_DIR)
    }

    pub fn in_progress(&self) -> PathBuf {
        self.root.join(IN_PROGRESS_FILE)
    }

    /// EC-28 で設定フォルダの複製を作るときの行き先。
    pub fn backup_root(&self, suffix: &str) -> PathBuf {
        let name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| APP_FOLDER_NAME.to_string());
        match self.root.parent() {
            Some(parent) => parent.join(format!("{name}.backup-{suffix}")),
            None => PathBuf::from(format!("{name}.backup-{suffix}")),
        }
    }

    /// 設定フォルダとログフォルダを作る。既にあれば何もしない。
    pub fn ensure_created(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.logs_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_all_files_live_under_one_root() {
        let paths = AppPaths::with_root(r"C:\dummy\GitLabRegister");
        for p in [
            paths.settings(),
            paths.projects(),
            paths.schedules(),
            paths.holidays(),
            paths.history(),
            paths.candidates(),
            paths.logs_dir(),
            paths.webview_data_dir(),
            paths.in_progress(),
        ] {
            assert!(
                p.starts_with(paths.root()),
                "{p:?} が設定フォルダの外にある (NFR-11 に反する)"
            );
        }
    }

    #[test]
    fn paths_from_environment_is_under_appdata() {
        // この試験は APPDATA が設定された環境でのみ意味を持つ。
        let appdata = std::env::var_os("APPDATA").expect("Windows では APPDATA が設定されている");
        let paths = AppPaths::from_environment().expect("保存先を決められる");
        assert!(paths.root().starts_with(Path::new(&appdata)));
        assert!(paths.root().ends_with(APP_FOLDER_NAME));
    }

    #[test]
    fn paths_ensure_created_makes_root_and_logs() {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let paths = AppPaths::with_root(dir.path().join("GitLabRegister"));
        paths.ensure_created().expect("作成できる");
        assert!(paths.root().is_dir());
        assert!(paths.logs_dir().is_dir());
        // 二度目も失敗しない。
        paths.ensure_created().expect("既存でも成功する");
    }

    /// NFR-05 / NFR-09: WebView2 のデータフォルダが設定フォルダ配下にある。
    #[test]
    fn paths_webview_data_is_under_root() {
        let paths = AppPaths::with_root(r"C:\dummy\GitLabRegister");
        let webview = paths.webview_data_dir();
        assert!(webview.starts_with(paths.root()), "{webview:?}");
        assert!(webview.ends_with(WEBVIEW_DATA_DIR));
    }

    #[test]
    fn paths_backup_root_is_sibling_of_root() {
        let paths = AppPaths::with_root(r"C:\dummy\GitLabRegister");
        let backup = paths.backup_root("20261003");
        assert_eq!(backup.parent(), paths.root().parent());
        assert!(backup
            .file_name()
            .expect("名前がある")
            .to_string_lossy()
            .contains("backup-20261003"));
    }
}
