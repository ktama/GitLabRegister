//! GitLabClient 本体 (ADR-08)。
//!
//! 再試行・待機・分類の規則をここに集める。
//! 通信の実体は `HttpTransport` に委ね、判断だけを持つ。

use super::error::{classify_status, extract_message, ErrorKind, GitLabError};
use super::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use chrono::NaiveDate;
use serde::Deserialize;
use std::time::Duration;

/// 対応する GitLab-EE の下限 (NFR-13)。
pub const MINIMUM_VERSION: (u32, u32) = (19, 2);

/// 再試行の規則 (EC-17、EC-18)。
///
/// 設計書 6.2 は一時エラーを 2 秒後と 4 秒後に最大 2 回、
/// 要求上限を待機時間の指示に従い指示がなければ 60 秒待って 1 回と定める。
/// 試験では待機を 0 にして規則だけを検証する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 一時エラーの再試行間隔。要素数が再試行の上限になる。
    pub transient_backoff: Vec<Duration>,
    /// 要求上限で待機時間の指示がない場合の待機時間。
    pub rate_limit_default_wait: Duration,
    /// 要求上限の再試行回数。
    pub rate_limit_attempts: usize,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            transient_backoff: vec![Duration::from_secs(2), Duration::from_secs(4)],
            rate_limit_default_wait: Duration::from_secs(60),
            rate_limit_attempts: 1,
        }
    }
}

impl RetryPolicy {
    /// 待機なしの規則。試験で使う。
    pub fn without_waiting() -> Self {
        Self {
            transient_backoff: vec![Duration::ZERO, Duration::ZERO],
            rate_limit_default_wait: Duration::ZERO,
            rate_limit_attempts: 1,
        }
    }
}

/// 接続確認の結果 (FR-01、EC-35)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// 認証された GitLab-EE 上の利用者名 (FR-01)。
    pub username: String,
    pub version: Version,
    pub tier: Tier,
}

/// GitLab-EE のバージョン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// 下限を満たすか (NFR-13、EC-35)。
    pub fn is_supported(&self) -> bool {
        (self.major, self.minor) >= MINIMUM_VERSION
    }

    fn parse(text: &str) -> Option<Self> {
        // GitLab は "19.2.1" や "19.2.1-ee" の形で返す。
        let core = text.split('-').next()?;
        let mut parts = core.split('.');
        Some(Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next().unwrap_or("0").parse().unwrap_or(0),
            patch: parts.next().unwrap_or("0").parse().unwrap_or(0),
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// ライセンスティアの判定結果 (EC-15)。
///
/// # 判定できない場合について
///
/// ティアを直接示す情報は管理者権限がないと取得できない。
/// 権限がない利用者では `Unknown` になる。
/// 要件書 第 3 節は Premium 以上を前提としているため、`Unknown` のときは
/// EE 固有項目を使える前提で進め、登録が失敗したら EC-15 の扱いに落とす。
/// この方針は設計書 第 10 節 R-5 の確認待ちである。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// EE 固有項目を使える。
    PremiumOrAbove,
    /// EE 固有項目を使えない。
    Free,
    /// 判定できなかった。
    Unknown,
}

impl Tier {
    /// EE 固有項目の入力を許すか (EC-15)。
    pub fn allows_ee_fields(self) -> bool {
        matches!(self, Tier::PremiumOrAbove | Tier::Unknown)
    }
}

/// 作成する Issue の内容 (FR-06)。
///
/// イテレーションは含めない。GitLab-EE が作成時に受け付けないため、
/// 別の操作で設定する (ADR-05)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewIssue {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
    pub assignee_ids: Vec<u64>,
    pub milestone: Option<String>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub confidential: bool,
    pub epic_id: Option<u64>,
    pub weight: Option<u32>,
}

/// 作成された Issue (FR-10 の結果一覧に出す識別情報)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedIssue {
    /// プロジェクト内の番号。
    pub iid: u64,
    /// GitLab 全体での識別子。イテレーション設定に使う。
    pub id: u64,
    pub web_url: String,
}

/// 候補値の種別 (FR-16)。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum CandidateKind {
    Label,
    Member,
    Milestone,
    Epic,
    Iteration,
}

/// 候補値 1 件。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// イテレーションの期間。発生日を含むイテレーションを選ぶのに使う (計画 9)。
    /// 期間を持たない種別と、期間を保持する前の保持データでは空になる。
    #[serde(default)]
    pub start_date: Option<NaiveDate>,
    #[serde(default)]
    pub due_date: Option<NaiveDate>,
}

/// GitLab-EE との通信。
#[derive(Debug)]
pub struct GitLabClient<T> {
    transport: T,
    retry: RetryPolicy,
}

impl<T: HttpTransport> GitLabClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(transport: T, retry: RetryPolicy) -> Self {
        Self { transport, retry }
    }

    /// 通信口を参照する。送られた要求を確かめる試験で使う。
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// 接続確認を行い、利用者名・バージョン・ティアを得る (FR-01、EC-35)。
    ///
    /// バージョンが下限を満たさない場合も結果を返す。
    /// 登録を実行しない判断は呼び出し側が `Version::is_supported` で行う。
    pub async fn connect(&self) -> Result<Connection, GitLabError> {
        #[derive(Deserialize)]
        struct User {
            username: String,
        }
        #[derive(Deserialize)]
        struct VersionBody {
            version: String,
        }

        let user: User = self
            .send_json(HttpRequest::get("/api/v4/user"), "接続確認")
            .await?;
        let version_body: VersionBody = self
            .send_json(HttpRequest::get("/api/v4/version"), "バージョン取得")
            .await?;
        let version = Version::parse(&version_body.version).ok_or_else(|| {
            GitLabError::new(
                ErrorKind::Validation,
                "バージョン取得",
                format!("バージョンを解釈できません: {}", version_body.version),
            )
        })?;
        Ok(Connection {
            username: user.username,
            version,
            tier: self.probe_tier().await,
        })
    }

    /// ライセンスティアを調べる (EC-15)。
    ///
    /// 管理者権限がない場合は判定できず `Unknown` を返す。失敗を呼び出し側へ伝えない。
    async fn probe_tier(&self) -> Tier {
        #[derive(Deserialize)]
        struct License {
            plan: String,
        }
        match self
            .send_json::<License>(HttpRequest::get("/api/v4/license"), "ティア判定")
            .await
        {
            Ok(license) => match license.plan.to_ascii_lowercase().as_str() {
                "premium" | "ultimate" | "silver" | "gold" => Tier::PremiumOrAbove,
                "free" | "core" | "starter" | "bronze" => Tier::Free,
                _ => Tier::Unknown,
            },
            // 権限不足や未提供は判定できないものとして扱う。
            Err(_) => Tier::Unknown,
        }
    }

    /// Issue を 1 件作成する (FR-06、FR-09、FR-12)。
    pub async fn create_issue(&self, issue: &NewIssue) -> Result<CreatedIssue, GitLabError> {
        #[derive(Deserialize)]
        struct Created {
            id: u64,
            iid: u64,
            web_url: String,
        }
        let path = format!("/api/v4/projects/{}/issues", urlencode(&issue.project_id));
        let created: Created = self
            .send_json(HttpRequest::post(path, issue_body(issue)), "Issue の作成")
            .await?;
        Ok(CreatedIssue {
            id: created.id,
            iid: created.iid,
            web_url: created.web_url,
        })
    }

    /// 作成済みの Issue にイテレーションを設定する (ADR-05)。
    ///
    /// GitLab-EE は Issue の作成でイテレーションを受け付けないため、別の操作になる。
    /// 失敗しても Issue の作成は成功として扱う判断は呼び出し側が行う (FR-06)。
    pub async fn set_iteration(
        &self,
        issue_id: u64,
        iteration_id: &str,
    ) -> Result<(), GitLabError> {
        let query = r#"mutation($issue: IssuableID!, $iteration: IterationID!) {
  issueSetIteration(input: { id: $issue, iterationId: $iteration }) {
    errors
  }
}"#;
        let body = serde_json::json!({
            "query": query,
            "variables": {
                "issue": format!("gid://gitlab/Issue/{issue_id}"),
                "iteration": iteration_id,
            }
        });
        let response = self
            .send(
                HttpRequest::post("/api/graphql", body),
                "イテレーション設定",
            )
            .await?;

        // GraphQL は状態コード 200 でも本文にエラーを入れる。
        // 設計書 6.2 の分類へ落とすため、本文を検査する。
        let value: serde_json::Value = serde_json::from_str(&response.body).map_err(|e| {
            GitLabError::new(
                ErrorKind::Validation,
                "イテレーション設定",
                format!("応答を解釈できません: {e}"),
            )
        })?;
        if let Some(errors) = value.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                return Err(GitLabError::new(
                    classify_graphql_errors(errors),
                    "イテレーション設定",
                    extract_message(&response.body),
                ));
            }
        }
        let mutation_errors = value
            .pointer("/data/issueSetIteration/errors")
            .and_then(|e| e.as_array());
        if let Some(errors) = mutation_errors {
            if !errors.is_empty() {
                return Err(GitLabError::new(
                    ErrorKind::Validation,
                    "イテレーション設定",
                    extract_message(&response.body),
                ));
            }
        }
        Ok(())
    }

    /// プロジェクトを 1 件取得する (FR-02、EC-29)。
    pub async fn fetch_project(&self, project_id: &str) -> Result<Candidate, GitLabError> {
        #[derive(Deserialize)]
        struct ProjectBody {
            id: u64,
            path_with_namespace: String,
            #[serde(default)]
            archived: bool,
        }
        let path = format!("/api/v4/projects/{}", urlencode(project_id));
        let project: ProjectBody = self
            .send_json(HttpRequest::get(path), "プロジェクトの取得")
            .await?;
        if project.archived {
            return Err(GitLabError::new(
                ErrorKind::NotFound,
                "プロジェクトの取得",
                format!(
                    "プロジェクト {} はアーカイブされています",
                    project.path_with_namespace
                ),
            ));
        }
        Ok(Candidate {
            id: project.id.to_string(),
            name: project.path_with_namespace,
            ..Candidate::default()
        })
    }

    /// 候補値の一覧を取得する (FR-16)。
    pub async fn fetch_candidates(
        &self,
        project_id: &str,
        kind: CandidateKind,
    ) -> Result<Vec<Candidate>, GitLabError> {
        let encoded = urlencode(project_id);
        let path = match kind {
            CandidateKind::Label => format!("/api/v4/projects/{encoded}/labels"),
            CandidateKind::Member => format!("/api/v4/projects/{encoded}/members/all"),
            CandidateKind::Milestone => format!("/api/v4/projects/{encoded}/milestones"),
            CandidateKind::Epic => return self.fetch_epic_candidates(&encoded).await,
            CandidateKind::Iteration => format!("/api/v4/projects/{encoded}/iterations"),
        };
        let request = HttpRequest::get(path).with_query("per_page", "100");
        let items: Vec<serde_json::Value> = self.send_json(request, "候補の取得").await?;
        Ok(items.iter().filter_map(|i| to_candidate(i, kind)).collect())
    }

    /// エピックの候補 (FR-16、計画 10)。
    ///
    /// GitLab のエピックはグループの機能で、プロジェクト単位の取得口はない。
    /// プロジェクトの属するグループを調べ、そのグループと上位グループのエピックを取る。
    /// 個人の名前空間に置かれたプロジェクトにはエピックがないため、2 回目の通信をせずに 0 件を返す。
    async fn fetch_epic_candidates(&self, encoded: &str) -> Result<Vec<Candidate>, GitLabError> {
        #[derive(Deserialize)]
        struct Namespace {
            id: u64,
            kind: String,
        }
        #[derive(Deserialize)]
        struct ProjectBody {
            namespace: Namespace,
        }
        let project: ProjectBody = self
            .send_json(
                HttpRequest::get(format!("/api/v4/projects/{encoded}")),
                "候補の取得",
            )
            .await?;
        if project.namespace.kind != "group" {
            return Ok(Vec::new());
        }
        let request = HttpRequest::get(format!("/api/v4/groups/{}/epics", project.namespace.id))
            .with_query("include_ancestor_groups", "true")
            .with_query("per_page", "100");
        let items: Vec<serde_json::Value> = self.send_json(request, "候補の取得").await?;
        Ok(items
            .iter()
            .filter_map(|i| to_candidate(i, CandidateKind::Epic))
            .collect())
    }

    /// 応答を型へ読み取る。
    async fn send_json<R: serde::de::DeserializeOwned>(
        &self,
        request: HttpRequest,
        operation: &str,
    ) -> Result<R, GitLabError> {
        let response = self.send(request, operation).await?;
        serde_json::from_str(&response.body).map_err(|e| {
            GitLabError::new(
                ErrorKind::Validation,
                operation,
                format!("応答を解釈できません: {e}"),
            )
        })
    }

    /// 1 回の操作を、再試行の規則に従って送る (EC-16、EC-17、EC-18)。
    async fn send(
        &self,
        request: HttpRequest,
        operation: &str,
    ) -> Result<HttpResponse, GitLabError> {
        let mut transient_used = 0usize;
        let mut rate_limit_used = 0usize;

        loop {
            let outcome = self.transport.send(request.clone()).await;
            let error = match outcome {
                Ok(response) if response.is_success() => return Ok(response),
                Ok(response) => {
                    let kind = classify_status(response.status, response.header("retry-after"));
                    GitLabError::new(kind, operation, extract_message(&response.body))
                }
                Err(transport_error) => to_error(transport_error, operation),
            };

            match &error.kind {
                ErrorKind::Transient if transient_used < self.retry.transient_backoff.len() => {
                    let wait = self.retry.transient_backoff[transient_used];
                    transient_used += 1;
                    sleep(wait).await;
                }
                ErrorKind::RateLimited { retry_after }
                    if rate_limit_used < self.retry.rate_limit_attempts =>
                {
                    let wait = retry_after.unwrap_or(self.retry.rate_limit_default_wait);
                    rate_limit_used += 1;
                    sleep(wait).await;
                }
                _ => return Err(error),
            }
        }
    }
}

async fn sleep(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    tokio::time::sleep(duration).await;
}

fn to_error(error: TransportError, operation: &str) -> GitLabError {
    let kind = match &error {
        TransportError::Timeout | TransportError::Connect(_) => ErrorKind::Unreachable,
        TransportError::ProxyAuthentication(_) => ErrorKind::ProxyAuthentication,
        // トークン未設定は再入力を促すべきもので、再試行する意味がない。
        TransportError::MissingToken => ErrorKind::Authentication,
        TransportError::Other(_) => ErrorKind::Transient,
    };
    GitLabError::new(kind, operation, error.to_string())
}

/// GraphQL の最上位エラーを分類へ落とす。
fn classify_graphql_errors(errors: &[serde_json::Value]) -> ErrorKind {
    let text = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if text.contains("unauthorized") || text.contains("authentication") {
        ErrorKind::Authentication
    } else if text.contains("not allowed")
        || text.contains("forbidden")
        || text.contains("permission")
    {
        ErrorKind::Authorization
    } else if text.contains("not found") || text.contains("does not exist") {
        ErrorKind::NotFound
    } else {
        ErrorKind::Validation
    }
}

fn issue_body(issue: &NewIssue) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("title".into(), issue.title.clone().into());
    if !issue.description.is_empty() {
        body.insert("description".into(), issue.description.clone().into());
    }
    if !issue.labels.is_empty() {
        body.insert("labels".into(), issue.labels.join(",").into());
    }
    if !issue.assignee_ids.is_empty() {
        body.insert(
            "assignee_ids".into(),
            serde_json::Value::Array(issue.assignee_ids.iter().map(|i| (*i).into()).collect()),
        );
    }
    if let Some(milestone) = &issue.milestone {
        body.insert("milestone".into(), milestone.clone().into());
    }
    if let Some(start) = issue.start_date {
        body.insert("start_date".into(), start.to_string().into());
    }
    if let Some(due) = issue.due_date {
        body.insert("due_date".into(), due.to_string().into());
    }
    if issue.confidential {
        body.insert("confidential".into(), true.into());
    }
    if let Some(epic) = issue.epic_id {
        body.insert("epic_id".into(), epic.into());
    }
    if let Some(weight) = issue.weight {
        body.insert("weight".into(), weight.into());
    }
    serde_json::Value::Object(body)
}

fn to_candidate(value: &serde_json::Value, kind: CandidateKind) -> Option<Candidate> {
    let id = match value.get("id") {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => return None,
    };
    if kind == CandidateKind::Iteration {
        return to_iteration_candidate(value, id);
    }
    let name_key = match kind {
        CandidateKind::Label => "name",
        CandidateKind::Member => "username",
        CandidateKind::Milestone | CandidateKind::Epic | CandidateKind::Iteration => "title",
    };
    let name = value.get(name_key)?.as_str()?.to_string();
    Some(Candidate {
        id,
        name,
        ..Candidate::default()
    })
}

/// イテレーションの候補。期間を持たせる。
///
/// 周期設定から自動で作られたイテレーションは題名を持たないことが多い。
/// 題名がなければ期間を名前にし、候補から落とさない。
fn to_iteration_candidate(value: &serde_json::Value, id: String) -> Option<Candidate> {
    let date = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    };
    let start_date = date("start_date");
    let due_date = date("due_date");
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let name = match (title, start_date, due_date) {
        (Some(title), _, _) => title.to_string(),
        (None, Some(start), Some(due)) => format!("{start}〜{due}"),
        // 題名も期間もなければ、利用者が見分けられないため候補にしない。
        _ => return None,
    };
    Some(Candidate {
        id,
        name,
        start_date,
        due_date,
    })
}

/// プロジェクトの指定に名前空間付きの経路を使えるようにする。
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => other
                .to_string()
                .as_bytes()
                .iter()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab::transport::HttpResponse;
    use crate::testing::FakeTransport;

    fn client(responses: Vec<Result<HttpResponse, TransportError>>) -> GitLabClient<FakeTransport> {
        GitLabClient::with_retry(
            FakeTransport::new(responses),
            RetryPolicy::without_waiting(),
        )
    }

    fn ok(body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse::new(200, body))
    }

    fn status(code: u16, body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse::new(code, body))
    }

    // ---- G5 接続確認・バージョン・ティア ----

    /// FR-01: 接続に成功すると利用者名が返る。
    #[tokio::test]
    async fn connect_returns_username_and_version() {
        let c = client(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            ok(r#"{"plan":"premium"}"#),
        ]);
        let connection = c.connect().await.expect("接続できる");
        assert_eq!(connection.username, "taro");
        assert_eq!(
            connection.version,
            Version {
                major: 19,
                minor: 2,
                patch: 1
            }
        );
        assert_eq!(connection.tier, Tier::PremiumOrAbove);
        assert!(connection.version.is_supported());
    }

    /// EC-11: 認証が拒否されたら再試行せず認証エラーを返す。
    #[tokio::test]
    async fn connect_authentication_failure_is_not_retried() {
        let c = client(vec![status(401, r#"{"message":"401 Unauthorized"}"#)]);
        let err = c.connect().await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Authentication);
        assert_eq!(c.transport.sent_count(), 1, "再試行してはいけない");
    }

    /// EC-35: バージョンが下限未満であることを判別できる。
    #[tokio::test]
    async fn connect_detects_unsupported_version() {
        let c = client(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"18.11.3-ee"}"#),
            status(403, "{}"),
        ]);
        let connection = c.connect().await.expect("接続はできる");
        assert!(!connection.version.is_supported(), "下限未満と判定されない");
        assert_eq!(connection.version.to_string(), "18.11.3");
    }

    /// NFR-13: 下限の境界。19.2 は満たし、19.1 は満たさない。
    #[test]
    fn connect_version_boundary() {
        assert!(Version::parse("19.2.0").expect("解釈できる").is_supported());
        assert!(Version::parse("19.3.0").expect("解釈できる").is_supported());
        assert!(Version::parse("20.0.0").expect("解釈できる").is_supported());
        assert!(!Version::parse("19.1.9").expect("解釈できる").is_supported());
        assert!(!Version::parse("18.9.0").expect("解釈できる").is_supported());
    }

    /// EC-15: 管理者権限がなくティアを判定できない場合は Unknown になり、
    /// EE 固有項目は使える前提で進む (設計書 R-5 の確認待ち)。
    #[tokio::test]
    async fn connect_tier_unknown_when_not_permitted() {
        let c = client(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            status(403, r#"{"message":"403 Forbidden"}"#),
        ]);
        let connection = c.connect().await.expect("接続できる");
        assert_eq!(connection.tier, Tier::Unknown);
        assert!(connection.tier.allows_ee_fields());
    }

    /// EC-15: Free と判定できた場合は EE 固有項目を使えない。
    #[tokio::test]
    async fn connect_tier_free_disallows_ee_fields() {
        let c = client(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"19.2.1-ee"}"#),
            ok(r#"{"plan":"free"}"#),
        ]);
        let connection = c.connect().await.expect("接続できる");
        assert_eq!(connection.tier, Tier::Free);
        assert!(!connection.tier.allows_ee_fields());
    }

    #[tokio::test]
    async fn connect_rejects_unparsable_version() {
        let c = client(vec![
            ok(r#"{"username":"taro"}"#),
            ok(r#"{"version":"不明"}"#),
        ]);
        let err = c.connect().await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Validation);
    }

    // ---- G4 再試行と待機 ----

    /// EC-17: サーバー側エラーは 2 回まで再試行し、いずれも失敗したら失敗を返す。
    #[tokio::test]
    async fn retry_transient_up_to_two_times() {
        let c = client(vec![
            status(500, "{}"),
            status(500, "{}"),
            status(500, "{}"),
        ]);
        let err = c.fetch_project("11").await.expect_err("3 回とも失敗する");
        assert_eq!(err.kind, ErrorKind::Transient);
        assert_eq!(c.transport.sent_count(), 3, "最初の 1 回と再試行 2 回");
    }

    /// EC-17: 再試行で成功したらその結果を返す。
    #[tokio::test]
    async fn retry_transient_then_success() {
        let c = client(vec![
            status(503, "{}"),
            ok(r#"{"id":11,"path_with_namespace":"group/alpha"}"#),
        ]);
        let project = c.fetch_project("11").await.expect("再試行で成功する");
        assert_eq!(project.name, "group/alpha");
        assert_eq!(c.transport.sent_count(), 2);
    }

    /// EC-18: 要求上限は待機時間の指示に従い 1 回再試行する。
    #[tokio::test]
    async fn retry_rate_limited_once_with_retry_after() {
        let c = client(vec![
            Ok(HttpResponse::new(429, "{}").with_header("Retry-After", "0")),
            ok(r#"{"id":11,"path_with_namespace":"group/alpha"}"#),
        ]);
        c.fetch_project("11").await.expect("再試行で成功する");
        assert_eq!(c.transport.sent_count(), 2);
    }

    /// EC-18: 要求上限が続いたら 1 回の再試行で打ち切る。
    #[tokio::test]
    async fn retry_rate_limited_gives_up_after_one_attempt() {
        let c = client(vec![status(429, "{}"), status(429, "{}")]);
        let err = c.fetch_project("11").await.expect_err("失敗する");
        assert!(matches!(err.kind, ErrorKind::RateLimited { .. }));
        assert_eq!(c.transport.sent_count(), 2, "最初の 1 回と再試行 1 回");
    }

    /// EC-16: 応答が返らない場合は到達不能として扱い、再試行しない。
    #[tokio::test]
    async fn retry_timeout_is_unreachable_and_not_retried() {
        let c = client(vec![Err(TransportError::Timeout)]);
        let err = c.fetch_project("11").await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Unreachable);
        assert!(err.kind.aborts_batch(), "一括登録を止める分類である");
        assert_eq!(c.transport.sent_count(), 1);
    }

    /// EC-34: プロキシ認証の失敗は再試行しない。
    #[tokio::test]
    async fn retry_proxy_authentication_is_not_retried() {
        let c = client(vec![Err(TransportError::ProxyAuthentication(
            "407 Proxy Authentication Required".into(),
        ))]);
        let err = c.fetch_project("11").await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::ProxyAuthentication);
        assert_eq!(c.transport.sent_count(), 1);
    }

    /// EC-12: 認可エラーは再試行しない。
    #[tokio::test]
    async fn retry_authorization_is_not_retried() {
        let c = client(vec![status(403, r#"{"message":"403 Forbidden"}"#)]);
        let err = c.fetch_project("11").await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Authorization);
        assert_eq!(c.transport.sent_count(), 1);
    }

    /// 既定の規則は設計書 6.2 の値と一致する。
    #[test]
    fn retry_default_policy_matches_design() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.transient_backoff,
            vec![Duration::from_secs(2), Duration::from_secs(4)]
        );
        assert_eq!(policy.rate_limit_default_wait, Duration::from_secs(60));
        assert_eq!(policy.rate_limit_attempts, 1);
    }

    // ---- G6 Issue の作成 ----

    fn new_issue() -> NewIssue {
        NewIssue {
            project_id: "group/alpha".into(),
            title: "2026年10月 月次報告".into(),
            description: "期限 2026-10-12".into(),
            labels: vec!["report".into(), "monthly".into()],
            assignee_ids: vec![7, 9],
            milestone: Some("2026Q4".into()),
            start_date: NaiveDate::from_ymd_opt(2026, 10, 5),
            due_date: NaiveDate::from_ymd_opt(2026, 10, 12),
            confidential: true,
            epic_id: Some(42),
            weight: Some(3),
        }
    }

    /// FR-06: 指定した項目が要求に載る。イテレーションは含めない (ADR-05)。
    #[tokio::test]
    async fn issue_create_sends_all_fields() {
        let c = client(vec![ok(
            r#"{"id":1001,"iid":12,"web_url":"https://gitlab.example.com/group/alpha/-/issues/12"}"#,
        )]);
        let created = c.create_issue(&new_issue()).await.expect("作成できる");
        assert_eq!(created.iid, 12);
        assert_eq!(created.id, 1001);

        let request = c.transport.last_request().expect("送信された");
        let body = request.body.expect("本文がある");
        assert_eq!(body["title"], "2026年10月 月次報告");
        assert_eq!(body["labels"], "report,monthly");
        assert_eq!(body["assignee_ids"], serde_json::json!([7, 9]));
        assert_eq!(body["milestone"], "2026Q4");
        assert_eq!(body["start_date"], "2026-10-05");
        assert_eq!(body["due_date"], "2026-10-12");
        assert_eq!(body["confidential"], true);
        assert_eq!(body["epic_id"], 42);
        assert_eq!(body["weight"], 3);
        assert!(
            body.get("iteration_id").is_none(),
            "作成時にイテレーションを送ってはいけない (ADR-05)"
        );
        // プロジェクトの経路が符号化されている。
        assert!(request.path.contains("group%2Falpha"), "{}", request.path);
    }

    /// FR-06: 未設定の項目は要求に載せない。
    #[tokio::test]
    async fn issue_create_omits_unset_fields() {
        let c = client(vec![ok(r#"{"id":1,"iid":1,"web_url":"u"}"#)]);
        let issue = NewIssue {
            project_id: "11".into(),
            title: "確認".into(),
            ..NewIssue::default()
        };
        c.create_issue(&issue).await.expect("作成できる");
        let body = c
            .transport
            .last_request()
            .expect("送信された")
            .body
            .expect("本文がある");
        for key in [
            "description",
            "labels",
            "assignee_ids",
            "milestone",
            "start_date",
            "due_date",
            "confidential",
            "epic_id",
            "weight",
        ] {
            assert!(body.get(key).is_none(), "{key} を送ってはいけない");
        }
        assert_eq!(body["title"], "確認");
    }

    /// EC-33: マイルストーンが見つからない場合は入力エラーとして項目名と理由が載る。
    #[tokio::test]
    async fn issue_create_milestone_not_found_is_validation() {
        let c = client(vec![status(
            400,
            r#"{"message":{"milestone":["not found"]}}"#,
        )]);
        let err = c.create_issue(&new_issue()).await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Validation);
        assert!(err.message.contains("milestone"), "{}", err.message);
        assert!(!err.kind.aborts_batch(), "他の件は続行する");
    }

    /// EC-13: 担当者を割り当てられない場合も入力エラーになる。
    #[tokio::test]
    async fn issue_create_assignee_error_is_validation() {
        let c = client(vec![status(
            400,
            r#"{"message":{"assignee_ids":["is not a member"]}}"#,
        )]);
        let err = c.create_issue(&new_issue()).await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Validation);
        assert!(err.message.contains("assignee_ids"), "{}", err.message);
    }

    /// EC-14: 担当者のアカウントが無効化・削除されている場合も入力エラーとして扱う。
    ///
    /// EC-13 と同じ分類になる。再試行せず該当件のみ失敗とし、他の件は続行する。
    #[tokio::test]
    async fn issue_create_disabled_account_is_validation() {
        let c = client(vec![status(
            400,
            r#"{"message":{"assignee_ids":["user is blocked or no longer exists"]}}"#,
        )]);
        let err = c.create_issue(&new_issue()).await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Validation);
        assert!(!err.kind.is_retryable(), "再試行してはいけない");
        assert!(!err.kind.aborts_batch(), "他の件は続行する");
        assert!(err.message.contains("assignee_ids"), "{}", err.message);
        assert_eq!(c.transport.sent_count(), 1);
    }

    /// EC-02: タイトルが長すぎる場合の理由が利用者に伝わる。
    #[tokio::test]
    async fn issue_create_title_too_long_message() {
        let c = client(vec![status(
            400,
            r#"{"message":{"title":["is too long (maximum is 255 characters)"]}}"#,
        )]);
        let err = c.create_issue(&new_issue()).await.expect_err("失敗する");
        assert!(err.message.contains("255"), "{}", err.message);
    }

    // ---- G7 イテレーション設定 ----

    /// ADR-05: 作成とは別の操作でイテレーションを設定する。
    #[tokio::test]
    async fn iteration_set_sends_mutation() {
        let c = client(vec![ok(r#"{"data":{"issueSetIteration":{"errors":[]}}}"#)]);
        c.set_iteration(1001, "gid://gitlab/Iteration/55")
            .await
            .expect("設定できる");
        let request = c.transport.last_request().expect("送信された");
        assert_eq!(request.path, "/api/graphql");
        let body = request.body.expect("本文がある");
        assert!(body["query"]
            .as_str()
            .expect("文字列")
            .contains("issueSetIteration"));
        assert_eq!(body["variables"]["issue"], "gid://gitlab/Issue/1001");
        assert_eq!(body["variables"]["iteration"], "gid://gitlab/Iteration/55");
    }

    /// R-3: 状態コード 200 でも本文のエラーを拾い、分類へ落とす。
    #[tokio::test]
    async fn iteration_set_mutation_errors_become_validation() {
        let c = client(vec![ok(
            r#"{"data":{"issueSetIteration":{"errors":["Iteration not found"]}}}"#,
        )]);
        let err = c
            .set_iteration(1001, "gid://gitlab/Iteration/55")
            .await
            .expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::Validation);
        assert!(!err.kind.aborts_batch(), "Issue の作成は成功として扱う");
    }

    /// R-3: 最上位のエラーは文面から分類を推定する。
    #[tokio::test]
    async fn iteration_set_top_level_errors_are_classified() {
        for (message, expected) in [
            ("Unauthorized access", ErrorKind::Authentication),
            (
                "The resource that you are attempting to access does not exist",
                ErrorKind::NotFound,
            ),
            ("You do not have permission", ErrorKind::Authorization),
            ("Variable $iteration is invalid", ErrorKind::Validation),
        ] {
            let body = serde_json::json!({"errors":[{"message": message}]}).to_string();
            let c = client(vec![ok(&body)]);
            let err = c
                .set_iteration(1, "gid://gitlab/Iteration/1")
                .await
                .expect_err("失敗する");
            assert_eq!(err.kind, expected, "文面: {message}");
        }
    }

    // ---- G8 プロジェクトと候補 ----

    /// FR-02: プロジェクトを取得できる。
    #[tokio::test]
    async fn project_fetch_returns_path_with_namespace() {
        let c = client(vec![ok(
            r#"{"id":11,"path_with_namespace":"group/alpha","archived":false}"#,
        )]);
        let project = c.fetch_project("group/alpha").await.expect("取得できる");
        assert_eq!(project.id, "11");
        assert_eq!(project.name, "group/alpha");
    }

    /// EC-29: アーカイブされたプロジェクトは対象不在として扱う。
    #[tokio::test]
    async fn project_archived_is_not_found() {
        let c = client(vec![ok(
            r#"{"id":11,"path_with_namespace":"group/alpha","archived":true}"#,
        )]);
        let err = c.fetch_project("group/alpha").await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("アーカイブ"), "{}", err.message);
    }

    /// EC-29: 削除されたプロジェクトは対象不在になる。
    #[tokio::test]
    async fn project_deleted_is_not_found() {
        let c = client(vec![status(404, r#"{"message":"404 Project Not Found"}"#)]);
        let err = c.fetch_project("group/alpha").await.expect_err("失敗する");
        assert_eq!(err.kind, ErrorKind::NotFound);
    }

    /// FR-16: 種別ごとに候補の名前の取り出し方が変わる。
    #[tokio::test]
    async fn candidate_fetch_maps_name_per_kind() {
        let cases = [
            (
                CandidateKind::Label,
                r#"[{"id":1,"name":"report"}]"#,
                "report",
            ),
            (
                CandidateKind::Member,
                r#"[{"id":7,"username":"taro"}]"#,
                "taro",
            ),
            (
                CandidateKind::Milestone,
                r#"[{"id":3,"title":"2026Q4"}]"#,
                "2026Q4",
            ),
            (
                CandidateKind::Iteration,
                r#"[{"id":55,"title":"Sprint 12"}]"#,
                "Sprint 12",
            ),
        ];
        for (kind, body, expected) in cases {
            let c = client(vec![ok(body)]);
            let got = c.fetch_candidates("11", kind).await.expect("取得できる");
            assert_eq!(got.len(), 1, "{kind:?}");
            assert_eq!(got[0].name, expected, "{kind:?}");
        }
    }

    /// 計画 10: エピックはプロジェクトの属するグループから、上位グループの分も含めて取る。
    #[tokio::test]
    async fn epic_candidates_come_from_project_group() {
        let c = client(vec![
            ok(
                r#"{"id":11,"path_with_namespace":"group/alpha","namespace":{"id":9,"kind":"group","full_path":"group"}}"#,
            ),
            ok(r#"[{"id":42,"iid":3,"title":"基盤整備"}]"#),
        ]);
        let got = c
            .fetch_candidates("group/alpha", CandidateKind::Epic)
            .await
            .expect("取得できる");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "42");
        assert_eq!(got[0].name, "基盤整備");

        let sent = c.transport.sent_requests();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].path, "/api/v4/projects/group%2Falpha");
        assert_eq!(sent[1].path, "/api/v4/groups/9/epics");
        assert!(sent[1]
            .query
            .contains(&("include_ancestor_groups".to_string(), "true".to_string())));
    }

    /// 計画 10: 個人の名前空間のプロジェクトにはエピックがない。1 回の通信で 0 件を返す。
    #[tokio::test]
    async fn epic_candidates_empty_for_user_namespace() {
        let c = client(vec![ok(
            r#"{"id":12,"path_with_namespace":"taro/notes","namespace":{"id":5,"kind":"user","full_path":"taro"}}"#,
        )]);
        let got = c
            .fetch_candidates("taro/notes", CandidateKind::Epic)
            .await
            .expect("取得できる");
        assert!(got.is_empty());
        assert_eq!(c.transport.sent_count(), 1);
    }

    /// 計画 9: イテレーションは題名と期間を持つ候補になる。
    #[tokio::test]
    async fn iteration_candidate_keeps_title_and_period() {
        let c = client(vec![ok(
            r#"[{"id":55,"title":"Sprint 12","start_date":"2026-10-01","due_date":"2026-10-14"}]"#,
        )]);
        let got = c
            .fetch_candidates("11", CandidateKind::Iteration)
            .await
            .expect("取得できる");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "55");
        assert_eq!(got[0].name, "Sprint 12");
        assert_eq!(got[0].start_date, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert_eq!(got[0].due_date, NaiveDate::from_ymd_opt(2026, 10, 14));
    }

    /// 計画 9: 周期設定から作られた題名のないイテレーションを捨てず、期間で名付ける。
    #[tokio::test]
    async fn iteration_without_title_is_named_by_period() {
        let c = client(vec![ok(
            r#"[{"id":56,"title":null,"start_date":"2026-10-15","due_date":"2026-10-28"},
                {"id":57,"title":"  ","start_date":"2026-10-29","due_date":"2026-11-11"},
                {"id":58,"title":null}]"#,
        )]);
        let got = c
            .fetch_candidates("11", CandidateKind::Iteration)
            .await
            .expect("取得できる");
        let names: Vec<_> = got.iter().map(|i| i.name.as_str()).collect();
        // 題名も期間もない候補は見分けられないため落とす。
        assert_eq!(names, ["2026-10-15〜2026-10-28", "2026-10-29〜2026-11-11"]);
    }

    /// 計画 9: 期間を持つ前の保持データ (id と name だけ) をそのまま読める。
    #[test]
    fn candidate_cache_without_period_loads() {
        let got: Candidate = serde_json::from_str(r#"{"id":"1","name":"report"}"#).expect("読める");
        assert_eq!(got.name, "report");
        assert_eq!(got.start_date, None);
        assert_eq!(got.due_date, None);
    }

    /// FR-16: 1 回で多く取るため件数を指定する。
    #[tokio::test]
    async fn candidate_fetch_requests_large_page() {
        let c = client(vec![ok("[]")]);
        c.fetch_candidates("11", CandidateKind::Label)
            .await
            .expect("取得できる");
        let request = c.transport.last_request().expect("送信された");
        assert!(request
            .query
            .contains(&("per_page".to_string(), "100".to_string())));
    }

    /// 候補が 0 件でも失敗しない。
    #[tokio::test]
    async fn candidate_fetch_empty_list() {
        let c = client(vec![ok("[]")]);
        let got = c
            .fetch_candidates("11", CandidateKind::Label)
            .await
            .expect("取得できる");
        assert!(got.is_empty());
    }

    /// NFR-07: 応答に認証情報が含まれていても失敗の表示に残らない。
    #[tokio::test]
    async fn error_message_from_response_is_redacted() {
        let c = client(vec![status(
            401,
            r#"{"message":"invalid PRIVATE-TOKEN: glpat-abcdefghij"}"#,
        )]);
        let err = c.fetch_project("11").await.expect_err("失敗する");
        assert!(
            !err.message.contains("glpat-abcdefghij"),
            "トークンが残っている: {}",
            err.message
        );
    }
}
