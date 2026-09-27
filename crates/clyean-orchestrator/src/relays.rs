// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The relay manager: it decides whether a relay opens.  A request is validated, checked
//! against what this host can carry, bound so that a busy port is reported before anyone
//! is asked, and then allowed by the user's allowlist for the session or the project, or
//! by an answer to a prompt that the User Assistant's extension shows over the lease.  The
//! listeners themselves belong to a [`RelayTransport`], which the host implements over
//! the bridge, so this crate knows nothing of sockets.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clyean_harness::session::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::allowlist::{
    AllowEntry, AllowScope, RelayFile, RelayFileError, RelayProtocol, RelaySettings, RelaySpec,
};
use crate::lease::{LeaseAnswer, LeaseError, LeaseRefusal, LeaseRequests, UserAssistantLease};

/// The most relays into the User Assistant's container one `clyean` process keeps open.
pub const MAX_RELAYS: usize = 16;
/// The most prompts that may wait for an answer at once.
pub const MAX_PENDING_PROMPTS: usize = 8;
/// How long the user has to answer a prompt; the extension denies it after this.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How much longer than [`APPROVAL_TIMEOUT`] the host waits for the extension's answer.
const APPROVAL_SLACK: Duration = Duration::from_secs(15);
/// How long the extension may take to report its session.
const SESSION_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

pub const APPROVE_METHOD: &str = "relay.approve";
pub const SESSION_METHOD: &str = "session.current";

/// Why a relay did not open.  The code is the protocol error code; a denial also says why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RelayError {
    pub code: &'static str,
    pub message: String,
    pub reason: Option<&'static str>,
}

impl RelayError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason: None,
        }
    }

    pub fn disabled() -> Self {
        Self::new(
            "relay_disabled",
            "relaying is turned off in Clyean's relay file",
        )
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("relay_invalid", message)
    }

    pub fn protocol_unsupported(message: impl Into<String>) -> Self {
        Self::new("relay_protocol_unsupported", message)
    }

    pub fn limit(message: impl Into<String>) -> Self {
        Self::new("relay_limit", message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new("relay_unavailable", message)
    }

    pub fn lease_only(method: &str) -> Self {
        Self::new(
            "lease_only",
            format!("{method} is accepted only on the User Assistant's lease"),
        )
    }

    pub fn new_unknown(method: &str) -> Self {
        Self::new("unknown_method", format!("unknown method: {method}"))
    }

    fn denied(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: "relay_denied",
            message: message.into(),
            reason: Some(reason),
        }
    }
}

/// Why the host could not bind or carry a relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    PortUnavailable(String),
    AddressUnavailable(String),
    ProtocolUnsupported(String),
    /// The User Assistant's bridge is not open.
    Unavailable(String),
    Other(String),
}

impl From<TransportError> for RelayError {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::PortUnavailable(message) => {
                Self::new("relay_port_unavailable", message)
            }
            TransportError::AddressUnavailable(message) => {
                Self::new("relay_address_unavailable", message)
            }
            TransportError::ProtocolUnsupported(message) => Self::protocol_unsupported(message),
            TransportError::Unavailable(message) => Self::unavailable(message),
            TransportError::Other(message) => Self::new("relay_unavailable", message),
        }
    }
}

/// The host's listeners for relays into the User Assistant's container.
pub trait RelayTransport: Send + Sync {
    /// Whether this host can relay `protocol` into the User Assistant's container.
    fn supports(&self, protocol: RelayProtocol) -> bool;
    /// Binds the relay's host addresses and holds them, accepting nothing yet.
    fn bind(&self, spec: &RelaySpec) -> Result<Box<dyn PendingRelay>, TransportError>;
    /// Binds and releases the relay's host addresses, for relays Podman will publish.
    fn check_available(&self, spec: &RelaySpec) -> Result<(), TransportError>;
    /// Whether the host has an IPv6 loopback address.
    fn has_ipv6_loopback(&self) -> bool;
}

/// A relay whose addresses are held until it is activated or dropped.
pub trait PendingRelay: Send {
    fn activate(self: Box<Self>) -> Result<Box<dyn LiveRelay>, TransportError>;
}

/// A relay that carries traffic until it is closed.
pub trait LiveRelay: Send + Sync {
    fn connections(&self) -> usize;
    fn close(&self);
}

/// Opens a page in the user's browser, for sign-ins.
pub trait BrowserOpener: Send + Sync {
    fn open(&self, url: &str);
}

/// Who asked for a relay, as far as the host can tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Requester {
    /// A sub-agent, known from the RPC session its request arrived on.
    Agent { agent: String, display: String },
    /// The User Assistant's own sign-in flow, which asked over the lease.
    SignIn { provider: String },
    /// A process in the User Assistant's container, as the bridge reported it: the command
    /// line is whatever that process set.
    Process {
        #[serde(skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

/// The server a relay would reach, for prompts about server containers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerFacts {
    pub name: String,
    pub command: String,
    pub cwd: String,
    pub persistent: bool,
}

/// How a relay was allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    Project,
    Session,
    Once,
}

impl From<AllowScope> for Approval {
    fn from(scope: AllowScope) -> Self {
        match scope {
            AllowScope::Project => Self::Project,
            AllowScope::Session => Self::Session,
        }
    }
}

/// A request to relay one host port to one guest port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayRequest {
    pub protocol: RelayProtocol,
    pub host_port: u16,
    pub guest_port: u16,
    /// `None` listens on loopback.
    pub host_addresses: Option<Vec<IpAddr>>,
    pub reason: Option<String>,
    /// Whether the relay serves the User Assistant's sign-in, whose page the host opens.
    pub sign_in: bool,
}

impl RelayRequest {
    /// Reads a request from protocol parameters.
    pub fn from_params(params: &Value) -> Result<Self, RelayError> {
        #[derive(Deserialize)]
        struct Params {
            protocol: String,
            host_port: u16,
            guest_port: u16,
            #[serde(default)]
            host_addresses: Option<Vec<String>>,
            #[serde(default)]
            reason: Option<String>,
        }
        let params: Params = serde_json::from_value(params.clone())
            .map_err(|error| RelayError::invalid(format!("invalid parameters: {error}")))?;
        let protocol = params.protocol.parse().map_err(RelayError::invalid)?;
        let host_addresses = params
            .host_addresses
            .map(|addresses| {
                addresses
                    .iter()
                    .map(|address| {
                        address.parse::<IpAddr>().map_err(|_| {
                            RelayError::invalid(format!("{address:?} is not an IP address"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        Ok(Self {
            protocol,
            host_port: params.host_port,
            guest_port: params.guest_port,
            host_addresses,
            reason: params.reason.filter(|reason| !reason.trim().is_empty()),
            sign_in: false,
        })
    }

    fn spec(&self, ipv6_loopback: bool) -> RelaySpec {
        RelaySpec {
            protocol: self.protocol,
            host_port: self.host_port,
            guest_port: self.guest_port,
            host_addresses: self
                .host_addresses
                .clone()
                .unwrap_or_else(|| RelaySpec::default_addresses(ipv6_loopback)),
        }
    }
}

/// An open relay as `relay.open` and `relay.list` report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayOpened {
    pub relay_id: String,
    #[serde(flatten)]
    pub spec: RelaySpec,
    pub approval: Approval,
    pub endpoints: Vec<String>,
}

struct OpenRelay {
    opened: RelayOpened,
    requester: Requester,
    opened_at: String,
    live: Box<dyn LiveRelay>,
}

type PromptOutcome = Result<Approval, RelayError>;

/// What a prompt about one relay says.
struct Asked<'a> {
    spec: &'a RelaySpec,
    requester: &'a Requester,
    server: Option<&'a ServerFacts>,
    reason: Option<&'a str>,
}

/// The relay file's settings, with the reason it could not be read, if it could not;
/// then nothing permanent can be recorded.
struct LoadedSettings {
    settings: RelaySettings,
    problem: Option<String>,
}

pub struct RelayManager {
    project: String,
    file: RelayFile,
    lease: Arc<UserAssistantLease>,
    transport: Arc<dyn RelayTransport>,
    opener: Arc<dyn BrowserOpener>,
    relays: Mutex<BTreeMap<u64, OpenRelay>>,
    next_id: AtomicU64,
    prompts: tokio::sync::Mutex<HashMap<String, watch::Receiver<Option<PromptOutcome>>>>,
}

impl RelayManager {
    /// A manager for the project at `project` (its canonical directory, which keys its
    /// allowlist in `file`).
    pub fn new(
        project: impl Into<String>,
        file: RelayFile,
        lease: Arc<UserAssistantLease>,
        transport: Arc<dyn RelayTransport>,
        opener: Arc<dyn BrowserOpener>,
    ) -> Self {
        Self {
            project: project.into(),
            file,
            lease,
            transport,
            opener,
            relays: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            prompts: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Opens a relay into the User Assistant's container, in the order the specification
    /// gives: validity, platform, availability, allowlist, prompt, activation.
    pub async fn open(
        &self,
        request: RelayRequest,
        requester: Requester,
    ) -> Result<RelayOpened, RelayError> {
        let loaded = self.settings().await;
        if !loaded.settings.enabled {
            return Err(RelayError::disabled());
        }
        let spec = request.spec(self.transport.has_ipv6_loopback());
        spec.validate().map_err(RelayError::invalid)?;
        if let Some(open) = self.find_open(&spec) {
            return Ok(open);
        }
        if self.relays.lock().expect("unpoisoned").len() >= MAX_RELAYS {
            return Err(RelayError::limit(format!(
                "this clyean process already has {MAX_RELAYS} relays open"
            )));
        }
        if !self.transport.supports(spec.protocol) {
            let why = match spec.protocol {
                RelayProtocol::Sctp => ": SCTP relays need a Linux or WSL host whose kernel provides SCTP (its sctp module)",
                _ => "",
            };
            return Err(RelayError::protocol_unsupported(format!(
                "this host cannot relay {} into the User Assistant's container{why}",
                spec.protocol.label()
            )));
        }
        let pending = self.transport.bind(&spec)?;
        let asked = Asked {
            spec: &spec,
            requester: &requester,
            server: None,
            reason: request.reason.as_deref(),
        };
        let approval = self.evaluate(asked, &loaded).await?;
        let live = pending.activate()?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let opened = RelayOpened {
            relay_id: format!("r-{id}"),
            endpoints: spec.host_endpoints(),
            spec: spec.clone(),
            approval,
        };
        self.relays.lock().expect("unpoisoned").insert(
            id,
            OpenRelay {
                opened: opened.clone(),
                requester,
                opened_at: clyean_project::utc_now_rfc3339(),
                live,
            },
        );
        if request.sign_in && loaded.settings.open_sign_in_pages {
            self.opener
                .open(&format!("http://localhost:{}/launch", spec.host_port));
        }
        Ok(opened)
    }

    /// Decides a relay that Podman will publish for a server container: validity, the
    /// host's availability of its addresses, then the allowlist or a prompt.
    pub async fn approve_published(
        &self,
        spec: &RelaySpec,
        requester: &Requester,
        server: &ServerFacts,
        reason: Option<&str>,
    ) -> Result<Approval, RelayError> {
        let loaded = self.settings().await;
        if !loaded.settings.enabled {
            return Err(RelayError::disabled());
        }
        spec.validate().map_err(RelayError::invalid)?;
        if spec.protocol == RelayProtocol::Sctp {
            return Err(RelayError::protocol_unsupported(
                "server containers cannot publish SCTP: neither rootless Podman nor the Podman machine forwarders carry it",
            ));
        }
        self.transport.check_available(spec)?;
        let asked = Asked {
            spec,
            requester,
            server: Some(server),
            reason,
        };
        self.evaluate(asked, &loaded).await
    }

    /// The default host addresses of a relay that names none.
    pub fn default_addresses(&self) -> Vec<IpAddr> {
        RelaySpec::default_addresses(self.transport.has_ipv6_loopback())
    }

    pub fn close(&self, relay_id: &str) {
        let removed = {
            let mut relays = self.relays.lock().expect("unpoisoned");
            let key = relays
                .iter()
                .find(|(_, relay)| relay.opened.relay_id == relay_id)
                .map(|(key, _)| *key);
            key.and_then(|key| relays.remove(&key))
        };
        if let Some(relay) = removed {
            relay.live.close();
        }
    }

    /// Closes every relay, when the bridge that carries them ends.
    pub fn close_all(&self) {
        let relays: Vec<OpenRelay> = std::mem::take(&mut *self.relays.lock().expect("unpoisoned"))
            .into_values()
            .collect();
        for relay in relays {
            relay.live.close();
        }
    }

    pub fn list(&self) -> Vec<Value> {
        self.relays
            .lock()
            .expect("unpoisoned")
            .values()
            .map(|relay| {
                let mut listed = serde_json::to_value(&relay.opened).expect("relays serialize");
                listed["requester"] =
                    serde_json::to_value(&relay.requester).expect("requesters serialize");
                listed["opened_at"] = Value::String(relay.opened_at.clone());
                listed["connections"] = json!(relay.live.connections());
                listed
            })
            .collect()
    }

    fn find_open(&self, spec: &RelaySpec) -> Option<RelayOpened> {
        self.relays
            .lock()
            .expect("unpoisoned")
            .values()
            .find(|relay| relay.opened.spec == *spec)
            .map(|relay| relay.opened.clone())
    }

    /// The relay file's settings, or the defaults with the reason the file could not be
    /// read.
    async fn settings(&self) -> LoadedSettings {
        let file = self.file.clone();
        let (settings, problem) = match tokio::task::spawn_blocking(move || file.load()).await {
            Ok(Ok(settings)) => (settings, None),
            Ok(Err(error)) => {
                tracing::warn!(target: "clyean::relays", %error, "the relay file cannot be read");
                (RelaySettings::default(), Some(error.to_string()))
            }
            Err(error) => (RelaySettings::default(), Some(error.to_string())),
        };
        LoadedSettings { settings, problem }
    }

    async fn current_session(&self) -> Option<String> {
        let answer = self
            .lease
            .request(SESSION_METHOD, json!({}), SESSION_QUERY_TIMEOUT)
            .await
            .ok()?;
        answer
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    async fn evaluate(
        &self,
        asked: Asked<'_>,
        loaded: &LoadedSettings,
    ) -> Result<Approval, RelayError> {
        let spec = asked.spec;
        let session = self.current_session().await;
        if let Some(scope) = loaded
            .settings
            .covering_scope(&self.project, session.as_deref(), spec)
        {
            if let (AllowScope::Session, Some(session)) = (scope, &session) {
                let session = session.clone();
                self.record(move |settings, now| {
                    settings.touch_session(&session, now);
                    Ok(())
                })
                .await;
            }
            return Ok(scope.into());
        }
        let facts = json!({
            "requester": asked.requester,
            "relay": spec,
            "server": asked.server,
            "reason": asked.reason,
            "session_available": session.is_some(),
            "permanent_available": loaded.problem.is_none(),
            "file_problem": loaded.problem,
            "warning": exposure_warning(spec),
        });
        let approval = self.prompt(facts).await?;
        let scope = match approval {
            Approval::Project => Some(AllowScope::Project),
            Approval::Session => Some(AllowScope::Session),
            Approval::Once => None,
        };
        if let Some(scope) = scope {
            let project = self.project.clone();
            let entry = AllowEntry::exactly(spec);
            self.record(move |settings, now| {
                settings.add(&project, session.as_deref(), scope, entry, now)
            })
            .await;
        }
        Ok(approval)
    }

    /// Records a change in the relay file.  A failure leaves the approval standing for
    /// this relay and is only logged.
    async fn record(
        &self,
        change: impl FnOnce(&mut RelaySettings, &str) -> Result<(), String> + Send + 'static,
    ) {
        let file = self.file.clone();
        let written = tokio::task::spawn_blocking(move || {
            let now = clyean_project::utc_now_rfc3339();
            file.update(|settings| change(settings, &now))
        })
        .await;
        match written {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(message))) => {
                tracing::warn!(target: "clyean::relays", %message, "the relay file was not changed")
            }
            Ok(Err(error)) => {
                tracing::warn!(target: "clyean::relays", %error, "the relay file was not changed")
            }
            Err(error) => {
                tracing::warn!(target: "clyean::relays", %error, "the relay file was not changed")
            }
        }
    }

    /// Asks the user through the extension.  Identical pending prompts share one answer,
    /// and at most [`MAX_PENDING_PROMPTS`] wait at once.
    async fn prompt(&self, facts: Value) -> PromptOutcome {
        let key = facts.to_string();
        let asker = {
            let mut prompts = self.prompts.lock().await;
            if let Some(waiting) = prompts.get(&key) {
                Err(waiting.clone())
            } else if prompts.len() >= MAX_PENDING_PROMPTS {
                return Err(RelayError::limit(format!(
                    "{MAX_PENDING_PROMPTS} relay requests are already waiting for an answer"
                )));
            } else {
                let (sender, receiver) = watch::channel(None);
                prompts.insert(key.clone(), receiver);
                Ok(sender)
            }
        };
        let sender = match asker {
            Ok(sender) => sender,
            Err(mut waiting) => {
                return match waiting.wait_for(Option::is_some).await {
                    Ok(outcome) => outcome.clone().expect("waited for an outcome"),
                    Err(_) => Err(RelayError::denied(
                        "no_user_interface",
                        "the prompt ended without an answer",
                    )),
                };
            }
        };
        let outcome = self.ask(facts).await;
        let _ = sender.send(Some(outcome.clone()));
        self.prompts.lock().await.remove(&key);
        outcome
    }

    async fn ask(&self, facts: Value) -> PromptOutcome {
        let answer = self
            .lease
            .request(APPROVE_METHOD, facts, APPROVAL_TIMEOUT + APPROVAL_SLACK)
            .await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(LeaseError::Timeout(_)) => {
                return Err(RelayError::denied(
                    "timed_out",
                    "the user did not answer the relay prompt in time",
                ))
            }
            Err(error) => {
                return Err(RelayError::denied(
                    "no_user_interface",
                    format!("no user interface could ask about the relay ({error})"),
                ))
            }
        };
        match answer.get("decision").and_then(Value::as_str) {
            Some("project") => Ok(Approval::Project),
            Some("session") => Ok(Approval::Session),
            Some("once") => Ok(Approval::Once),
            _ => {
                let reason = match answer.get("reason").and_then(Value::as_str) {
                    Some("timed_out") => "timed_out",
                    Some("no_user_interface") => "no_user_interface",
                    _ => "denied",
                };
                let message = match reason {
                    "timed_out" => "the user did not answer the relay prompt in time",
                    "no_user_interface" => "no user interface could ask about the relay",
                    _ => "the user denied the relay; do not ask for it again unless the user asks",
                };
                Err(RelayError::denied(reason, message))
            }
        }
    }

    /// Lists, adds, or removes allowlist entries for the extension's `/clyean relays`.
    async fn edit_allowlist(&self, method: &str, params: Value) -> LeaseAnswer {
        let session = params
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let scope = || -> Result<AllowScope, LeaseRefusal> {
            serde_json::from_value(params["scope"].clone())
                .map_err(|_| LeaseRefusal::new("relay_invalid", "the scope is project or session"))
        };
        let project = self.project.clone();
        match method {
            "relay.allowlist.list" => {}
            "relay.allowlist.add" => {
                let scope = scope()?;
                let entry: AllowEntry = serde_json::from_value(params["entry"].clone())
                    .map_err(|error| LeaseRefusal::new("relay_invalid", error.to_string()))?;
                entry
                    .validate()
                    .map_err(|message| LeaseRefusal::new("relay_invalid", message))?;
                let session = session.clone();
                self.change_file(move |settings, now| {
                    settings.add(&project, session.as_deref(), scope, entry, now)
                })
                .await?;
            }
            "relay.allowlist.remove" => {
                let scope = scope()?;
                let index = params["index"].as_u64().ok_or_else(|| {
                    LeaseRefusal::new("relay_invalid", "index names an entry, from zero")
                })? as usize;
                let session = session.clone();
                self.change_file(move |settings, _| {
                    settings
                        .remove(&project, session.as_deref(), scope, index)
                        .map(drop)
                })
                .await?;
            }
            other => {
                return Err(LeaseRefusal::new(
                    "unknown_method",
                    format!("unknown method: {other}"),
                ))
            }
        }
        Ok(self.allowlist_listing(session.as_deref()).await)
    }

    async fn change_file(
        &self,
        change: impl FnOnce(&mut RelaySettings, &str) -> Result<(), String> + Send + 'static,
    ) -> Result<(), LeaseRefusal> {
        let file = self.file.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let now = clyean_project::utc_now_rfc3339();
            file.update(|settings| change(settings, &now))
        })
        .await
        .map_err(|error| LeaseRefusal::new("internal", error.to_string()))?;
        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(LeaseRefusal::new("relay_invalid", message)),
            Err(error @ RelayFileError::Invalid { .. }) => {
                Err(LeaseRefusal::new("relay_file_invalid", error.to_string()))
            }
            Err(error) => Err(LeaseRefusal::new("internal", error.to_string())),
        }
    }

    async fn allowlist_listing(&self, session: Option<&str>) -> Value {
        let LoadedSettings {
            settings,
            problem: file_problem,
        } = self.settings().await;
        let render = |entries: Vec<AllowEntry>| -> Vec<Value> {
            entries
                .iter()
                .map(|entry| {
                    let mut value = serde_json::to_value(entry).expect("entries serialize");
                    value["text"] = Value::String(entry.to_string());
                    value
                })
                .collect()
        };
        json!({
            "type": "relay_allowlist",
            "project_dir": self.project,
            "enabled": settings.enabled,
            "project": render(settings.project_entries(&self.project)),
            "session": render(settings.session_entries(&self.project, session)),
            "file": self.file.path().display().to_string(),
            "file_problem": file_problem,
            "relays": self.list(),
        })
    }
}

impl LeaseRequests for RelayManager {
    fn handle<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, LeaseAnswer> {
        Box::pin(async move {
            match method {
                "relay.open" => {
                    let provider = params
                        .get("provider")
                        .and_then(Value::as_str)
                        .unwrap_or("a provider")
                        .to_string();
                    if params.get("purpose").and_then(Value::as_str) != Some("sign-in") {
                        return Err(LeaseRefusal::new(
                            "relay_invalid",
                            "relays requested on the lease serve sign-ins",
                        ));
                    }
                    let mut request = RelayRequest::from_params(&params)
                        .map_err(|error| LeaseRefusal::new(error.code, error.message))?;
                    request.sign_in = true;
                    let opened = self
                        .open(request, Requester::SignIn { provider })
                        .await
                        .map_err(|error| LeaseRefusal::new(error.code, error.message))?;
                    Ok(opened_result(&opened))
                }
                "relay.close" => {
                    let relay_id = params.get("relay_id").and_then(Value::as_str).unwrap_or("");
                    self.close(relay_id);
                    Ok(json!({"type": "relay_closed", "relay_id": relay_id}))
                }
                method if method.starts_with("relay.allowlist.") => {
                    self.edit_allowlist(method, params).await
                }
                other => Err(LeaseRefusal::new(
                    "unknown_method",
                    format!("unknown method: {other}"),
                )),
            }
        })
    }
}

/// The result of a successful `relay.open`.
pub fn opened_result(opened: &RelayOpened) -> Value {
    let mut result = serde_json::to_value(opened).expect("relays serialize");
    result["type"] = Value::String("relay_open".into());
    result
}

/// What the prompt says about listening beyond loopback.
fn exposure_warning(spec: &RelaySpec) -> Option<String> {
    let exposed: Vec<String> = spec
        .host_addresses
        .iter()
        .filter(|address| !address.is_loopback())
        .map(IpAddr::to_string)
        .collect();
    if exposed.is_empty() {
        return None;
    }
    let listed = exposed.join(" and ");
    if spec.host_addresses.iter().any(IpAddr::is_unspecified) {
        Some(format!(
            "Listening on {listed} accepts connections from every network this computer is on."
        ))
    } else {
        Some(format!(
            "Listening on {listed} accepts connections from other computers on that network."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fixture, PROJECT};

    fn request(protocol: RelayProtocol, host: u16, guest: u16) -> RelayRequest {
        RelayRequest {
            protocol,
            host_port: host,
            guest_port: guest,
            host_addresses: None,
            reason: Some("Preview the sign-up page".into()),
            sign_in: false,
        }
    }

    fn process() -> Requester {
        Requester::Process {
            pid: Some(42),
            command: Some("python3 -m http.server 8000".into()),
        }
    }

    #[tokio::test]
    async fn an_answer_for_the_project_opens_the_relay_and_later_requests_skip_the_prompt() {
        let fixture = fixture(Some("project")).await;
        let opened = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 3000, 5173), process())
            .await
            .unwrap();
        assert_eq!(opened.approval, Approval::Project);
        assert_eq!(opened.endpoints, ["127.0.0.1:3000", "[::1]:3000"]);
        let prompt = fixture.prompts.lock().unwrap()[0].clone();
        assert_eq!(prompt["requester"]["kind"], "process");
        assert_eq!(prompt["relay"]["host_port"], 3000);
        assert_eq!(prompt["session_available"], true);
        assert_eq!(prompt["warning"], Value::Null);
        let settings = fixture.file.load().unwrap();
        assert_eq!(settings.project_entries(PROJECT).len(), 1);

        fixture.manager.close(&opened.relay_id);
        assert_eq!(fixture.transport.closed.load(Ordering::SeqCst), 1);
        let again = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 3000, 5173), process())
            .await
            .unwrap();
        assert_eq!(again.approval, Approval::Project);
        assert_eq!(
            fixture.prompts.lock().unwrap().len(),
            1,
            "the allowlist answered"
        );
    }

    #[tokio::test]
    async fn a_session_answer_is_recorded_for_the_session_and_once_records_nothing() {
        let session = fixture(Some("session")).await;
        session
            .manager
            .open(request(RelayProtocol::Udp, 5353, 5300), process())
            .await
            .unwrap();
        let settings = session.file.load().unwrap();
        assert_eq!(settings.session_entries(PROJECT, Some("s1")).len(), 1);
        assert!(settings.project_entries(PROJECT).is_empty());

        let once = fixture(Some("once")).await;
        once.manager
            .open(request(RelayProtocol::Tcp, 8080, 8080), process())
            .await
            .unwrap();
        assert_eq!(once.file.load().unwrap(), RelaySettings::default());
    }

    #[tokio::test]
    async fn denials_and_missing_user_interfaces_come_back_as_errors() {
        let denied = fixture(Some("deny")).await;
        let error = denied
            .manager
            .open(request(RelayProtocol::Tcp, 3000, 3000), process())
            .await
            .unwrap_err();
        assert_eq!((error.code, error.reason), ("relay_denied", Some("denied")));

        let alone = fixture(None).await;
        let error = alone
            .manager
            .open(request(RelayProtocol::Tcp, 3000, 3000), process())
            .await
            .unwrap_err();
        assert_eq!(error.reason, Some("no_user_interface"));
    }

    #[tokio::test]
    async fn requests_are_checked_before_anyone_is_asked() {
        let fixture = fixture(Some("once")).await;
        fixture.transport.busy.lock().unwrap().push(9000);
        let busy = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 9000, 9000), process())
            .await
            .unwrap_err();
        assert_eq!(busy.code, "relay_port_unavailable");
        let sctp = fixture
            .manager
            .open(request(RelayProtocol::Sctp, 36412, 36412), process())
            .await
            .unwrap_err();
        assert_eq!(sctp.code, "relay_protocol_unsupported");
        assert!(
            sctp.message.contains("Linux or WSL host"),
            "{}",
            sctp.message
        );
        let invalid = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 0, 1), process())
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "relay_invalid");
        assert!(fixture.prompts.lock().unwrap().is_empty());
        fixture
            .file
            .update(|settings| settings.enabled = false)
            .unwrap();
        let disabled = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 3000, 3000), process())
            .await
            .unwrap_err();
        assert_eq!(disabled.code, "relay_disabled");
    }

    #[tokio::test]
    async fn an_open_relay_is_returned_again_and_the_count_is_capped() {
        let fixture = fixture(Some("once")).await;
        let first = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 4000, 4000), process())
            .await
            .unwrap();
        let repeat = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 4000, 4000), process())
            .await
            .unwrap();
        assert_eq!(first.relay_id, repeat.relay_id);
        for port in 1..MAX_RELAYS as u16 {
            fixture
                .manager
                .open(request(RelayProtocol::Tcp, 5000 + port, 80), process())
                .await
                .unwrap();
        }
        let over = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 6000, 80), process())
            .await
            .unwrap_err();
        assert_eq!(over.code, "relay_limit");
        assert_eq!(fixture.manager.list().len(), MAX_RELAYS);
        fixture.manager.close_all();
        assert!(fixture.manager.list().is_empty());
    }

    #[tokio::test]
    async fn sign_in_relays_open_the_launch_page() {
        let fixture = fixture(Some("once")).await;
        let answer = fixture
            .manager
            .handle(
                "relay.open",
                json!({"protocol": "tcp", "host_port": 54549, "guest_port": 54549,
                       "purpose": "sign-in", "provider": "openrouter.ai"}),
            )
            .await
            .unwrap();
        assert_eq!(answer["type"], "relay_open");
        assert_eq!(
            fixture.opener.0.lock().unwrap().as_slice(),
            ["http://localhost:54549/launch"]
        );
        let prompt = fixture.prompts.lock().unwrap()[0].clone();
        assert_eq!(
            prompt["requester"],
            json!({"kind": "sign_in", "provider": "openrouter.ai"})
        );
        let refused = fixture
            .manager
            .handle(
                "relay.open",
                json!({"protocol": "tcp", "host_port": 1, "guest_port": 1}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, "relay_invalid");
    }

    #[tokio::test]
    async fn non_loopback_relays_carry_a_warning() {
        let fixture = fixture(Some("once")).await;
        let mut everywhere = request(RelayProtocol::Tcp, 3000, 3000);
        everywhere.host_addresses = Some(vec!["0.0.0.0".parse().unwrap()]);
        fixture.manager.open(everywhere, process()).await.unwrap();
        let prompt = fixture.prompts.lock().unwrap()[0].clone();
        assert!(prompt["warning"]
            .as_str()
            .unwrap()
            .contains("every network"));
    }

    #[tokio::test]
    async fn the_extension_edits_the_allowlist_over_the_lease() {
        let fixture = fixture(Some("once")).await;
        let listing = fixture
            .manager
            .handle(
                "relay.allowlist.add",
                json!({"scope": "project", "entry": {"protocols": ["tcp"], "hostPorts": "3000-3010",
                       "guestPorts": "any", "hostAddresses": ["127.0.0.1"]}}),
            )
            .await
            .unwrap();
        assert_eq!(
            listing["project"][0]["text"],
            "tcp 3000-3010 -> any on 127.0.0.1"
        );
        let asked = fixture
            .manager
            .open(request(RelayProtocol::Tcp, 3005, 9999), process())
            .await
            .unwrap();
        assert_eq!(
            asked.approval,
            Approval::Once,
            "the entry leaves out IPv6 loopback, so the user was asked"
        );
        let mut ipv4 = request(RelayProtocol::Tcp, 3005, 9999);
        ipv4.host_addresses = Some(vec!["127.0.0.1".parse().unwrap()]);
        assert_eq!(
            fixture
                .manager
                .open(ipv4, process())
                .await
                .unwrap()
                .approval,
            Approval::Project
        );
        let listing = fixture
            .manager
            .handle(
                "relay.allowlist.remove",
                json!({"scope": "project", "index": 0}),
            )
            .await
            .unwrap();
        assert!(listing["project"].as_array().unwrap().is_empty());
        let invalid = fixture
            .manager
            .handle(
                "relay.allowlist.add",
                json!({"scope": "everywhere", "entry": {}}),
            )
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "relay_invalid");
    }

    #[tokio::test]
    async fn published_relays_refuse_sctp_and_check_availability() {
        let fixture = fixture(Some("once")).await;
        let server = ServerFacts {
            name: "web".into(),
            command: "npm run dev".into(),
            cwd: "app".into(),
            persistent: true,
        };
        let spec = RelaySpec {
            protocol: RelayProtocol::Sctp,
            host_port: 36412,
            guest_port: 36412,
            host_addresses: vec!["127.0.0.1".parse().unwrap()],
        };
        let error = fixture
            .manager
            .approve_published(&spec, &process(), &server, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "relay_protocol_unsupported");
        let tcp = RelaySpec {
            protocol: RelayProtocol::Tcp,
            ..spec
        };
        let approval = fixture
            .manager
            .approve_published(&tcp, &process(), &server, Some("preview"))
            .await
            .unwrap();
        assert_eq!(approval, Approval::Once);
        assert_eq!(fixture.prompts.lock().unwrap()[0]["server"]["name"], "web");
    }
}
