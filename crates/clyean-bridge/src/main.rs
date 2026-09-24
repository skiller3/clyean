// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! `clyean-bridge`, the container end of the bridge.  It runs only inside Linux containers:
//!
//! ```text
//! clyean-bridge bridge --ready-file PATH [--channel NAME=PATH]...
//! clyean-bridge await --ready-file PATH [--timeout-seconds N] -- COMMAND [ARGUMENT]...
//! clyean-bridge connect SOCKET
//! clyean-bridge --version
//! ```

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    run(arguments)
}

#[cfg(not(unix))]
fn run(_arguments: Vec<OsString>) -> ExitCode {
    eprintln!("clyean-bridge runs only inside Linux containers");
    ExitCode::from(2)
}

#[cfg(unix)]
fn run(arguments: Vec<OsString>) -> ExitCode {
    let mut arguments = arguments.into_iter();
    let Some(subcommand) = arguments.next() else {
        return usage("a subcommand is required");
    };
    let rest: Vec<OsString> = arguments.collect();
    match subcommand.to_str() {
        Some("--version") => {
            println!("clyean-bridge {}", clyean_bridge::VERSION);
            ExitCode::SUCCESS
        }
        Some("bridge") => bridge(&rest),
        Some("await") => await_bridge(rest),
        Some("connect") => connect(&rest),
        _ => usage(&format!("unknown subcommand {subcommand:?}")),
    }
}

#[cfg(unix)]
fn bridge(arguments: &[OsString]) -> ExitCode {
    use clyean_bridge::container::{self, Channel};

    let mut channels = Vec::new();
    let mut ready_file = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let value = arguments.next().and_then(|value| value.to_str());
        match (argument.to_str(), value) {
            (Some("--channel"), Some(spec)) => match Channel::parse(spec) {
                Ok(channel) => channels.push(channel),
                Err(message) => return usage(&message),
            },
            (Some("--ready-file"), Some(path)) => ready_file = Some(std::path::PathBuf::from(path)),
            _ => return usage(&format!("unexpected bridge argument {argument:?}")),
        }
    }
    let Some(ready_file) = ready_file else {
        return usage("bridge needs --ready-file");
    };
    match container::run(&channels, &ready_file, std::io::stdin(), std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("clyean-bridge: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(unix)]
fn await_bridge(arguments: Vec<OsString>) -> ExitCode {
    use clyean_bridge::gate;

    let Some(separator) = arguments.iter().position(|argument| argument == "--") else {
        return usage("await needs -- before the command to run");
    };
    let (options, command) = arguments.split_at(separator);
    let command = &command[1..];
    let mut ready_file = None;
    let mut timeout = gate::DEFAULT_TIMEOUT;
    let mut options = options.iter();
    while let Some(option) = options.next() {
        let value = options.next().and_then(|value| value.to_str());
        match (option.to_str(), value) {
            (Some("--ready-file"), Some(path)) => ready_file = Some(std::path::PathBuf::from(path)),
            (Some("--timeout-seconds"), Some(seconds)) => match seconds.parse() {
                Ok(seconds) => timeout = std::time::Duration::from_secs(seconds),
                Err(_) => {
                    return usage(&format!("--timeout-seconds takes a number, not {seconds}"))
                }
            },
            _ => return usage(&format!("unexpected await argument {option:?}")),
        }
    }
    let Some(ready_file) = ready_file else {
        return usage("await needs --ready-file");
    };
    if command.is_empty() {
        return usage("await needs a command after --");
    }
    if !gate::wait_for_ready(&ready_file, timeout) {
        eprintln!(
            "clyean-bridge: the bridge was not ready within {} seconds, so the harness was not started; the clyean process that started this container could not open its bridge",
            timeout.as_secs()
        );
        return ExitCode::from(125);
    }
    let error = gate::exec(command);
    eprintln!("clyean-bridge: could not start {:?}: {error}", command[0]);
    ExitCode::from(126)
}

#[cfg(unix)]
fn connect(arguments: &[OsString]) -> ExitCode {
    let [socket] = arguments else {
        return usage("connect takes one socket path");
    };
    match clyean_bridge::container::connect(std::path::Path::new(socket)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("clyean-bridge: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(unix)]
fn usage(problem: &str) -> ExitCode {
    eprintln!("clyean-bridge: {problem}");
    eprintln!("usage: clyean-bridge bridge --ready-file PATH [--channel NAME=PATH]...");
    eprintln!("       clyean-bridge await --ready-file PATH [--timeout-seconds N] -- COMMAND [ARGUMENT]...");
    eprintln!("       clyean-bridge connect SOCKET");
    eprintln!("       clyean-bridge --version");
    ExitCode::from(2)
}
