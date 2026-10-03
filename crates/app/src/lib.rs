//! GitLabRegister アプリ本体のサービス層とコマンド境界 (設計書 4.1、4.2、6.1)。
//!
//! コア層の計算とインフラ層の入出力を組み合わせ、画面が呼ぶ単位にまとめる。
//! 秘密情報は SecretHolder が持ち、この層は写しを持たない (NFR-05)。

pub mod boundary;
pub mod candidate;
pub mod connection;
pub mod expansion;
pub mod masterdata;
pub mod registration;
pub mod schedule;

#[cfg(test)]
mod testing;

pub use boundary::{Boundary, Failure, FailureKind};
pub use candidate::{CandidateList, CandidateService, Freshness};
pub use connection::{ConnectError, ConnectOutcome, ConnectionService, ConnectionState};
pub use expansion::{ExpandError, ExpansionService, Preview, PreviewWarning};
pub use masterdata::{HistoryFilter, HolidayError, MasterDataService};
pub use registration::{
    AbortReason, BatchOutcome, CancellationToken, ItemOutcome, ItemStatus, Progress,
    RegistrationItem, RegistrationService,
};
pub use schedule::{DeleteConfirmation, PatternChangeConfirmation, SaveError, ScheduleService};
