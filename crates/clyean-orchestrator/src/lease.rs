// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The User Assistant's lease: the one connection its orchestration extension holds for
//! the life of its harness process.  The first lease of a bridge session is the only one:
//! the extension takes it before any agent command runs, and every later `session.lease`
//! is refused, so no other process in the container can take the channel over.
//!
//! The lease carries requests both ways.  The orchestrator asks the User Assistant for
//! credential copies, relay approvals, and its session id; the extension asks the
//! orchestrator for sign-in relays and allowlist edits, which are accepted nowhere else.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clyean_harness::session::BoxFuture;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt, Lines};
use tokio::sync::{mpsc, oneshot};

/// Why a request to the User Assistant produced no result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    #[error("no User Assistant is connected")]
    Unavailable,
    #[error("the User Assistant's connection ended before it answered")]
    Closed,
    #[error("the User Assistant did not answer within {} seconds", .0.as_secs())]
    Timeout(Duration),
    #[error("{message}")]
    Refused { code: String, message: String },
    #[error("the User Assistant's answer was malformed: {0}")]
    Malformed(String),
}

/// The answer to a request the extension sent on the lease.
pub type LeaseAnswer = Result<Value, LeaseRefusal>;

/// A refused request from the extension: a protocol error code and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRefusal {
    pub code: String,
    pub message: String,
}

impl LeaseRefusal {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Handles the requests the extension sends on the lease.
pub trait LeaseRequests: Send + Sync {
    fn handle<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, LeaseAnswer>;
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, LeaseError>>>>>;

struct Connection {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
}

#[derive(Default)]
pub struct UserAssistantLease {
    claimed: AtomicBool,
    connection: Mutex<Option<Connection>>,
    requests: Mutex<Option<Arc<dyn LeaseRequests>>>,
    next_request: AtomicU64,
}

impl UserAssistantLease {
    /// Claims the lease for one connection.  Only the first claim succeeds.
    pub fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::SeqCst)
    }

    /// Routes the extension's requests to `handler`.
    pub fn set_request_handler(&self, handler: Arc<dyn LeaseRequests>) {
        *self.requests.lock().expect("unpoisoned") = Some(handler);
    }

    pub fn is_connected(&self) -> bool {
        self.connection.lock().expect("unpoisoned").is_some()
    }

    /// Serves the claimed lease connection until it ends: writes the orchestrator's
    /// requests, routes the extension's answers to them, and answers the extension's own
    /// requests.
    pub async fn serve<R, W>(&self, mut lines: Lines<R>, mut writer: W) -> std::io::Result<()>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let (outgoing, mut lines_out) = mpsc::unbounded_channel::<String>();
        let pending = Pending::default();
        *self.connection.lock().expect("unpoisoned") = Some(Connection {
            outgoing: outgoing.clone(),
            pending: pending.clone(),
        });
        let ended = loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => self.route(&pending, &outgoing, &line),
                    Ok(None) => break Ok(()),
                    Err(error) => break Err(error),
                },
                Some(line) = lines_out.recv() => {
                    if let Err(error) = write_line(&mut writer, &line).await {
                        break Err(error);
                    }
                }
            }
        };
        *self.connection.lock().expect("unpoisoned") = None;
        for (_, waiter) in pending.lock().expect("unpoisoned").drain() {
            let _ = waiter.send(Err(LeaseError::Closed));
        }
        ended
    }

    /// Sends one request to the User Assistant and waits up to `timeout` for its result.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LeaseError> {
        let id = format!(
            "lease-{}",
            self.next_request.fetch_add(1, Ordering::Relaxed) + 1
        );
        let (sender, answer) = oneshot::channel();
        let pending = {
            let slot = self.connection.lock().expect("unpoisoned");
            let connection = slot.as_ref().ok_or(LeaseError::Unavailable)?;
            connection
                .pending
                .lock()
                .expect("unpoisoned")
                .insert(id.clone(), sender);
            let line = json!({"id": id, "method": method, "params": params}).to_string();
            connection
                .outgoing
                .send(line)
                .map_err(|_| LeaseError::Closed)?;
            connection.pending.clone()
        };
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LeaseError::Closed),
            Err(_) => {
                pending.lock().expect("unpoisoned").remove(&id);
                Err(LeaseError::Timeout(timeout))
            }
        }
    }

    /// Answers a request line from the extension, or resolves the pending request an
    /// answer line belongs to; other lines are ignored.
    fn route(&self, pending: &Pending, outgoing: &mpsc::UnboundedSender<String>, line: &str) {
        let Ok(frame) = serde_json::from_str::<Value>(line) else {
            tracing::debug!(target: "clyean::lease", "ignoring a malformed line on the lease");
            return;
        };
        let Some(id) = frame.get("id").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        if let Some(method) = frame.get("method").and_then(Value::as_str) {
            self.answer_request(id, method.to_string(), frame["params"].clone(), outgoing);
            return;
        }
        let Some(waiter) = pending.lock().expect("unpoisoned").remove(&id) else {
            return;
        };
        let result = match (frame.get("result"), frame.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (_, Some(error)) => Err(LeaseError::Refused {
                code: error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("refused")
                    .to_string(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no reason given")
                    .to_string(),
            }),
            (None, None) => Err(LeaseError::Malformed(
                "an answer with neither result nor error".into(),
            )),
        };
        let _ = waiter.send(result);
    }

    fn answer_request(
        &self,
        id: String,
        method: String,
        params: Value,
        outgoing: &mpsc::UnboundedSender<String>,
    ) {
        let handler = self.requests.lock().expect("unpoisoned").clone();
        let outgoing = outgoing.clone();
        tokio::spawn(async move {
            let answer = match handler {
                Some(handler) => handler.handle(&method, params).await,
                None => Err(LeaseRefusal::new(
                    "unknown_method",
                    format!("unknown method: {method}"),
                )),
            };
            let reply = match answer {
                Ok(result) => json!({"id": id, "result": result}),
                Err(refusal) => {
                    json!({"id": id, "error": {"code": refusal.code, "message": refusal.message}})
                }
            };
            let _ = outgoing.send(reply.to_string());
        });
    }
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &str) -> std::io::Result<()> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    struct Echo;

    impl LeaseRequests for Echo {
        fn handle<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, LeaseAnswer> {
            Box::pin(async move {
                match method {
                    "echo" => Ok(params),
                    other => Err(LeaseRefusal::new("unknown_method", other.to_string())),
                }
            })
        }
    }

    /// Serves `lease` over an in-memory pipe and returns the extension's end.
    fn connect(lease: &Arc<UserAssistantLease>) -> tokio::io::DuplexStream {
        let (orchestrator_end, extension_end) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(orchestrator_end);
        let serving = lease.clone();
        tokio::spawn(async move {
            let _ = serving.serve(BufReader::new(reader).lines(), writer).await;
        });
        extension_end
    }

    async fn connected(lease: &UserAssistantLease) {
        for _ in 0..200 {
            if lease.is_connected() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the lease never connected");
    }

    #[test]
    fn only_the_first_claim_succeeds() {
        let lease = UserAssistantLease::default();
        assert!(lease.claim());
        assert!(!lease.claim());
    }

    #[tokio::test]
    async fn requests_travel_both_ways_on_one_connection() {
        let lease = Arc::new(UserAssistantLease::default());
        lease.set_request_handler(Arc::new(Echo));
        let extension = connect(&lease);
        connected(&lease).await;
        let (reader, mut writer) = tokio::io::split(extension);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(b"{\"id\":\"ua-1\",\"method\":\"echo\",\"params\":{\"x\":1}}\n")
            .await
            .unwrap();
        let reply: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(reply, json!({"id": "ua-1", "result": {"x": 1}}));

        let asking = lease.clone();
        let question = tokio::spawn(async move {
            asking
                .request("session.current", json!({}), Duration::from_secs(5))
                .await
        });
        let request: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "session.current");
        let answer = json!({"id": request["id"], "result": {"session_id": "s1"}});
        writer
            .write_all(format!("{answer}\n").as_bytes())
            .await
            .unwrap();
        assert_eq!(question.await.unwrap().unwrap()["session_id"], "s1");

        writer
            .write_all(b"{\"id\":\"ua-2\",\"method\":\"nope\",\"params\":{}}\n")
            .await
            .unwrap();
        let refusal: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(refusal["error"]["code"], "unknown_method");
    }

    #[tokio::test]
    async fn requests_fail_without_a_connection_or_an_answer() {
        let lease = Arc::new(UserAssistantLease::default());
        assert_eq!(
            lease.request("x", json!({}), Duration::from_secs(1)).await,
            Err(LeaseError::Unavailable)
        );
        let _extension = connect(&lease);
        connected(&lease).await;
        assert_eq!(
            lease
                .request("x", json!({}), Duration::from_millis(50))
                .await,
            Err(LeaseError::Timeout(Duration::from_millis(50)))
        );
    }
}
