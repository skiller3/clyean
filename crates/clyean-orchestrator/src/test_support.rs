// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A relay manager over a fake transport, whose User Assistant answers every prompt the
//! same way, for the tests of relays and servers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::allowlist::{RelayFile, RelayProtocol, RelaySpec};
use crate::lease::UserAssistantLease;
use crate::relays::{
    BrowserOpener, LiveRelay, PendingRelay, RelayManager, RelayTransport, TransportError,
    APPROVE_METHOD, SESSION_METHOD,
};

pub const PROJECT: &str = "/home/skyei/workspace/app";

#[derive(Default)]
pub struct FakeTransport {
    pub busy: Mutex<Vec<u16>>,
    pub activated: AtomicUsize,
    pub closed: Arc<AtomicUsize>,
}

struct FakePending(Arc<AtomicUsize>);
struct FakeLive(Arc<AtomicUsize>);

impl PendingRelay for FakePending {
    fn activate(self: Box<Self>) -> Result<Box<dyn LiveRelay>, TransportError> {
        Ok(Box::new(FakeLive(self.0.clone())))
    }
}

impl LiveRelay for FakeLive {
    fn connections(&self) -> usize {
        0
    }
    fn close(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl RelayTransport for FakeTransport {
    fn supports(&self, protocol: RelayProtocol) -> bool {
        protocol != RelayProtocol::Sctp
    }
    fn bind(&self, spec: &RelaySpec) -> Result<Box<dyn PendingRelay>, TransportError> {
        if self.busy.lock().unwrap().contains(&spec.host_port) {
            return Err(TransportError::PortUnavailable(format!(
                "port {} is in use",
                spec.host_port
            )));
        }
        self.activated.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakePending(self.closed.clone())))
    }
    fn check_available(&self, spec: &RelaySpec) -> Result<(), TransportError> {
        self.bind(spec).map(drop)
    }
    fn has_ipv6_loopback(&self) -> bool {
        true
    }
}

#[derive(Default)]
pub struct RecordingOpener(pub Mutex<Vec<String>>);

impl BrowserOpener for RecordingOpener {
    fn open(&self, url: &str) {
        self.0.lock().unwrap().push(url.to_string());
    }
}

pub struct Fixture {
    pub manager: Arc<RelayManager>,
    pub transport: Arc<FakeTransport>,
    pub opener: Arc<RecordingOpener>,
    pub file: RelayFile,
    pub prompts: Arc<Mutex<Vec<Value>>>,
    _dir: tempfile::TempDir,
}

/// A manager whose User Assistant answers every prompt with `decision` and reports
/// session `s1`; without a decision, no User Assistant is connected.
pub async fn fixture(decision: Option<&'static str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let file = RelayFile::in_config_dir(dir.path());
    let lease = Arc::new(UserAssistantLease::default());
    let transport = Arc::new(FakeTransport::default());
    let opener = Arc::new(RecordingOpener::default());
    let manager = Arc::new(RelayManager::new(
        PROJECT,
        file.clone(),
        lease.clone(),
        transport.clone(),
        opener.clone(),
    ));
    let prompts = Arc::new(Mutex::new(Vec::new()));
    if let Some(decision) = decision {
        let (orchestrator_end, extension_end) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(orchestrator_end);
        let serving = lease.clone();
        tokio::spawn(async move {
            let _ = serving.serve(BufReader::new(reader).lines(), writer).await;
        });
        let seen = prompts.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(extension_end);
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str() {
                    Some(SESSION_METHOD) => json!({"session_id": "s1"}),
                    Some(APPROVE_METHOD) => {
                        seen.lock().unwrap().push(request["params"].clone());
                        json!({"decision": decision})
                    }
                    _ => json!({}),
                };
                let reply = json!({"id": request["id"], "result": result});
                writer
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        for _ in 0..200 {
            if lease.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    Fixture {
        manager,
        transport,
        opener,
        file,
        prompts,
        _dir: dir,
    }
}
