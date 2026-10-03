//! 試験の補助。

use gitlabregister_infra::gitlab::client::{GitLabClient, RetryPolicy};
use gitlabregister_infra::gitlab::transport::{HttpResponse, TransportError};
use gitlabregister_infra::testing::FakeTransport;

/// 待機なしの規則で、用意した応答を順に返すクライアントを作る。
pub fn client_with(
    responses: Vec<Result<HttpResponse, TransportError>>,
) -> GitLabClient<FakeTransport> {
    GitLabClient::with_retry(FakeTransport::new(responses), RetryPolicy::without_waiting())
}

pub fn ok(body: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(200, body))
}

pub fn status(code: u16, body: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(code, body))
}
