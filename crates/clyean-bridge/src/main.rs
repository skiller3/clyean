// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! `clyean-bridge`, the container end of the bridge.  It runs only inside Linux containers:
//!
//! ```text
//! clyean-bridge bridge --ready-file PATH [--channel NAME=PATH]...
//! clyean-bridge await --ready-file PATH [--timeout-seconds N] -- COMMAND [ARGUMENT]...
//! clyean-bridge connect SOCKET
//! clyean-bridge relay open PROTOCOL HOST_PORT GUEST_PORT [--host-address ADDRESS]... [--reason TEXT]
//! clyean-bridge relay close RELAY_ID
//! clyean-bridge relay list
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
        Some("relay") => relay(&rest),
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

/// `relay open|close|list`: one request to the orchestrator socket, whose result, or
/// error, is printed as one line of JSON.
#[cfg(unix)]
fn relay(arguments: &[OsString]) -> ExitCode {
    let arguments: Vec<&str> = match arguments.iter().map(|a| a.to_str()).collect::<Option<_>>() {
        Some(arguments) => arguments,
        None => return usage("relay arguments must be UTF-8"),
    };
    let (method, params) = match arguments.as_slice() {
        ["open", protocol, host_port, guest_port, options @ ..] => {
            match relay_open_params(protocol, host_port, guest_port, options) {
                Ok(params) => ("relay.open", params),
                Err(problem) => return usage(&problem),
            }
        }
        ["close", relay_id] => ("relay.close", serde_json::json!({"relay_id": relay_id})),
        ["list"] => ("relay.list", serde_json::json!({})),
        _ => return usage("relay takes open, close, or list"),
    };
    print_reply(method, params)
}

#[cfg(unix)]
fn relay_open_params(
    protocol: &str,
    host_port: &str,
    guest_port: &str,
    options: &[&str],
) -> Result<serde_json::Value, String> {
    let port = |text: &str| -> Result<u16, String> {
        text.parse()
            .map_err(|_| format!("{text:?} is not a port from 1 to 65535"))
    };
    let mut params = serde_json::json!({
        "protocol": protocol,
        "host_port": port(host_port)?,
        "guest_port": port(guest_port)?,
    });
    let mut addresses = Vec::new();
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match (*option, options.next()) {
            ("--host-address", Some(address)) => addresses.push(address.to_string()),
            ("--reason", Some(reason)) => params["reason"] = serde_json::json!(reason),
            _ => return Err(format!("unexpected relay argument {option:?}")),
        }
    }
    if !addresses.is_empty() {
        params["host_addresses"] = serde_json::json!(addresses);
    }
    Ok(params)
}

/// Sends one request and prints its result on standard output; an error is printed there
/// too, with its message on standard error and a failing exit status.
#[cfg(unix)]
fn print_reply(method: &str, params: serde_json::Value) -> ExitCode {
    use clyean_bridge::client;
    match client::request(&client::socket_path(), method, params) {
        Ok(Ok(result)) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Ok(Err(error)) => {
            println!("{error}");
            let message = error["message"].as_str().unwrap_or("the request failed");
            eprintln!("clyean-bridge: {message}");
            ExitCode::FAILURE
        }
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
    eprintln!("       clyean-bridge relay open PROTOCOL HOST_PORT GUEST_PORT [--host-address ADDRESS]... [--reason TEXT]");
    eprintln!("       clyean-bridge relay close RELAY_ID");
    eprintln!("       clyean-bridge relay list");
    eprintln!("       clyean-bridge --version");
    ExitCode::from(2)
}
