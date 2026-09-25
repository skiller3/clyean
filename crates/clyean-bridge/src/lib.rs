// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The bridge between a User Assistant container and the `clyean` process that started
//! it.  One `podman exec -i` session carries every channel: the bridge serves Unix sockets
//! inside the container, and each connection to them becomes a stream multiplexed over the
//! session's standard input and output to the host, which connects it to the orchestrator
//! or to the Herdr socket.  When the host process dies, the session's input closes, the
//! bridge exits, and every connection it proxied ends, which is how the User Assistant
//! learns that its owner is gone.

pub mod protocol;

#[cfg(unix)]
pub mod container;
#[cfg(unix)]
pub mod gate;
#[cfg(feature = "host")]
pub mod host;

/// Where the bridge executable is mounted, read-only, inside User Assistant containers.
pub const CONTAINER_PATH: &str = "/usr/local/libexec/clyean/clyean-bridge";

/// The file the bridge creates once every socket it serves accepts connections.  It lives
/// on the container's private in-memory `/run/clyean`, so it never outlives a container.
pub const READY_FILE: &str = "/run/clyean/bridge.ready";

/// The channel carrying the orchestrator protocol.
pub const ORCHESTRATOR_CHANNEL: &str = "orchestrator";

/// The channel carrying the Herdr socket API, passed through unmodified.
pub const HERDR_CHANNEL: &str = "herdr";

/// The version of the bridge executable, which matches the `clyean` release it ships with.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
