// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A client for the contained harness's RPC mode: newline-delimited JSON over the stdio of
//! a harness child process.  The client sends commands, correlates responses by `id`,
//! reassembles protocol v2 chunked frames, and turns a prompt into a completed turn with
//! streamed text.

pub mod client;
pub mod frames;
pub mod session;

pub use client::{HarnessClient, HarnessEvent, HarnessOutput, UiRequestHandler};
pub use frames::{FrameDecoder, RpcFrame};
pub use session::{AgentSessionDriver, HarnessSession, SessionInfo, TurnOutcome, TurnProgress};

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("failed to spawn the harness process: {0}")]
    Spawn(std::io::Error),
    #[error("the harness process exited before it became ready: {0}")]
    ExitedBeforeReady(String),
    #[error("the harness closed its output stream")]
    OutputClosed,
    #[error("the harness rejected {command}: {message}")]
    CommandFailed { command: String, message: String },
    #[error("malformed frame from the harness: {0}")]
    MalformedFrame(String),
    #[error("timed out waiting for the harness ({0})")]
    Timeout(String),
    #[error("i/o error talking to the harness: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, HarnessError>;
