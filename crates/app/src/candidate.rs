//! 候補値の取得と保持 (FR-16、EC-19、ADR-07)。
//!
//! 選択欄を開いた時に未取得なら取得し、結果をファイルへ保持する。
//! 到達できない状態では保持分と取得日時を返す。
//!
//! 起動時には取得しない。選択欄を開くまで読み込まないことで
//! NFR-03 (起動から接続画面の操作まで 5 秒以内) の予算を使わない (ADR-07)。

use chrono::NaiveDateTime;
use gitlabregister_infra::atomic::StoreError;
use gitlabregister_infra::gitlab::client::{Candidate, CandidateKind, GitLabClient};
use gitlabregister_infra::gitlab::transport::HttpTransport;
use gitlabregister_infra::gitlab::GitLabError;
use gitlabregister_infra::store::{CandidateCacheEntry, ConfigStore};

/// 候補の鮮度 (FR-16、EC-19)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// 今回取得した。
    JustFetched { at: NaiveDateTime },
    /// 保持していたものを使った。最新でない可能性がある。
    Cached { fetched_at: NaiveDateTime },
    /// 一度も取得していない。利用者が値を直接入力する。
    NeverFetched,
}

impl Freshness {
    pub fn describe(&self) -> String {
        match self {
            Freshness::JustFetched { at } => {
                format!("{} に取得しました", at.format("%Y-%m-%d %H:%M"))
            }
            Freshness::Cached { fetched_at } => format!(
                "{} に取得した内容を表示しています。最新でない可能性があります",
                fetched_at.format("%Y-%m-%d %H:%M")
            ),
            Freshness::NeverFetched => {
                "候補を取得していません。値を直接入力してください".into()
            }
        }
    }
}

/// 候補の取得結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateList {
    pub items: Vec<Candidate>,
    pub freshness: Freshness,
}

impl CandidateList {
    /// 文字で絞り込む (FR-16)。大文字と小文字を区別しない。
    pub fn filter(&self, query: &str) -> Vec<Candidate> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.items.clone();
        }
        self.items
            .iter()
            .filter(|c| c.name.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }

    /// ラベル名の一覧。EC-32 の突合に渡す。
    pub fn names(&self) -> Vec<String> {
        self.items.iter().map(|c| c.name.clone()).collect()
    }
}

/// 候補の取得と保持。
pub struct CandidateService {
    config: ConfigStore,
}

impl CandidateService {
    pub fn new(config: ConfigStore) -> Self {
        Self { config }
    }

    /// 保持している候補を返す。取得は行わない (EC-19)。
    pub fn cached(
        &self,
        project: &str,
        kind: CandidateKind,
    ) -> Result<CandidateList, StoreError> {
        let entries = self.config.load_candidates()?;
        Ok(match find(&entries, project, kind) {
            Some(entry) => CandidateList {
                items: entry.items.clone(),
                freshness: Freshness::Cached {
                    fetched_at: entry.fetched_at,
                },
            },
            None => CandidateList {
                items: Vec::new(),
                freshness: Freshness::NeverFetched,
            },
        })
    }

    /// 選択欄を開いたときに呼ぶ (FR-16、ADR-07)。
    ///
    /// 未取得なら取得し、取得済みなら保持分を返す。
    /// 取得に失敗した場合も、保持分があればそれを返す (EC-19)。
    pub async fn ensure<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        project: &str,
        kind: CandidateKind,
        now: NaiveDateTime,
    ) -> Result<CandidateList, StoreError> {
        let cached = self.cached(project, kind)?;
        if cached.freshness != Freshness::NeverFetched {
            return Ok(cached);
        }
        match self.fetch(client, project, kind, now).await {
            Ok(list) => Ok(list),
            // 取得できなくても保持分 (この場合は 0 件) を返し、直接入力へ倒す。
            Err(_) => Ok(cached),
        }
    }

    /// 取得し直す (FR-16)。
    ///
    /// 失敗した場合は保持分を返し、取得できなかったことを呼び出し側へ伝える。
    pub async fn refetch<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        project: &str,
        kind: CandidateKind,
        now: NaiveDateTime,
    ) -> Result<CandidateList, (CandidateList, GitLabError)> {
        match self.fetch(client, project, kind, now).await {
            Ok(list) => Ok(list),
            Err(error) => {
                let fallback = self
                    .cached(project, kind)
                    .unwrap_or(CandidateList {
                        items: Vec::new(),
                        freshness: Freshness::NeverFetched,
                    });
                Err((fallback, error))
            }
        }
    }

    async fn fetch<T: HttpTransport>(
        &self,
        client: &GitLabClient<T>,
        project: &str,
        kind: CandidateKind,
        now: NaiveDateTime,
    ) -> Result<CandidateList, GitLabError> {
        let items = client.fetch_candidates(project, kind).await?;
        // 保持に失敗しても取得した内容は返す。次回の起動で取り直せばよい。
        if let Ok(mut entries) = self.config.load_candidates() {
            entries.retain(|e| !(e.project == project && e.kind == kind));
            entries.push(CandidateCacheEntry {
                project: project.to_string(),
                kind,
                items: items.clone(),
                fetched_at: now,
            });
            let _ = self.config.save_candidates(&entries);
        }
        Ok(CandidateList {
            items,
            freshness: Freshness::JustFetched { at: now },
        })
    }
}

fn find<'a>(
    entries: &'a [CandidateCacheEntry],
    project: &str,
    kind: CandidateKind,
) -> Option<&'a CandidateCacheEntry> {
    entries
        .iter()
        .find(|e| e.project == project && e.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{client_with, ok, status};
    use chrono::NaiveDate;
    use gitlabregister_infra::gitlab::transport::TransportError;
    use gitlabregister_infra::paths::AppPaths;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 3)
            .expect("有効な日付")
            .and_hms_opt(14, 0, 0)
            .expect("有効な時刻")
    }

    fn later() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 4)
            .expect("有効な日付")
            .and_hms_opt(9, 0, 0)
            .expect("有効な時刻")
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        service: CandidateService,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("一時ディレクトリ");
        let root = dir.path().join("GitLabRegister");
        Fixture {
            _dir: dir,
            service: CandidateService::new(ConfigStore::new(AppPaths::with_root(root))),
        }
    }

    fn labels() -> Result<
        gitlabregister_infra::gitlab::transport::HttpResponse,
        TransportError,
    > {
        ok(r#"[{"id":1,"name":"report"},{"id":2,"name":"monthly"}]"#)
    }

    /// FR-16: 選択欄を開くと候補が取得される。
    #[tokio::test]
    async fn candidate_ensure_fetches_when_never_fetched() {
        let f = fixture();
        let client = client_with(vec![labels()]);
        let list = f
            .service
            .ensure(&client, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");
        assert_eq!(list.names(), vec!["report".to_string(), "monthly".to_string()]);
        assert_eq!(list.freshness, Freshness::JustFetched { at: now() });
        assert_eq!(client.transport().sent_count(), 1);
    }

    /// ADR-07: 2 回目は取得せず保持分を返す。
    #[tokio::test]
    async fn candidate_ensure_uses_cache_second_time() {
        let f = fixture();
        let first = client_with(vec![labels()]);
        f.service
            .ensure(&first, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");

        let second = client_with(vec![labels()]);
        let list = f
            .service
            .ensure(&second, "group/alpha", CandidateKind::Label, later())
            .await
            .expect("保持分を返す");
        assert_eq!(
            list.freshness,
            Freshness::Cached {
                fetched_at: now()
            }
        );
        assert_eq!(second.transport().sent_count(), 0, "再取得してはいけない");
    }

    /// EC-19: 到達できない状態では保持分と取得日時を返す。
    #[tokio::test]
    async fn candidate_offline_returns_cache_with_timestamp() {
        let f = fixture();
        let online = client_with(vec![labels()]);
        f.service
            .ensure(&online, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");

        let offline = client_with(vec![Err(TransportError::Timeout)]);
        let list = f
            .service
            .cached("group/alpha", CandidateKind::Label)
            .expect("読める");
        assert_eq!(list.items.len(), 2);
        match list.freshness {
            Freshness::Cached { fetched_at } => assert_eq!(fetched_at, now()),
            other => panic!("保持分として扱われていない: {other:?}"),
        }
        assert!(list.freshness.describe().contains("最新でない"), "{}", list.freshness.describe());

        // 再取得に失敗しても保持分を返す。
        let (fallback, error) = f
            .service
            .refetch(&offline, "group/alpha", CandidateKind::Label, later())
            .await
            .expect_err("取得は失敗する");
        assert_eq!(fallback.items.len(), 2);
        assert_eq!(
            error.kind,
            gitlabregister_infra::gitlab::ErrorKind::Unreachable
        );
    }

    /// FR-16: 一度も取得していないプロジェクトは 0 件と、直接入力の案内を返す。
    #[test]
    fn candidate_never_fetched_prompts_direct_input() {
        let f = fixture();
        let list = f
            .service
            .cached("group/beta", CandidateKind::Label)
            .expect("読める");
        assert!(list.items.is_empty());
        assert_eq!(list.freshness, Freshness::NeverFetched);
        assert!(
            list.freshness.describe().contains("直接入力"),
            "{}",
            list.freshness.describe()
        );
    }

    /// FR-16: 再取得すると GitLab-EE 上の変更が反映される。
    #[tokio::test]
    async fn candidate_refetch_reflects_new_label() {
        let f = fixture();
        let first = client_with(vec![labels()]);
        f.service
            .ensure(&first, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");

        let second = client_with(vec![ok(
            r#"[{"id":1,"name":"report"},{"id":2,"name":"monthly"},{"id":3,"name":"quarterly"}]"#,
        )]);
        let list = f
            .service
            .refetch(&second, "group/alpha", CandidateKind::Label, later())
            .await
            .expect("取得できる");
        assert!(list.names().contains(&"quarterly".to_string()));
        assert_eq!(list.freshness, Freshness::JustFetched { at: later() });

        // 保持分も更新されている。
        let cached = f
            .service
            .cached("group/alpha", CandidateKind::Label)
            .expect("読める");
        assert_eq!(cached.items.len(), 3);
    }

    /// FR-16: 文字を入力して絞り込める。大文字と小文字を区別しない。
    #[test]
    fn candidate_filter_is_case_insensitive() {
        let list = CandidateList {
            items: vec![
                Candidate {
                    id: "1".into(),
                    name: "Report".into(),
                },
                Candidate {
                    id: "2".into(),
                    name: "monthly".into(),
                },
            ],
            freshness: Freshness::NeverFetched,
        };
        assert_eq!(list.filter("rep").len(), 1);
        assert_eq!(list.filter("REP").len(), 1);
        assert_eq!(list.filter("").len(), 2, "空の入力では絞り込まない");
        assert!(list.filter("該当なし").is_empty());
    }

    /// 種別ごとに別の候補として保持する。
    #[tokio::test]
    async fn candidate_cache_is_per_kind() {
        let f = fixture();
        let label_client = client_with(vec![labels()]);
        f.service
            .ensure(&label_client, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");

        // 同じプロジェクトでも種別が違えば未取得。
        let member_client = client_with(vec![ok(r#"[{"id":7,"username":"taro"}]"#)]);
        let list = f
            .service
            .ensure(&member_client, "group/alpha", CandidateKind::Member, later())
            .await
            .expect("取得できる");
        assert_eq!(list.names(), vec!["taro".to_string()]);
        assert_eq!(member_client.transport().sent_count(), 1);
    }

    /// プロジェクトごとに別の候補として保持する。
    #[tokio::test]
    async fn candidate_cache_is_per_project() {
        let f = fixture();
        let alpha = client_with(vec![labels()]);
        f.service
            .ensure(&alpha, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("取得できる");
        let beta = client_with(vec![ok(r#"[{"id":9,"name":"beta-only"}]"#)]);
        let list = f
            .service
            .ensure(&beta, "group/beta", CandidateKind::Label, later())
            .await
            .expect("取得できる");
        assert_eq!(list.names(), vec!["beta-only".to_string()]);
    }

    /// 取得に失敗して保持分もない場合は 0 件と未取得を返し、失敗を伝播しない。
    #[tokio::test]
    async fn candidate_ensure_failure_falls_back_to_never_fetched() {
        let f = fixture();
        let client = client_with(vec![status(403, r#"{"message":"403 Forbidden"}"#)]);
        let list = f
            .service
            .ensure(&client, "group/alpha", CandidateKind::Label, now())
            .await
            .expect("失敗を伝播しない");
        assert!(list.items.is_empty());
        assert_eq!(list.freshness, Freshness::NeverFetched);
    }
}
