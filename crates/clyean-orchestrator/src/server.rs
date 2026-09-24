// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The connection handling of the orchestrator protocol: one request per connection, an
//! immediate response, then streamed events until a closing event.  Connections arrive
//! through the bridge of the User Assistant's container, so any byte stream serves.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use crate::protocol::{Request, Response, StreamedEvent};
use crate::service::{Dispatch, OrchestratorService};

/// The method a User Assistant calls once and keeps open for its whole life; the
/// connection ending tells it that its `clyean` process is gone.
pub const LEASE_METHOD: &str = "session.lease";

const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);

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
    if request.method == LEASE_METHOD {
        let response = Response::result(&request.id, json!({"type": "lease"}));
        write_json(&mut writer, &response).await?;
        while let Ok(Some(_)) = lines.next_line().await {}
        return Ok(());
    }
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
