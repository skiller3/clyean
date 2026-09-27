// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The container end and the host end of the bridge, connected in one process: a client of
//! a socket the bridge serves talks to a host-side service through a stream, and a client
//! of a host relay reaches a server on the "container's" loopback.

#![cfg(all(unix, feature = "host"))]

use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clyean_bridge::container::{self, Channel};
use clyean_bridge::host::{serve, BridgeHandle, ConnectFuture, Connector, LocalStream};
use clyean_bridge::protocol::{Peer, RelayProtocol};
use clyean_bridge::relay::{BoundRelay, RelaySpec};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Answers every line on the `orchestrator` channel with `pong:<line>`, and records the
/// peers the bridge reports.
#[derive(Default)]
struct PongConnector {
    peers: Mutex<Vec<Peer>>,
}

impl Connector for PongConnector {
    fn connect<'a>(&'a self, channel: &'a str, peer: Option<&'a Peer>) -> ConnectFuture<'a> {
        Box::pin(async move {
            if channel != "orchestrator" {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such channel"));
            }
            if let Some(peer) = peer {
                self.peers.lock().unwrap().push(peer.clone());
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

struct Session {
    socket_path: PathBuf,
    handle: BridgeHandle,
    connector: Arc<PongConnector>,
    _dir: tempfile::TempDir,
}

/// Runs both ends of a bridge in this process, connected by a socket pair.
async fn start() -> Session {
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
    let (introduced, introduction) = tokio::sync::oneshot::channel();
    let connector = Arc::new(PongConnector::default());
    tokio::spawn(serve(
        host_reader,
        host_writer,
        connector.clone(),
        introduced,
    ));
    let introduction = introduction.await.unwrap();
    assert_eq!(introduction.version, clyean_bridge::VERSION);

    for _ in 0..200 {
        if ready_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Session {
        socket_path,
        handle: introduction.handle,
        connector,
        _dir: dir,
    }
}

#[tokio::test]
async fn a_client_inside_reaches_the_host_service_through_the_bridge() {
    let session = start().await;
    let client = tokio::net::UnixStream::connect(&session.socket_path)
        .await
        .unwrap();
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
    let expected_peer = cfg!(target_os = "linux").then(std::process::id);
    assert_eq!(
        session
            .connector
            .peers
            .lock()
            .unwrap()
            .first()
            .map(|peer| peer.pid),
        expected_peer,
        "the host learns which process connected wherever the kernel reports it"
    );
}

#[tokio::test]
async fn a_host_relay_reaches_a_server_on_the_container_loopback() {
    let server = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let guest_port = server.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut connection, _) = server.accept().await.unwrap();
        let mut request = [0u8; 5];
        connection.read_exact(&mut request).await.unwrap();
        connection.write_all(b"hello from the guest").await.unwrap();
    });
    let session = start().await;
    let bound = BoundRelay::bind(RelaySpec {
        protocol: RelayProtocol::Tcp,
        host_port: 0,
        guest_port,
        addresses: vec![Ipv4Addr::LOCALHOST.into()],
    })
    .unwrap();
    let relay_address = bound.local_addresses()[0];
    let _relay = bound.activate(session.handle.clone()).unwrap();
    let mut client = tokio::net::TcpStream::connect(relay_address).await.unwrap();
    client.write_all(b"hello").await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, b"hello from the guest");
}
