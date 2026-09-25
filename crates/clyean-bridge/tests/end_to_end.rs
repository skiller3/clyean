// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The container end and the host end of the bridge, connected in one process: a client of
//! a socket the bridge serves talks to a host-side service through a stream.

#![cfg(all(unix, feature = "host"))]

use std::io;
use std::sync::Arc;
use std::time::Duration;

use clyean_bridge::container::{self, Channel};
use clyean_bridge::host::{serve, ConnectFuture, Connector, LocalStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Answers every line on the `orchestrator` channel with `pong:<line>`.
struct PongConnector;

impl Connector for PongConnector {
    fn connect<'a>(&'a self, channel: &'a str) -> ConnectFuture<'a> {
        Box::pin(async move {
            if channel != "orchestrator" {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such channel"));
            }
            let (client, server) = tokio::io::duplex(4096);
            tokio::spawn(async move {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let reply = format!("pong:{line}\n");
                    if writer.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
                let _ = writer.shutdown().await;
            });
            Ok(Box::new(client) as Box<dyn LocalStream>)
        })
    }
}

#[tokio::test]
async fn a_client_inside_reaches_the_host_service_through_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("orchestrator.sock");
    let ready_file = dir.path().join("bridge.ready");

    let (host_end, bridge_end) = std::os::unix::net::UnixStream::pair().unwrap();
    let bridge_input = bridge_end.try_clone().unwrap();
    let channels = vec![Channel {
        name: "orchestrator".into(),
        socket_path: socket_path.clone(),
    }];
    let ready = ready_file.clone();
    std::thread::spawn(move || container::run(&channels, &ready, bridge_input, bridge_end));

    host_end.set_nonblocking(true).unwrap();
    let host_end = tokio::net::UnixStream::from_std(host_end).unwrap();
    let (host_reader, host_writer) = host_end.into_split();
    let (introduced, version) = tokio::sync::oneshot::channel();
    tokio::spawn(serve(
        host_reader,
        host_writer,
        Arc::new(PongConnector),
        introduced,
    ));
    assert_eq!(version.await.unwrap(), clyean_bridge::VERSION);

    for _ in 0..200 {
        if ready_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    for message in ["ping", "again"] {
        writer
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.as_deref(), Some(format!("pong:{message}").as_str()));
    }
    writer.shutdown().await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(end, None);
}
