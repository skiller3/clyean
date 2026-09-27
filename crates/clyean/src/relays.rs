// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The host's side of relays: the orchestrator's relay manager decides, and this module
//! carries its decisions out through the bridge's relay listeners, and opens sign-in pages
//! in the user's browser.

use std::net::{Ipv6Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use clyean_bridge::host::BridgeHandle;
use clyean_bridge::protocol::RelayProtocol as BridgeProtocol;
use clyean_bridge::relay::{self, ActiveRelay, BindError, BoundRelay};
use clyean_orchestrator::allowlist::{RelayProtocol, RelaySpec};
use clyean_orchestrator::relays::{
    BrowserOpener, LiveRelay, PendingRelay, RelayTransport, TransportError,
};

/// Relays through the bridge of this process's User Assistant, once it is open.
#[derive(Default)]
pub struct BridgeRelayTransport {
    bridge: Arc<Mutex<Option<BridgeHandle>>>,
}

impl BridgeRelayTransport {
    /// Carries relays through `bridge` from now on.
    pub fn attach(&self, bridge: BridgeHandle) {
        *self.bridge.lock().expect("unpoisoned") = Some(bridge);
    }
}

fn bridge_protocol(protocol: RelayProtocol) -> BridgeProtocol {
    match protocol {
        RelayProtocol::Tcp => BridgeProtocol::Tcp,
        RelayProtocol::Udp => BridgeProtocol::Udp,
        RelayProtocol::Sctp => BridgeProtocol::Sctp,
    }
}

fn bridge_spec(spec: &RelaySpec) -> relay::RelaySpec {
    relay::RelaySpec {
        protocol: bridge_protocol(spec.protocol),
        host_port: spec.host_port,
        guest_port: spec.guest_port,
        addresses: spec.host_addresses.clone(),
    }
}

fn transport_error(error: BindError) -> TransportError {
    match error {
        BindError::PortUnavailable(message) => TransportError::PortUnavailable(message),
        BindError::AddressUnavailable(message) => TransportError::AddressUnavailable(message),
        BindError::ProtocolUnsupported(message) => TransportError::ProtocolUnsupported(message),
        BindError::Other(error) => TransportError::Other(error.to_string()),
    }
}

impl RelayTransport for BridgeRelayTransport {
    fn supports(&self, protocol: RelayProtocol) -> bool {
        relay::protocol_supported(bridge_protocol(protocol))
    }

    fn bind(&self, spec: &RelaySpec) -> Result<Box<dyn PendingRelay>, TransportError> {
        let bound = BoundRelay::bind(bridge_spec(spec)).map_err(transport_error)?;
        Ok(Box::new(Pending {
            bound,
            bridge: self.bridge.clone(),
        }))
    }

    fn check_available(&self, spec: &RelaySpec) -> Result<(), TransportError> {
        relay::check_available(&bridge_spec(spec)).map_err(transport_error)
    }

    fn has_ipv6_loopback(&self) -> bool {
        TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, 0))).is_ok()
    }
}

struct Pending {
    bound: BoundRelay,
    bridge: Arc<Mutex<Option<BridgeHandle>>>,
}

impl PendingRelay for Pending {
    fn activate(self: Box<Self>) -> Result<Box<dyn LiveRelay>, TransportError> {
        let bridge = self
            .bridge
            .lock()
            .expect("unpoisoned")
            .clone()
            .filter(BridgeHandle::is_open)
            .ok_or_else(|| {
                TransportError::Unavailable("the User Assistant's bridge is not open".into())
            })?;
        let active = self
            .bound
            .activate(bridge)
            .map_err(|error| TransportError::Other(error.to_string()))?;
        Ok(Box::new(Live(active)))
    }
}

struct Live(ActiveRelay);

impl LiveRelay for Live {
    fn connections(&self) -> usize {
        self.0.connections()
    }

    fn close(&self) {
        self.0.close();
    }
}

/// Opens pages with the platform's opener, detached and silent; a missing or failing
/// opener is only logged, since the login dialog shows the address too.
pub struct SystemBrowserOpener;

impl BrowserOpener for SystemBrowserOpener {
    fn open(&self, url: &str) {
        let Some(command) = opener_command(url, &Platform::current()) else {
            tracing::debug!(target: "clyean::relays", %url, "no browser opener on this host");
            return;
        };
        let spawned = std::process::Command::new(&command[0])
            .args(&command[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            // The opener exits on its own; reaping it keeps no zombie behind.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => {
                tracing::debug!(target: "clyean::relays", %url, %error, "the browser opener failed")
            }
        }
    }
}

/// What decides how a page opens on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: &'static str,
    /// Set inside a WSL distribution.
    pub wsl: bool,
    /// Whether `wslview` is on `PATH`.
    pub wslview: bool,
}

impl Platform {
    fn current() -> Self {
        let wsl = std::env::var_os("WSL_DISTRO_NAME").is_some();
        Self {
            os: std::env::consts::OS,
            wsl,
            wslview: wsl && on_path("wslview"),
        }
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
    })
}

/// The command that opens `url` on `platform`: `xdg-open` on Linux, `wslview` or
/// Windows' `explorer.exe` inside WSL, `open` on macOS, and `explorer.exe` on Windows.
pub fn opener_command(url: &str, platform: &Platform) -> Option<Vec<String>> {
    let program = match (platform.os, platform.wsl) {
        ("linux", true) if platform.wslview => "wslview",
        ("linux", true) => "explorer.exe",
        ("linux", false) => "xdg-open",
        ("macos", _) => "open",
        ("windows", _) => "explorer.exe",
        _ => return None,
    };
    Some(vec![program.to_string(), url.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(os: &'static str, wsl: bool, wslview: bool) -> Platform {
        Platform { os, wsl, wslview }
    }

    #[test]
    fn each_platform_has_its_opener() {
        let url = "http://localhost:54549/launch";
        let program = |p: Platform| opener_command(url, &p).map(|command| command[0].clone());
        assert_eq!(
            program(platform("linux", false, false)).as_deref(),
            Some("xdg-open")
        );
        assert_eq!(
            program(platform("linux", true, true)).as_deref(),
            Some("wslview")
        );
        assert_eq!(
            program(platform("linux", true, false)).as_deref(),
            Some("explorer.exe")
        );
        assert_eq!(
            program(platform("macos", false, false)).as_deref(),
            Some("open")
        );
        assert_eq!(
            program(platform("windows", false, false)).as_deref(),
            Some("explorer.exe")
        );
        assert_eq!(program(platform("freebsd", false, false)), None);
        assert_eq!(
            opener_command(url, &platform("macos", false, false)).unwrap()[1],
            url
        );
    }

    #[test]
    fn relays_need_an_open_bridge() {
        let transport = BridgeRelayTransport::default();
        let spec = RelaySpec {
            protocol: RelayProtocol::Tcp,
            host_port: 0,
            guest_port: 80,
            host_addresses: vec![std::net::Ipv4Addr::LOCALHOST.into()],
        };
        let pending = transport.bind(&spec).unwrap();
        assert!(matches!(
            pending.activate(),
            Err(TransportError::Unavailable(_))
        ));
        assert!(transport.supports(RelayProtocol::Tcp));
    }
}
