// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The process-level RPC client: spawns the harness, negotiates the protocol, correlates
//! responses to requests by id, and broadcasts every other frame as an event.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

use crate::frames::{FrameDecoder, RpcFrame};
use crate::{HarnessError, Result};

const EVENT_CHANNEL_CAPACITY: usize = 4096;
const DEFAULT_MAX_REASSEMBLED_BYTES: u64 = 64 * 1024 * 1024;
const STDERR_TAIL_LINES: usize = 40;

/// Frames that are not responses to a pending request.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    Frame(RpcFrame),
    Closed,
}

/// Anything the harness wrote that is not a frame (its stderr), for diagnostics.
#[derive(Debug, Clone)]
pub struct HarnessOutput(pub String);

type PendingResponses = Arc<Mutex<HashMap<String, oneshot::Sender<RpcFrame>>>>;

pub struct HarnessClient {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: PendingResponses,
    events: broadcast::Sender<HarnessEvent>,
    next_id: AtomicU64,
    protocol_version: u64,
}

impl HarnessClient {
    /// Spawns `command` with piped stdio, waits for the ready frame, and upgrades to
    /// protocol v2 when the harness advertises it.
    pub async fn spawn(mut command: Command, ready_timeout: Duration) -> Result<Self> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(HarnessError::Spawn)?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        tokio::spawn(forward_stderr(stderr, stderr_tail.clone()));

        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let pending: PendingResponses = Arc::new(Mutex::new(HashMap::new()));
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(read_frames(
            stdout,
            pending.clone(),
            events.clone(),
            ready_tx,
        ));

        let ready = match tokio::time::timeout(ready_timeout, ready_rx).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(_)) => {
                let tail = stderr_tail
                    .lock()
                    .await
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                let status = match child.try_wait() {
                    Ok(Some(status)) => format!("exit status {status}"),
                    _ => "output closed".to_string(),
                };
                return Err(HarnessError::ExitedBeforeReady(format!(
                    "{status}; stderr:\n{tail}"
                )));
            }
            Err(_) => return Err(HarnessError::Timeout("ready frame".into())),
        };
        let supports_v2 = ready
            .0
            .get("supportedProtocolVersions")
            .and_then(Value::as_array)
            .is_some_and(|versions| versions.iter().any(|v| v.as_u64() == Some(2)));

        let client = Self {
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
            pending,
            events,
            next_id: AtomicU64::new(1),
            protocol_version: if supports_v2 { 2 } else { 1 },
        };
        if supports_v2 {
            client
                .request("negotiate_protocol", json!({"protocolVersion": 2}))
                .await?;
        }
        Ok(client)
    }

    pub fn protocol_version(&self) -> u64 {
        self.protocol_version
    }

    /// Subscribes to every non-response frame.  Subscribe before sending the command
    /// whose events you need, or the first events may be missed.
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.events.subscribe()
    }

    /// Sends a command and waits for its response, failing when the harness reports
    /// `success: false`.
    pub async fn request(&self, command_type: &str, mut params: Value) -> Result<RpcFrame> {
        let id = format!("clyean-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        let object = params.as_object_mut().expect("params must be an object");
        object.insert("id".into(), Value::String(id.clone()));
        object.insert("type".into(), Value::String(command_type.into()));
        let mut line = serde_json::to_string(&params).expect("serializable");
        line.push('\n');
        {
            let mut stdin_slot = self.stdin.lock().await;
            let stdin = stdin_slot.as_mut().ok_or(HarnessError::OutputClosed)?;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }
        let response = rx.await.map_err(|_| HarnessError::OutputClosed)?;
        if response.0.get("success").and_then(Value::as_bool) == Some(false) {
            let message = response
                .0
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            return Err(HarnessError::CommandFailed {
                command: command_type.to_string(),
                message,
            });
        }
        Ok(response)
    }

    /// Closes stdin (dropping the handle is what delivers EOF) so the harness drains and
    /// exits, then waits for it, killing it after `grace`.
    pub async fn shutdown(&self, grace: Duration) -> Result<()> {
        drop(self.stdin.lock().await.take());
        let mut child_slot = self.child.lock().await;
        if let Some(mut child) = child_slot.take() {
            if tokio::time::timeout(grace, child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
        }
        Ok(())
    }
}

async fn forward_stderr(stderr: tokio::process::ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "clyean::harness", stderr = %line);
        let mut tail = tail.lock().await;
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

async fn read_frames(
    stdout: tokio::process::ChildStdout,
    pending: PendingResponses,
    events: broadcast::Sender<HarnessEvent>,
    ready_tx: oneshot::Sender<RpcFrame>,
) {
    let mut decoder = FrameDecoder::new(DEFAULT_MAX_REASSEMBLED_BYTES);
    let mut lines = BufReader::new(stdout).lines();
    let mut ready_tx = Some(ready_tx);
    while let Ok(Some(line)) = lines.next_line().await {
        let frame = match decoder.feed_line(&line) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(target: "clyean::harness", %error, "dropping malformed frame");
                continue;
            }
        };
        if frame.kind() == "ready" {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(frame);
            }
            continue;
        }
        if frame.kind() == "response" {
            if let Some(id) = frame.id() {
                if let Some(tx) = pending.lock().await.remove(id) {
                    let _ = tx.send(frame);
                    continue;
                }
            }
        }
        let _ = events.send(HarnessEvent::Frame(frame));
    }
    pending.lock().await.clear();
    let _ = events.send(HarnessEvent::Closed);
}
