//! GitLabRegister コア層。
//!
//! 設計書 `docs/design.md` 版 0.2 の ADR-02 に従い、この層は通信・ファイル・時計を
//! 一切参照しない。現在日付は呼び出し側から引数で受け取る (ADR-06)。
//! 依存に `chrono` を既定機能なしで入れているのは、時計へアクセスする API を
//! 型レベルで利用できないようにするためである。
//!
//! 日付は日付のみを持つ型で扱い、時刻とタイムゾーンを計算に持ち込まない。
//! 表示・入力・計算・保存のすべてを日本標準時の日付単位に統一する (NFR-14)。

pub mod business_day;
pub mod expand;
pub mod history;
pub mod model;
pub mod offset;
pub mod recurrence;
pub mod template;
pub mod validator;

pub use business_day::BusinessDayCalendar;
pub use expand::{expand, ExpansionResult};
pub use history::HistoryIndex;
pub use model::*;
