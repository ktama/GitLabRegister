//! GitLab-EE との通信 (ADR-08)。
//!
//! 再試行・待機・タイムアウト・プロキシ・エラー分類の規則をこのモジュールに集める。
//! 呼び出し方式の違い (REST と GraphQL) は外へ出さず、応答は 8 分類へ正規化する (設計書 6.2)。

pub mod client;
pub mod error;
pub mod transport;

pub use client::{Candidate, CandidateKind, Connection, GitLabClient, NewIssue, RetryPolicy, Tier, Version};
pub use error::{ErrorKind, GitLabError};
pub use transport::{HttpRequest, HttpResponse, HttpTransport, Method, ProxyChoice, TransportError};
