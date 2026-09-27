// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A command-line client of the orchestrator socket, for scripts in the User Assistant's
//! container: `clyean-bridge relay ...` and `clyean-bridge server ...` send one request and
//! print its result, or its error, as one line of JSON.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Where the orchestrator socket is: `CLYEAN_ORCHESTRATOR_SOCKET`, or its fixed path.
pub fn socket_path() -> PathBuf {
    std::env::var_os("CLYEAN_ORCHESTRATOR_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::ORCHESTRATOR_SOCKET))
}

/// Sends one request and returns its result, or its error object.
pub fn request(socket: &Path, method: &str, params: Value) -> io::Result<Result<Value, Value>> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "the orchestrator socket {} is not reachable ({error}); only the User Assistant's container has one",
                socket.display()
            ),
        )
    })?;
    let id = format!("cli-{}", std::process::id());
    let line = json!({"id": id, "method": method, "params": params}).to_string();
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    let reply: Value = serde_json::from_str(reply.trim()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the orchestrator's reply is not JSON: {error}"),
        )
    })?;
    match (reply.get("result"), reply.get("error")) {
        (Some(result), _) => Ok(Ok(result.clone())),
        (None, Some(error)) => Ok(Err(error.clone())),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the orchestrator's reply has neither a result nor an error",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn answer_once(
        reply: &'static str,
    ) -> (PathBuf, std::thread::JoinHandle<Value>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(reply.as_bytes()).unwrap();
            serde_json::from_str(&line).unwrap()
        });
        (path, server, dir)
    }

    #[test]
    fn a_result_and_an_error_come_back_apart() {
        let (path, server, _dir) = answer_once(
            "{\"id\":\"x\",\"result\":{\"type\":\"relay_open\",\"relay_id\":\"r-1\"}}\n",
        );
        let result = request(&path, "relay.open", json!({"protocol": "tcp"})).unwrap();
        assert_eq!(result.unwrap()["relay_id"], "r-1");
        let sent = server.join().unwrap();
        assert_eq!(sent["method"], "relay.open");
        assert_eq!(sent["params"]["protocol"], "tcp");

        let (path, _server, _dir) = answer_once(
            "{\"id\":\"x\",\"error\":{\"code\":\"relay_denied\",\"message\":\"no\",\"reason\":\"denied\"}}\n",
        );
        let error = request(&path, "relay.open", json!({}))
            .unwrap()
            .unwrap_err();
        assert_eq!(error["reason"], "denied");
    }

    #[test]
    fn an_absent_socket_is_explained() {
        let error = request(
            Path::new("/nonexistent/orchestrator.sock"),
            "relay.list",
            json!({}),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("only the User Assistant's container"));
    }
}
