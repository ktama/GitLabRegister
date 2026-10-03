//! 試験のための差し替え実装。
//!
//! 本番の実行には含めない。`testing` 機能を有効にしたときだけ公開する。
//! クレートをまたいだ試験 (サービス層からインフラ層の判断を確かめる場合など) で使う。

use crate::gitlab::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use std::sync::Mutex;

/// あらかじめ用意した応答を順に返す通信口。送られた要求を記録する。
pub struct FakeTransport {
    responses: Mutex<Vec<Result<HttpResponse, TransportError>>>,
    sent: Mutex<Vec<HttpRequest>>,
}

impl FakeTransport {
    pub fn new(responses: Vec<Result<HttpResponse, TransportError>>) -> Self {
        let mut reversed = responses;
        reversed.reverse();
        Self {
            responses: Mutex::new(reversed),
            sent: Mutex::new(Vec::new()),
        }
    }

    pub fn sent_count(&self) -> usize {
        self.sent.lock().expect("施錠できる").len()
    }

    pub fn sent_requests(&self) -> Vec<HttpRequest> {
        self.sent.lock().expect("施錠できる").clone()
    }

    pub fn last_request(&self) -> Option<HttpRequest> {
        self.sent.lock().expect("施錠できる").last().cloned()
    }

    /// 残っている応答の件数。試験の設定漏れを見つけるために使う。
    pub fn remaining(&self) -> usize {
        self.responses.lock().expect("施錠できる").len()
    }
}

impl HttpTransport for FakeTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.sent.lock().expect("施錠できる").push(request);
        self.responses
            .lock()
            .expect("施錠できる")
            .pop()
            .unwrap_or_else(|| {
                Err(TransportError::Other(
                    "用意した応答を使い切った。試験の設定を見直すこと".into(),
                ))
            })
    }
}
