// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The start gate of a User Assistant container.  The container's command waits here until
//! the bridge reports that its sockets accept connections and then replaces itself with the
//! harness, so the harness never runs without its bridge and remains the only child of the
//! container's init process.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long a container waits for its bridge by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Waits until `ready_file` exists, returning whether it appeared within `timeout`.
pub fn wait_for_ready(ready_file: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if ready_file.exists() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Replaces the current process with `command`; it returns only when that fails.
pub fn exec(command: &[OsString]) -> io::Error {
    use std::os::unix::process::CommandExt;
    let Some((program, arguments)) = command.split_first() else {
        return io::Error::new(io::ErrorKind::InvalidInput, "no command to run");
    };
    std::process::Command::new(program).args(arguments).exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_ends_when_the_ready_file_appears() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("bridge.ready");
        let writer = {
            let ready = ready.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                std::fs::write(ready, b"ready\n").unwrap();
            })
        };
        assert!(wait_for_ready(&ready, Duration::from_secs(5)));
        writer.join().unwrap();
    }

    #[test]
    fn waiting_gives_up_after_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        assert!(!wait_for_ready(
            &dir.path().join("never"),
            Duration::from_millis(100)
        ));
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn an_empty_command_cannot_be_run() {
        assert_eq!(exec(&[]).kind(), io::ErrorKind::InvalidInput);
    }
}
