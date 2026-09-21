// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The Unix domain socket transport of the orchestrator protocol: one request per
//! connection, an immediate response, then streamed events until a closing event.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::protocol::{Request, Response, StreamedEvent};
use crate::service::{Dispatch, OrchestratorService};
use crate::{OrchestratorError, Result};

const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
pub async fn serve(
    service: Arc<OrchestratorService>,
    socket_path: &Path,
    shutdown: CancellationToken,
) -> Result<()> {
    use tokio::net::UnixListener;

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| OrchestratorError::io(format!("creating {}", parent.display()), e))?;
    }
    if socket_path.exists() {
        std::fs::remove_file(socket_path).map_err(|e| {
            OrchestratorError::io(
                format!("removing stale socket {}", socket_path.display()),
                e,
            )
        })?;
    }
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| OrchestratorError::io(format!("binding {}", socket_path.display()), e))?;
    tracing::info!(target: "clyean::orchestrator", socket = %socket_path.display(), "orchestrator listening");
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|e| OrchestratorError::io("accepting a connection", e))?;
                let service = service.clone();
                tokio::spawn(async move {
                    let (reader, writer) = stream.into_split();
                    if let Err(error) = handle_connection(service, reader, writer).await {
                        tracing::debug!(target: "clyean::orchestrator", %error, "connection ended with an error");
                    }
                });
            }
        }
    }
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

#[cfg(not(unix))]
pub async fn serve(
    _service: Arc<OrchestratorService>,
    _socket_path: &Path,
    _shutdown: CancellationToken,
) -> Result<()> {
    Err(OrchestratorError::Workflow(
        "the orchestrator socket requires a Unix host in this version".into(),
    ))
}

/// Serves one connection over any byte stream (exercised directly by tests).
pub async fn handle_connection<R, W>(
    service: Arc<OrchestratorService>,
    reader: R,
    mut writer: W,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    let line = match tokio::time::timeout(REQUEST_READ_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => return Ok(()),
        Ok(Err(error)) => return Err(error),
        Err(_) => return Ok(()),
    };
    let request: Request = match serde_json::from_str(&line) {
        Ok(request) => request,
        Err(error) => {
            let response =
                Response::error("", "invalid_request", format!("malformed request: {error}"));
            return write_json(&mut writer, &response).await;
        }
    };
    let Dispatch { response, stream } = service.dispatch(request).await;
    write_json(&mut writer, &response).await?;
    let Some(attachment) = stream else {
        return Ok(());
    };
    for event in attachment.replay {
        write_json(&mut writer, &event).await?;
        if event.closes_connection() {
            return Ok(());
        }
    }
    let mut receiver = attachment.receiver;
    loop {
        match receiver.recv().await {
            Ok(event) => {
                write_json(&mut writer, &event).await?;
                if event.closes_connection() {
                    return Ok(());
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => {
                let closed = StreamedEvent::Failed {
                    work_id: String::new(),
                    seq: u64::MAX,
                    code: "internal".into(),
                    message: "the work ended without a terminal event".into(),
                };
                return write_json(&mut writer, &closed).await;
            }
        }
    }
}

async fn write_json<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).expect("protocol values serialize");
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}
