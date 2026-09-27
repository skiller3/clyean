// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The server manager: it launches server containers for agents once the user has
//! approved, or allowlisted, each relay a launch asks for, waits until the server listens,
//! and reports how to reach it from inside the sandbox and from outside.  Podman is the
//! record of which servers run, so every `clyean` process of a project sees and stops the
//! same ones.  The containers belong to a [`ServerRuntime`], which the host implements
//! over Podman.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use clyean_agents::AgentId;
use clyean_harness::session::BoxFuture;
use clyean_harness::{RpcFrame, UiRequestHandler};
use clyean_sandbox::server::{exit_status_in_log, log_tail, server_log_relative_path};
use clyean_sandbox::server::{Publication, ServerContainer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::allowlist::{RelayProtocol, RelaySpec};
use crate::events::EventSink;
use crate::relays::{Approval, RelayError, RelayManager, Requester, ServerFacts};
use crate::work::WorkRegistry;

/// The most server containers of one project that run at once.
pub const MAX_SERVERS: usize = 8;
pub const MAX_PORTS: usize = 8;
pub const MAX_SERVER_RELAYS: usize = 4;
const MAX_NAME_LENGTH: usize = 32;
const DEFAULT_READY_TIMEOUT_SECONDS: u64 = 60;
const MAX_READY_TIMEOUT_SECONDS: u64 = 300;
const READINESS_POLL: Duration = Duration::from_millis(500);
const LOG_TAIL_LINES: usize = 20;

/// The title of the extension UI requests with which sub-agents ask for servers.
pub const SERVER_REQUEST_TITLE: &str = "clyean:server";

/// Why a server method failed.  Relay errors keep their codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ServerError {
    pub code: &'static str,
    pub message: String,
    pub reason: Option<&'static str>,
}

impl ServerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("server_invalid", message)
    }

    fn exists(name: &str) -> Self {
        Self::new(
            "server_exists",
            format!("a server named {name} already runs with other parameters; stop it first, or choose another name"),
        )
    }

    fn limit() -> Self {
        Self::new(
            "server_limit",
            format!("{MAX_SERVERS} servers of this project already run; stop one first"),
        )
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new("server_failed", message)
    }
}

impl From<RelayError> for ServerError {
    fn from(error: RelayError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            reason: error.reason,
        }
    }
}

/// A port a server listens on in its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PortSpec {
    pub protocol: RelayProtocol,
    pub port: u16,
}

/// A relay a launch asks for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayAsk {
    pub protocol: RelayProtocol,
    pub host_port: u16,
    pub guest_port: u16,
    #[serde(default)]
    pub host_addresses: Option<Vec<IpAddr>>,
}

/// The parameters of `server.launch`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub name: String,
    pub command: String,
    /// Relative to the project directory, or an absolute container path; empty means the
    /// project directory.
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub persistent: bool,
    pub ports: Vec<PortSpec>,
    #[serde(default)]
    pub relays: Vec<RelayAsk>,
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_seconds: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_ready_timeout() -> u64 {
    DEFAULT_READY_TIMEOUT_SECONDS
}

impl LaunchRequest {
    pub fn from_params(params: Value) -> Result<Self, ServerError> {
        let request: Self = serde_json::from_value(params)
            .map_err(|error| ServerError::invalid(format!("invalid parameters: {error}")))?;
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ServerError> {
        validate_name(&self.name)?;
        if self.command.trim().is_empty() || self.command.contains('\0') {
            return Err(ServerError::invalid("command is a non-empty shell command"));
        }
        if self.cwd.contains(['\0', '\n']) {
            return Err(ServerError::invalid(
                "cwd is a directory path on a single line",
            ));
        }
        if self.ports.is_empty() || self.ports.len() > MAX_PORTS {
            return Err(ServerError::invalid(format!(
                "a server declares one to {MAX_PORTS} ports it listens on"
            )));
        }
        for (index, port) in self.ports.iter().enumerate() {
            if port.port == 0 {
                return Err(ServerError::invalid("ports run from 1 to 65535"));
            }
            if self.ports[..index].contains(port) {
                return Err(ServerError::invalid(format!(
                    "{} port {} is declared twice",
                    port.protocol.label(),
                    port.port
                )));
            }
        }
        if self.relays.len() > MAX_SERVER_RELAYS {
            return Err(ServerError::invalid(format!(
                "a server has at most {MAX_SERVER_RELAYS} relays"
            )));
        }
        for (index, relay) in self.relays.iter().enumerate() {
            let declared = PortSpec {
                protocol: relay.protocol,
                port: relay.guest_port,
            };
            if !self.ports.contains(&declared) {
                return Err(ServerError::invalid(format!(
                    "the relay to {} port {} names a port the server does not declare",
                    relay.protocol.label(),
                    relay.guest_port
                )));
            }
            let repeated = self.relays[..index].iter().any(|earlier| {
                earlier.protocol == relay.protocol && earlier.host_port == relay.host_port
            });
            if repeated {
                return Err(ServerError::invalid(format!(
                    "two relays use {} host port {}",
                    relay.protocol.label(),
                    relay.host_port
                )));
            }
        }
        if !(1..=MAX_READY_TIMEOUT_SECONDS).contains(&self.ready_timeout_seconds) {
            return Err(ServerError::invalid(format!(
                "ready_timeout_seconds runs from 1 to {MAX_READY_TIMEOUT_SECONDS}"
            )));
        }
        Ok(())
    }

    fn reason(&self) -> Option<&str> {
        self.reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
    }
}

fn validate_name(name: &str) -> Result<(), ServerError> {
    let well_formed = !name.is_empty()
        && name.len() <= MAX_NAME_LENGTH
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if !well_formed || name == "localhost" {
        return Err(ServerError::invalid(format!(
            "{name:?} is not a server name: use up to {MAX_NAME_LENGTH} lower-case letters, digits, and inner hyphens, other than localhost"
        )));
    }
    Ok(())
}

/// A relay of a running server, as its launch recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRelay {
    #[serde(flatten)]
    pub spec: RelaySpec,
    pub approval: Approval,
}

/// What a launch asked for, recorded on its container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerDefinition {
    pub command: String,
    #[serde(default)]
    pub cwd: String,
    pub ports: Vec<PortSpec>,
    #[serde(default)]
    pub relays: Vec<ServerRelay>,
}

impl ServerDefinition {
    /// Whether a launch with `ports` and relays `specs` asks for this server again.
    fn is_asked_by(
        &self,
        command: &str,
        cwd: &str,
        ports: &[PortSpec],
        specs: &[RelaySpec],
    ) -> bool {
        let mut own_ports = self.ports.clone();
        let mut asked_ports = ports.to_vec();
        own_ports.sort();
        asked_ports.sort();
        self.command == command
            && self.cwd == cwd
            && own_ports == asked_ports
            && self.relays.len() == specs.len()
            && specs
                .iter()
                .all(|spec| self.relays.iter().any(|relay| relay.spec == *spec))
    }
}

/// A server container as the host lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningServer {
    pub name: String,
    pub container: String,
    pub persistent: bool,
    pub launch_id: String,
    pub requested_by: String,
    /// Seconds since the Unix epoch.
    pub started_at: i64,
    /// `None` when the container's definition label does not parse.
    pub definition: Option<ServerDefinition>,
}

/// What the host learns about a running server container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerProbe {
    /// The container's addresses on the project network, IPv4 first.
    pub addresses: Vec<IpAddr>,
    /// The container's `/proc/net` socket tables, each after a `== <table>` line.
    pub socket_tables: String,
}

/// The project's server containers on the host.
pub trait ServerRuntime: Send + Sync {
    /// Why server containers cannot run, when they cannot.
    fn unavailable(&self) -> Option<String>;
    /// The container path of a working directory, or why it is refused.
    fn workdir(&self, cwd: &str) -> Result<String, String>;
    /// Starts a server container and returns the container's name.
    fn start<'a>(&'a self, server: &'a ServerContainer) -> BoxFuture<'a, Result<String, String>>;
    /// The project's running server containers.
    fn list(&self) -> BoxFuture<'_, Result<Vec<RunningServer>, String>>;
    /// Probes a server container; `None` once it has ended.
    fn probe<'a>(
        &'a self,
        container: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerProbe>, String>>;
    fn stop<'a>(&'a self, containers: &'a [String]) -> BoxFuture<'a, Result<(), String>>;
    /// A server's log, as far as it has been written.
    fn log(&self, server: &str) -> String;
}

/// Whether a declared port is reachable on the project network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointState {
    Listening,
    LoopbackOnly,
    NotListening,
}

/// Where members of the project network send traffic for one declared port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxEndpoint {
    pub protocol: RelayProtocol,
    pub host: String,
    pub address: Option<IpAddr>,
    pub port: u16,
    pub state: EndpointState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn sandbox_endpoints(
    name: &str,
    ports: &[PortSpec],
    probe: &ContainerProbe,
) -> Vec<SandboxEndpoint> {
    ports
        .iter()
        .map(|declared| {
            let bound = bound_addresses(&probe.socket_tables, declared.protocol, declared.port);
            let state = if bound.iter().any(|address| !is_loopback(address)) {
                EndpointState::Listening
            } else if bound.is_empty() {
                EndpointState::NotListening
            } else {
                EndpointState::LoopbackOnly
            };
            SandboxEndpoint {
                protocol: declared.protocol,
                host: name.to_string(),
                address: probe.addresses.first().copied(),
                port: declared.port,
                state,
                message: (state == EndpointState::LoopbackOnly).then(|| format!(
                    "{} port {} listens only on the container's loopback, which other containers cannot reach; make the server listen on all interfaces (0.0.0.0 or ::), for example `vite --host 0.0.0.0`",
                    declared.protocol.label(),
                    declared.port
                )),
            }
        })
        .collect()
}

fn is_loopback(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback(),
        IpAddr::V6(address) => {
            address.is_loopback() || address.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// The local addresses of the sockets bound to `port` in the socket tables: listening TCP
/// sockets, bound UDP sockets, and SCTP endpoints.
fn bound_addresses(tables: &str, protocol: RelayProtocol, port: u16) -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    let mut table = "";
    for line in tables.lines() {
        if let Some(name) = line.strip_prefix("== ") {
            table = name.trim();
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let found = match (protocol, table) {
            (RelayProtocol::Tcp, "tcp" | "tcp6") if fields.get(3) == Some(&"0A") => {
                inet_socket(fields.get(1), port)
            }
            (RelayProtocol::Udp, "udp" | "udp6") => inet_socket(fields.get(1), port),
            (RelayProtocol::Sctp, "sctp/eps")
                if fields.get(5) == Some(&port.to_string().as_str()) =>
            {
                fields
                    .iter()
                    .skip(8)
                    .filter_map(|text| text.parse().ok())
                    .collect()
            }
            _ => Vec::new(),
        };
        addresses.extend(found);
    }
    addresses
}

/// The address of a `/proc/net/{tcp,udp}{,6}` local address field bound to `port`.
fn inet_socket(field: Option<&&str>, port: u16) -> Vec<IpAddr> {
    let Some((address, hex_port)) = field.and_then(|field| field.split_once(':')) else {
        return Vec::new();
    };
    if u16::from_str_radix(hex_port, 16).ok() != Some(port) {
        return Vec::new();
    }
    decode_proc_address(address).into_iter().collect()
}

/// Decodes an address as the kernel prints it in `/proc/net` tables: each 32-bit word in
/// the byte order of the kernel, which is little-endian on every host Podman supports.
fn decode_proc_address(hex: &str) -> Option<IpAddr> {
    let word = |index: usize| {
        hex.get(index * 8..index * 8 + 8)
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .map(u32::to_le_bytes)
    };
    match hex.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(0)?))),
        32 => {
            let mut bytes = [0u8; 16];
            for index in 0..4 {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&word(index)?);
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// What a launch reports: how to reach the server, and whether it is ready.
struct Report<'a> {
    name: &'a str,
    container: &'a str,
    persistent: bool,
    endpoints: Vec<SandboxEndpoint>,
    relays: &'a [ServerRelay],
}

impl Report<'_> {
    fn ready(&self) -> bool {
        self.endpoints
            .iter()
            .all(|endpoint| endpoint.state == EndpointState::Listening)
    }

    fn to_value(&self, kind: &str) -> Value {
        let relays: Vec<Value> = self
            .relays
            .iter()
            .map(|relay| {
                let mut value = serde_json::to_value(relay).expect("relays serialize");
                value["endpoints"] = json!(relay.spec.host_endpoints());
                value
            })
            .collect();
        json!({
            "type": kind,
            "name": self.name,
            "ready": self.ready(),
            "persistent": self.persistent,
            "container": self.container,
            "log": server_log_relative_path(self.name),
            "sandbox_endpoints": self.endpoints,
            "relays": relays,
            "summary": self.summary(),
        })
    }

    /// The report as a sentence for agents and the user.
    fn summary(&self) -> String {
        let inside: Vec<String> = self
            .endpoints
            .iter()
            .map(|endpoint| {
                format!(
                    "{}:{} ({})",
                    self.name,
                    endpoint.port,
                    endpoint.protocol.label()
                )
            })
            .collect();
        let mut text = if self.ready() {
            format!(
                "Server {} is ready: from inside the sandbox at {}",
                self.name,
                inside.join(", ")
            )
        } else {
            format!(
                "Server {} runs but is not ready yet: inside the sandbox it will answer at {}",
                self.name,
                inside.join(", ")
            )
        };
        let outside: Vec<String> = self
            .relays
            .iter()
            .flat_map(|relay| relay.spec.host_endpoints())
            .collect();
        if !outside.is_empty() {
            text.push_str(&format!(
                "; from this computer at {}",
                outside.join(" and ")
            ));
        }
        text.push('.');
        text.push_str(if self.persistent {
            "  It keeps running after Clyean exits, until stopped."
        } else {
            "  It stops when this clyean process ends."
        });
        for endpoint in &self.endpoints {
            match (&endpoint.message, endpoint.state) {
                (Some(message), _) => text.push_str(&format!("  {message}.")),
                (None, EndpointState::NotListening) => text.push_str(&format!(
                    "  {} port {} is not listening yet; see {} and list the server again later.",
                    endpoint.protocol.label(),
                    endpoint.port,
                    server_log_relative_path(self.name)
                )),
                _ => {}
            }
        }
        text
    }
}

pub struct ServerManager {
    runtime: Arc<dyn ServerRuntime>,
    relays: Option<Arc<RelayManager>>,
    launch_id: String,
    starting: tokio::sync::Mutex<()>,
}

impl ServerManager {
    /// A manager whose launches carry `launch_id`, the launch of this `clyean` process, and
    /// whose relays `relays` decides; without it, launches with relays are refused.
    pub fn new(
        runtime: Arc<dyn ServerRuntime>,
        relays: Option<Arc<RelayManager>>,
        launch_id: impl Into<String>,
    ) -> Self {
        Self {
            runtime,
            relays,
            launch_id: launch_id.into(),
            starting: tokio::sync::Mutex::new(()),
        }
    }

    /// Launches a server for `agent`, as `requester` in relay prompts: each relay is
    /// approved first, and nothing starts unless all are.
    pub async fn launch(
        &self,
        request: LaunchRequest,
        agent: AgentId,
        requester: Requester,
    ) -> Result<Value, ServerError> {
        if let Some(reason) = self.runtime.unavailable() {
            return Err(ServerError::failed(reason));
        }
        let workdir = self
            .runtime
            .workdir(&request.cwd)
            .map_err(ServerError::invalid)?;
        let specs = self.relay_specs(&request)?;
        let cwd = request.cwd.trim().to_string();
        if let Some(running) = self.check_running(&request, &cwd, &specs).await? {
            return Ok(running);
        }
        let facts = ServerFacts {
            name: request.name.clone(),
            command: request.command.clone(),
            cwd: if cwd.is_empty() {
                "the project directory".to_string()
            } else {
                cwd.clone()
            },
            persistent: request.persistent,
        };
        let mut relays = Vec::new();
        for spec in specs.iter() {
            let manager = self.relays.as_ref().ok_or_else(|| {
                RelayError::unavailable("this clyean process cannot relay to server containers")
            })?;
            let approval = manager
                .approve_published(spec, &requester, &facts, request.reason())
                .await?;
            relays.push(ServerRelay {
                spec: spec.clone(),
                approval,
            });
        }
        let definition = ServerDefinition {
            command: request.command.clone(),
            cwd: cwd.clone(),
            ports: request.ports.clone(),
            relays,
        };
        let container = {
            let _starting = self.starting.lock().await;
            if let Some(running) = self.check_running(&request, &cwd, &specs).await? {
                return Ok(running);
            }
            let server = ServerContainer {
                name: request.name.clone(),
                command: request.command.clone(),
                workdir,
                persistent: request.persistent,
                launch_id: self.launch_id.clone(),
                requested_by: agent.id().to_string(),
                publications: publications(&definition.relays),
                definition: serde_json::to_string(&definition).expect("definitions serialize"),
            };
            self.runtime.start(&server).await.map_err(|message| {
                ServerError::failed(format!(
                    "Podman could not start server {}: {message}",
                    request.name
                ))
            })?
        };
        let timeout = Duration::from_secs(request.ready_timeout_seconds);
        self.await_ready(
            &request.name,
            &container,
            request.persistent,
            &definition,
            timeout,
        )
        .await
    }

    /// The relays of a launch with their host addresses resolved.
    fn relay_specs(&self, request: &LaunchRequest) -> Result<Vec<RelaySpec>, ServerError> {
        request
            .relays
            .iter()
            .map(|relay| {
                let host_addresses = match &relay.host_addresses {
                    Some(addresses) => addresses.clone(),
                    None => self
                        .relays
                        .as_ref()
                        .map(|relays| relays.default_addresses())
                        .unwrap_or_else(|| RelaySpec::default_addresses(false)),
                };
                let spec = RelaySpec {
                    protocol: relay.protocol,
                    host_port: relay.host_port,
                    guest_port: relay.guest_port,
                    host_addresses,
                };
                spec.validate().map_err(ServerError::invalid)?;
                Ok(spec)
            })
            .collect()
    }

    /// The report of the server this launch asks for when it already runs, or why another
    /// server stands in the way.
    async fn check_running(
        &self,
        request: &LaunchRequest,
        cwd: &str,
        specs: &[RelaySpec],
    ) -> Result<Option<Value>, ServerError> {
        let running = self.runtime.list().await.map_err(ServerError::failed)?;
        if let Some(server) = running.iter().find(|server| server.name == request.name) {
            let definition = server.definition.clone().unwrap_or_default();
            if server.persistent != request.persistent
                || !definition.is_asked_by(&request.command, cwd, &request.ports, specs)
            {
                return Err(ServerError::exists(&request.name));
            }
            let timeout = Duration::from_secs(request.ready_timeout_seconds);
            let report = self
                .await_ready(
                    &server.name,
                    &server.container,
                    server.persistent,
                    &definition,
                    timeout,
                )
                .await?;
            return Ok(Some(report));
        }
        if running.len() >= MAX_SERVERS {
            return Err(ServerError::limit());
        }
        Ok(None)
    }

    /// Waits until every declared port listens on the project network, or the timeout
    /// passes, and reports the server; a server whose command ends first failed.
    async fn await_ready(
        &self,
        name: &str,
        container: &str,
        persistent: bool,
        definition: &ServerDefinition,
        timeout: Duration,
    ) -> Result<Value, ServerError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let probe = self
                .runtime
                .probe(container)
                .await
                .map_err(ServerError::failed)?;
            let Some(probe) = probe else {
                return Err(self.ended(name));
            };
            let report = Report {
                name,
                container,
                persistent,
                endpoints: sandbox_endpoints(name, &definition.ports, &probe),
                relays: &definition.relays,
            };
            if report.ready() || tokio::time::Instant::now() >= deadline {
                return Ok(report.to_value("server_started"));
            }
            tokio::time::sleep(READINESS_POLL).await;
        }
    }

    /// The failure of a server whose command ended: its exit status and its last lines.
    fn ended(&self, name: &str) -> ServerError {
        let log = self.runtime.log(name);
        let status = match exit_status_in_log(&log) {
            Some(0) => "exited with status 0, which a server running in the foreground does not do (run it without daemonizing)".to_string(),
            Some(status) => format!("exited with status {status}"),
            None => "ended".to_string(),
        };
        ServerError::failed(format!(
            "server {name}'s command {status} before its ports listened; the last lines of {}:\n{}",
            server_log_relative_path(name),
            log_tail(&log, LOG_TAIL_LINES)
        ))
    }

    /// Stops a server; one that is not running counts as stopped.
    pub async fn stop(&self, name: &str) -> Result<Value, ServerError> {
        validate_name(name)?;
        let running = self.runtime.list().await.map_err(ServerError::failed)?;
        let containers: Vec<String> = running
            .into_iter()
            .filter(|server| server.name == name)
            .map(|server| server.container)
            .collect();
        self.runtime
            .stop(&containers)
            .await
            .map_err(ServerError::failed)?;
        Ok(json!({"type": "server_stopped", "name": name}))
    }

    /// Stops every server of the project and returns their names.
    pub async fn stop_all(&self) -> Result<Vec<String>, ServerError> {
        let running = self.runtime.list().await.map_err(ServerError::failed)?;
        self.stop_servers(running).await
    }

    /// Stops this process's non-persistent servers, as `clyean` exits, and returns the
    /// servers that keep running.
    pub async fn finish(&self) -> Result<Vec<RunningServer>, ServerError> {
        let running = self.runtime.list().await.map_err(ServerError::failed)?;
        let (ending, remaining): (Vec<RunningServer>, Vec<RunningServer>) = running
            .into_iter()
            .partition(|server| !server.persistent && server.launch_id == self.launch_id);
        self.stop_servers(ending).await?;
        Ok(remaining)
    }

    async fn stop_servers(&self, servers: Vec<RunningServer>) -> Result<Vec<String>, ServerError> {
        let containers: Vec<String> = servers
            .iter()
            .map(|server| server.container.clone())
            .collect();
        self.runtime
            .stop(&containers)
            .await
            .map_err(ServerError::failed)?;
        Ok(servers.into_iter().map(|server| server.name).collect())
    }

    /// The project's servers with their endpoints' states now, their relays, and who
    /// asked for them.
    pub async fn list(&self) -> Result<Value, ServerError> {
        let running = self.runtime.list().await.map_err(ServerError::failed)?;
        let mut servers = Vec::new();
        for server in running {
            let definition = server.definition.clone().unwrap_or_default();
            let probe = self.runtime.probe(&server.container).await.ok().flatten();
            let report = Report {
                name: &server.name,
                container: &server.container,
                persistent: server.persistent,
                endpoints: sandbox_endpoints(
                    &server.name,
                    &definition.ports,
                    &probe.unwrap_or_default(),
                ),
                relays: &definition.relays,
            };
            let mut listed = report.to_value("server");
            listed["command"] = json!(definition.command);
            listed["cwd"] = json!(definition.cwd);
            listed["requested_by"] = json!(server.requested_by);
            listed["started_at"] = json!(rfc3339(server.started_at));
            listed["launched_by_this_process"] = json!(server.launch_id == self.launch_id);
            servers.push(listed);
        }
        Ok(json!({"type": "server_list", "servers": servers}))
    }
}

fn publications(relays: &[ServerRelay]) -> Vec<Publication> {
    relays
        .iter()
        .flat_map(|relay| {
            relay.spec.host_addresses.iter().map(|address| Publication {
                protocol: relay.spec.protocol.as_str().to_string(),
                host_address: *address,
                host_port: relay.spec.host_port,
                guest_port: relay.spec.guest_port,
            })
        })
        .collect()
}

fn rfc3339(unix_seconds: i64) -> Option<String> {
    let time = time::OffsetDateTime::from_unix_timestamp(unix_seconds).ok()?;
    time.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// Answers a sub-agent's `clyean:server` requests on its behalf: each launch is announced
/// on the unit of work, relays are approved as for anyone else, and the outcome, with where
/// the server listens, is announced again.
pub struct ServerRequests {
    pub servers: Arc<ServerManager>,
    pub agent: AgentId,
    pub work_id: String,
    pub registry: Arc<WorkRegistry>,
}

impl ServerRequests {
    async fn handle(&self, request: &RpcFrame) -> Value {
        let params: Option<Value> = request
            .0
            .get("placeholder")
            .and_then(Value::as_str)
            .and_then(|text| serde_json::from_str(text).ok());
        let Some(mut params) = params.filter(Value::is_object) else {
            return error_value(&ServerError::invalid("the server request was malformed"));
        };
        let action = params
            .as_object_mut()
            .and_then(|fields| fields.remove("action"));
        let outcome = match action.as_ref().and_then(Value::as_str) {
            Some("launch") => self.launch(params).await,
            Some("stop") => match params.get("name").and_then(Value::as_str) {
                Some(name) => self.servers.stop(name).await,
                None => Err(ServerError::invalid("stop needs the name of a server")),
            },
            Some("list") => self.servers.list().await,
            _ => Err(ServerError::invalid("the action is launch, stop, or list")),
        };
        outcome.unwrap_or_else(|error| error_value(&error))
    }

    async fn launch(&self, params: Value) -> Result<Value, ServerError> {
        let request = LaunchRequest::from_params(params)?;
        self.announce(format!(
            "asks to launch server {} ({}), listening on {}{}, {}",
            request.name,
            request.command,
            request
                .ports
                .iter()
                .map(|port| format!("{} {}", port.protocol.label(), port.port))
                .collect::<Vec<_>>()
                .join(", "),
            if request.relays.is_empty() {
                " with no relays".to_string()
            } else {
                format!(" with {} relay(s) to approve", request.relays.len())
            },
            if request.persistent {
                "kept running after Clyean exits"
            } else {
                "stopped when Clyean exits"
            }
        ))
        .await;
        let requester = Requester::Agent {
            agent: self.agent.id().to_string(),
            display: self.agent.display_name().to_string(),
        };
        let name = request.name.clone();
        let outcome = self.servers.launch(request, self.agent, requester).await;
        match &outcome {
            Ok(report) => {
                self.announce(report["summary"].as_str().unwrap_or_default().to_string())
                    .await
            }
            Err(error) => {
                self.announce(format!("could not launch server {name}: {error}"))
                    .await
            }
        }
        outcome
    }

    async fn announce(&self, text: String) {
        if let Some(handle) = self.registry.get(&self.work_id).await {
            let phase = handle.phase();
            EventSink::new(handle)
                .progress(self.agent.id(), phase, text)
                .await;
        }
    }
}

fn error_value(error: &ServerError) -> Value {
    json!({"error": {"code": error.code, "message": error.message, "reason": error.reason}})
}

impl UiRequestHandler for ServerRequests {
    fn answer<'a>(&'a self, request: &'a RpcFrame) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            if request.0.get("title").and_then(Value::as_str) != Some(SERVER_REQUEST_TITLE) {
                return None;
            }
            let answer = self.handle(request).await;
            Some(json!({"value": answer.to_string()}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;
    use crate::work::{WorkHandle, WorkId};
    use crate::Phase;
    use std::sync::Mutex;

    const LISTENING_TABLES: &str = "== tcp
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1
== tcp6
== udp
== udp6
== sctp/eps
";

    #[derive(Default)]
    struct FakeRuntime {
        unavailable: Option<String>,
        running: Mutex<Vec<RunningServer>>,
        started: Mutex<Vec<ServerContainer>>,
        stopped: Mutex<Vec<String>>,
        /// `None` probes as a container that has ended.
        tables: Mutex<Option<String>>,
        log: String,
    }

    impl FakeRuntime {
        fn listening() -> Self {
            Self {
                tables: Mutex::new(Some(LISTENING_TABLES.to_string())),
                ..Self::default()
            }
        }
    }

    impl ServerRuntime for FakeRuntime {
        fn unavailable(&self) -> Option<String> {
            self.unavailable.clone()
        }
        fn workdir(&self, cwd: &str) -> Result<String, String> {
            if cwd.contains("..") {
                return Err("no ..".into());
            }
            Ok(format!("/w/p/{cwd}"))
        }
        fn start<'a>(
            &'a self,
            server: &'a ServerContainer,
        ) -> BoxFuture<'a, Result<String, String>> {
            Box::pin(async move {
                let container = format!("clyean-p-server-{}", server.name);
                self.started.lock().unwrap().push(server.clone());
                self.running.lock().unwrap().push(RunningServer {
                    name: server.name.clone(),
                    container: container.clone(),
                    persistent: server.persistent,
                    launch_id: server.launch_id.clone(),
                    requested_by: server.requested_by.clone(),
                    started_at: 1_790_446_905,
                    definition: serde_json::from_str(&server.definition).ok(),
                });
                Ok(container)
            })
        }
        fn list(&self) -> BoxFuture<'_, Result<Vec<RunningServer>, String>> {
            Box::pin(async move { Ok(self.running.lock().unwrap().clone()) })
        }
        fn probe<'a>(
            &'a self,
            _container: &'a str,
        ) -> BoxFuture<'a, Result<Option<ContainerProbe>, String>> {
            Box::pin(async move {
                Ok(self
                    .tables
                    .lock()
                    .unwrap()
                    .clone()
                    .map(|tables| ContainerProbe {
                        addresses: vec!["10.89.0.7".parse().unwrap()],
                        socket_tables: tables,
                    }))
            })
        }
        fn stop<'a>(&'a self, containers: &'a [String]) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                self.stopped
                    .lock()
                    .unwrap()
                    .extend(containers.iter().cloned());
                self.running
                    .lock()
                    .unwrap()
                    .retain(|server| !containers.contains(&server.container));
                Ok(())
            })
        }
        fn log(&self, _server: &str) -> String {
            self.log.clone()
        }
    }

    fn launch_params(extra: Value) -> Value {
        let mut params = json!({"name": "web", "command": "npm run dev -- --host 0.0.0.0",
            "cwd": "app", "ports": [{"protocol": "tcp", "port": 5173}]});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    }

    fn request(extra: Value) -> LaunchRequest {
        LaunchRequest::from_params(launch_params(extra)).unwrap()
    }

    fn process() -> Requester {
        Requester::Process {
            pid: Some(7),
            command: Some("clyean-bridge server launch web".into()),
        }
    }

    #[test]
    fn socket_tables_tell_listening_from_loopback_only_ports() {
        let tables = "== tcp
  sl  local_address rem_address   st
   0: 00000000:1435 00000000:0000 0A 00000000:00000000
   1: 0100007F:0BB8 00000000:0000 0A 00000000:00000000
   2: 0100007F:1F90 0100007F:9C40 01 00000000:00000000
== tcp6
  sl  local_address rem_address   st
   0: 00000000000000000000000001000000:0CEA 00000000000000000000000000000000:0000 0A
   1: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 0A
== udp
  sl  local_address rem_address   st
   0: 00000000:14E9 00000000:0000 07
== udp6
== sctp/eps
 ENDPT     SOCK   STY SST HBKT LPORT   UID INODE LADDRS
 0 0 2   10  29   36412     0 23456 10.89.0.7 127.0.0.1
";
        let probe = ContainerProbe {
            addresses: vec!["10.89.0.7".parse().unwrap()],
            socket_tables: tables.to_string(),
        };
        let ports = [
            (RelayProtocol::Tcp, 5173, EndpointState::Listening),
            (RelayProtocol::Tcp, 3000, EndpointState::LoopbackOnly),
            (RelayProtocol::Tcp, 8080, EndpointState::NotListening),
            (RelayProtocol::Tcp, 3306, EndpointState::LoopbackOnly),
            (RelayProtocol::Tcp, 80, EndpointState::Listening),
            (RelayProtocol::Udp, 5353, EndpointState::Listening),
            (RelayProtocol::Udp, 5173, EndpointState::NotListening),
            (RelayProtocol::Sctp, 36412, EndpointState::Listening),
        ];
        let declared: Vec<PortSpec> = ports
            .iter()
            .map(|(protocol, port, _)| PortSpec {
                protocol: *protocol,
                port: *port,
            })
            .collect();
        let endpoints = sandbox_endpoints("web", &declared, &probe);
        for (endpoint, (_, port, state)) in endpoints.iter().zip(ports) {
            assert_eq!(endpoint.state, state, "port {port}");
            assert_eq!(endpoint.host, "web");
            assert_eq!(endpoint.address, Some("10.89.0.7".parse().unwrap()));
        }
        assert!(endpoints[1]
            .message
            .as_deref()
            .unwrap()
            .contains("all interfaces"));
        assert!(endpoints[0].message.is_none());
    }

    #[test]
    fn launch_requests_are_checked_field_by_field() {
        let refused = [
            json!({"name": "Web"}),
            json!({"name": "localhost"}),
            json!({"name": "-web"}),
            json!({"name": "a".repeat(33)}),
            json!({"command": "  "}),
            json!({"ports": []}),
            json!({"ports": [{"protocol": "tcp", "port": 1}, {"protocol": "tcp", "port": 1}]}),
            json!({"ports": [{"protocol": "tcp", "port": 0}]}),
            json!({"relays": [{"protocol": "tcp", "host_port": 3000, "guest_port": 3000}]}),
            json!({"relays": [{"protocol": "udp", "host_port": 3000, "guest_port": 5173}]}),
            json!({"relays": [{"protocol": "tcp", "host_port": 3000, "guest_port": 5173},
                              {"protocol": "tcp", "host_port": 3000, "guest_port": 5173}]}),
            json!({"ready_timeout_seconds": 0}),
            json!({"ready_timeout_seconds": 301}),
            json!({"port": 5173}),
        ];
        for extra in refused {
            let error = LaunchRequest::from_params(launch_params(extra.clone())).unwrap_err();
            assert_eq!(error.code, "server_invalid", "{extra}");
        }
        let accepted = request(json!({"relays": [{"protocol": "tcp", "host_port": 3000,
            "guest_port": 5173, "host_addresses": ["0.0.0.0"]}], "persistent": true}));
        assert_eq!(accepted.ready_timeout_seconds, 60);
        assert!(accepted.persistent);
    }

    #[tokio::test]
    async fn a_server_without_relays_starts_without_asking_and_says_where_it_listens() {
        let relays = fixture(Some("deny")).await;
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = ServerManager::new(runtime.clone(), Some(relays.manager.clone()), "l1");
        let report = servers
            .launch(request(json!({})), AgentId::Programmer, process())
            .await
            .unwrap();
        assert!(relays.prompts.lock().unwrap().is_empty());
        assert_eq!(report["type"], "server_started");
        assert_eq!(report["ready"], true);
        assert_eq!(report["persistent"], false);
        assert_eq!(report["log"], ".clyean/logs/servers/web.log");
        assert_eq!(
            report["sandbox_endpoints"],
            json!([{"protocol": "tcp", "host": "web", "address": "10.89.0.7", "port": 5173, "state": "listening"}])
        );
        assert!(report["summary"]
            .as_str()
            .unwrap()
            .starts_with("Server web is ready: from inside the sandbox at web:5173 (TCP)."));
        let started = runtime.started.lock().unwrap()[0].clone();
        assert_eq!(started.workdir, "/w/p/app");
        assert_eq!(started.requested_by, "programmer");
        assert_eq!(started.launch_id, "l1");
        assert!(started.publications.is_empty());
    }

    #[tokio::test]
    async fn every_relay_is_approved_before_anything_starts() {
        let denied = fixture(Some("deny")).await;
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = ServerManager::new(runtime.clone(), Some(denied.manager.clone()), "l1");
        let with_relay = json!({"relays": [{"protocol": "tcp", "host_port": 3000, "guest_port": 5173}],
            "reason": "Preview the sign-up page", "persistent": true});
        let error = servers
            .launch(request(with_relay.clone()), AgentId::Programmer, process())
            .await
            .unwrap_err();
        assert_eq!((error.code, error.reason), ("relay_denied", Some("denied")));
        assert!(runtime.started.lock().unwrap().is_empty());
        let prompt = denied.prompts.lock().unwrap()[0].clone();
        assert_eq!(prompt["server"]["name"], "web");
        assert_eq!(prompt["server"]["cwd"], "app");
        assert_eq!(prompt["server"]["persistent"], true);
        assert_eq!(prompt["reason"], "Preview the sign-up page");

        let allowed = fixture(Some("once")).await;
        let servers = ServerManager::new(runtime.clone(), Some(allowed.manager.clone()), "l1");
        let report = servers
            .launch(request(with_relay), AgentId::Programmer, process())
            .await
            .unwrap();
        assert_eq!(report["relays"][0]["approval"], "once");
        assert_eq!(
            report["relays"][0]["endpoints"],
            json!(["127.0.0.1:3000", "[::1]:3000"])
        );
        let publications: Vec<String> = runtime.started.lock().unwrap()[0]
            .publications
            .iter()
            .map(Publication::podman_argument)
            .collect();
        assert_eq!(
            publications,
            ["127.0.0.1:3000:5173/tcp", "[::1]:3000:5173/tcp"]
        );
        assert!(report["summary"]
            .as_str()
            .unwrap()
            .contains("from this computer at 127.0.0.1:3000 and [::1]:3000."));
    }

    #[tokio::test]
    async fn a_repeated_launch_returns_the_running_server_and_a_different_one_is_refused() {
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = ServerManager::new(runtime.clone(), None, "l1");
        servers
            .launch(request(json!({})), AgentId::Programmer, process())
            .await
            .unwrap();
        let again = servers
            .launch(
                request(json!({"ready_timeout_seconds": 5})),
                AgentId::QaTester,
                process(),
            )
            .await
            .unwrap();
        assert_eq!(again["ready"], true);
        assert_eq!(runtime.started.lock().unwrap().len(), 1);
        let other = servers
            .launch(
                request(json!({"command": "python3 -m http.server 5173"})),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap_err();
        assert_eq!(other.code, "server_exists");
        let relayed = servers
            .launch(
                request(
                    json!({"relays": [{"protocol": "tcp", "host_port": 3000, "guest_port": 5173}]}),
                ),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap_err();
        assert_eq!(relayed.code, "server_exists");
    }

    #[tokio::test]
    async fn relays_need_a_relay_manager_and_launches_need_the_runtime() {
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = ServerManager::new(runtime.clone(), None, "l1");
        let error = servers
            .launch(
                request(
                    json!({"relays": [{"protocol": "tcp", "host_port": 3000, "guest_port": 5173}]}),
                ),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "relay_unavailable");
        let unavailable = Arc::new(FakeRuntime {
            unavailable: Some("the project network could not be created".into()),
            ..FakeRuntime::listening()
        });
        let servers = ServerManager::new(unavailable, None, "l1");
        let error = servers
            .launch(request(json!({})), AgentId::Programmer, process())
            .await
            .unwrap_err();
        assert_eq!(error.code, "server_failed");
        assert!(error.message.contains("project network"));
        let invalid = servers
            .launch(
                request(json!({"cwd": "../x"})),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            invalid.code, "server_failed",
            "availability is checked first"
        );
    }

    #[tokio::test]
    async fn the_limit_counts_every_running_server_of_the_project() {
        let runtime = Arc::new(FakeRuntime::listening());
        *runtime.running.lock().unwrap() = (0..MAX_SERVERS)
            .map(|index| RunningServer {
                name: format!("s{index}"),
                container: format!("clyean-p-server-s{index}"),
                persistent: true,
                launch_id: "other".into(),
                requested_by: "user-assistant".into(),
                started_at: 0,
                definition: None,
            })
            .collect();
        let servers = ServerManager::new(runtime, None, "l1");
        let error = servers
            .launch(request(json!({})), AgentId::Programmer, process())
            .await
            .unwrap_err();
        assert_eq!(error.code, "server_limit");
    }

    #[tokio::test]
    async fn a_command_that_ends_first_fails_with_its_status_and_last_lines() {
        let runtime = Arc::new(FakeRuntime {
            log: "sh: 1: npm: not found\n[clyean] the server's command exited with status 127\n"
                .into(),
            ..FakeRuntime::default()
        });
        let servers = ServerManager::new(runtime, None, "l1");
        let error = servers
            .launch(request(json!({})), AgentId::Programmer, process())
            .await
            .unwrap_err();
        assert_eq!(error.code, "server_failed");
        assert!(
            error.message.contains("exited with status 127"),
            "{}",
            error.message
        );
        assert!(error.message.contains("npm: not found"));
        assert!(error.message.contains(".clyean/logs/servers/web.log"));
    }

    #[tokio::test]
    async fn a_server_not_ready_in_time_still_runs_and_says_why() {
        let loopback_only = LISTENING_TABLES.replace("00000000:1435", "0100007F:1435");
        let runtime = Arc::new(FakeRuntime {
            tables: Mutex::new(Some(loopback_only)),
            ..FakeRuntime::default()
        });
        let servers = ServerManager::new(runtime.clone(), None, "l1");
        let report = servers
            .launch(
                request(json!({"ready_timeout_seconds": 1})),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap();
        assert_eq!(report["type"], "server_started");
        assert_eq!(report["ready"], false);
        assert_eq!(report["sandbox_endpoints"][0]["state"], "loopback_only");
        let summary = report["summary"].as_str().unwrap();
        assert!(
            summary.starts_with("Server web runs but is not ready yet"),
            "{summary}"
        );
        assert!(summary.contains("vite --host 0.0.0.0"));
        assert!(runtime.stopped.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn clyean_stops_only_its_own_non_persistent_servers_as_it_exits() {
        let runtime = Arc::new(FakeRuntime::listening());
        let server = |name: &str, persistent: bool, launch: &str| RunningServer {
            name: name.into(),
            container: format!("clyean-p-server-{name}"),
            persistent,
            launch_id: launch.into(),
            requested_by: "programmer".into(),
            started_at: 0,
            definition: None,
        };
        *runtime.running.lock().unwrap() = vec![
            server("mine", false, "l1"),
            server("kept", true, "l1"),
            server("theirs", false, "l2"),
        ];
        let servers = ServerManager::new(runtime.clone(), None, "l1");
        let remaining: Vec<String> = servers
            .finish()
            .await
            .unwrap()
            .into_iter()
            .map(|server| server.name)
            .collect();
        assert_eq!(remaining, ["kept", "theirs"]);
        assert_eq!(*runtime.stopped.lock().unwrap(), ["clyean-p-server-mine"]);
        assert_eq!(
            servers.stop("gone").await.unwrap()["type"],
            "server_stopped"
        );
        assert_eq!(servers.stop_all().await.unwrap(), ["kept", "theirs"]);
        assert!(servers.stop("Bad Name").await.is_err());
    }

    #[tokio::test]
    async fn listings_report_endpoints_requesters_and_launches() {
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = ServerManager::new(runtime.clone(), None, "l1");
        servers
            .launch(
                request(json!({"persistent": true})),
                AgentId::Programmer,
                process(),
            )
            .await
            .unwrap();
        let listing = servers.list().await.unwrap();
        let listed = &listing["servers"][0];
        assert_eq!(listing["type"], "server_list");
        assert_eq!(listed["name"], "web");
        assert_eq!(listed["command"], "npm run dev -- --host 0.0.0.0");
        assert_eq!(listed["cwd"], "app");
        assert_eq!(listed["requested_by"], "programmer");
        assert_eq!(listed["launched_by_this_process"], true);
        assert_eq!(listed["sandbox_endpoints"][0]["state"], "listening");
        assert_eq!(listed["started_at"], "2026-09-26T18:21:45Z");
    }

    #[tokio::test]
    async fn sub_agent_requests_are_announced_on_their_unit_of_work() {
        let runtime = Arc::new(FakeRuntime::listening());
        let servers = Arc::new(ServerManager::new(runtime, None, "l1"));
        let registry = Arc::new(WorkRegistry::default());
        let (handle, _answers) = WorkHandle::new(WorkId::from_string("w1"));
        handle.record_phase(Phase::ImplementationProgramming);
        let mut events = handle.subscribe();
        registry.insert(handle).await;
        let requests = ServerRequests {
            servers,
            agent: AgentId::Programmer,
            work_id: "w1".into(),
            registry,
        };
        let frame = |placeholder: Value| {
            RpcFrame(
                json!({"type": "extension_ui_request", "id": "u1", "method": "input",
                "title": SERVER_REQUEST_TITLE, "placeholder": placeholder.to_string()}),
            )
        };
        let mut params = launch_params(json!({}));
        params["action"] = json!("launch");
        let answer = requests.answer(&frame(params)).await.unwrap();
        let report: Value = serde_json::from_str(answer["value"].as_str().unwrap()).unwrap();
        assert_eq!(report["type"], "server_started");
        let mut announced = Vec::new();
        for _ in 0..2 {
            match events.recv().await.unwrap() {
                crate::protocol::StreamedEvent::Progress {
                    agent, phase, text, ..
                } => {
                    assert_eq!(agent, "programmer");
                    assert_eq!(phase, "implementation.programming");
                    announced.push(text);
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert!(announced[0].starts_with("asks to launch server web (npm run dev -- --host 0.0.0.0), listening on TCP 5173 with no relays, stopped when Clyean exits"), "{}", announced[0]);
        assert!(announced[1].starts_with("Server web is ready"));

        let listing = requests
            .answer(&frame(json!({"action": "list"})))
            .await
            .unwrap();
        let listing: Value = serde_json::from_str(listing["value"].as_str().unwrap()).unwrap();
        assert_eq!(listing["servers"][0]["name"], "web");
        let refused = requests
            .answer(&frame(json!({"action": "explode"})))
            .await
            .unwrap();
        let refused: Value = serde_json::from_str(refused["value"].as_str().unwrap()).unwrap();
        assert_eq!(refused["error"]["code"], "server_invalid");
        let other = RpcFrame(
            json!({"type": "extension_ui_request", "id": "u2", "title": "clyean:credentials"}),
        );
        assert!(requests.answer(&other).await.is_none());
    }
}
