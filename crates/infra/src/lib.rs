//! GitLabRegister インフラ層。
//!
//! 設計書 `docs/design.md` 版 0.2 の ADR-03 / ADR-04 に従い、永続化はすべてテキストファイルで行う。
//! 全体置換と原子的置換は一時ファイル経由とし、履歴の追記は 1 件ごとにディスクへ確定させる。
//! 秘密情報はプロセスメモリ上の SecretHolder だけが持ち、Logger へ渡る経路を作らない (NFR-05、NFR-07)。

pub mod atomic;
pub mod gitlab;
pub mod history_store;
pub mod logger;
pub mod paths;
pub mod secret;
pub mod store;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use atomic::{StoreError, CURRENT_SCHEMA_VERSION};
pub use gitlab::{ErrorKind, GitLabError};
pub use history_store::{HistoryStore, InProgressMark, LoadedHistory};
pub use logger::{Level, LogRecord, Logger};
pub use paths::{AppPaths, PathsError};
pub use secret::{ProxyCredentials, SecretHolder};
pub use store::{
    AppSettings, CandidateCacheEntry, ConfigStore, ImportConflictPolicy, Project, ProxyMode,
    ProxySettings,
};
