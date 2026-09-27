// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The host's listeners for relays into the User Assistant's container.  A relay binds and
//! listens on its host addresses first, which holds the port exclusively and reports a
//! busy one before anyone is asked to approve it, and accepts connections only once
//! activated: until then they wait in the kernel's backlog, and closing an unapproved
//! relay resets them.  Each accepted TCP connection,
//! UDP flow, or SCTP association then becomes a stream the host opens through the bridge,
//! which the container end connects to the guest port on its loopback.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};

use crate::host::BridgeHandle;
use crate::protocol::{Message, MessageReader, RelayProtocol, RelayTarget};

/// The most TCP connections one relay carries at once.
pub const MAX_TCP_CONNECTIONS: usize = 64;
/// The most UDP flows, by client address and port, one relay carries at once.
pub const MAX_UDP_FLOWS: usize = 64;
/// How long a UDP flow may stay silent in both directions before it is closed.
pub const UDP_FLOW_IDLE: Duration = Duration::from_secs(120);
/// The most SCTP associations one relay carries at once.
pub const MAX_SCTP_ASSOCIATIONS: usize = 16;

/// How many datagrams wait for one flow's stream before newer ones are dropped.
const FLOW_QUEUE: usize = 64;
/// How often idle UDP flows are looked for.
const FLOW_SWEEP: Duration = Duration::from_secs(10);
/// How long an SCTP accept waits before checking whether its relay was closed.
#[cfg(target_os = "linux")]
const SCTP_ACCEPT_POLL: Duration = Duration::from_millis(500);

/// A relay from one host port, on one or more host addresses, to one guest port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySpec {
    pub protocol: RelayProtocol,
    pub host_port: u16,
    pub guest_port: u16,
    pub addresses: Vec<IpAddr>,
}

impl RelaySpec {
    fn target(&self) -> RelayTarget {
        RelayTarget {
            protocol: self.protocol,
            port: self.guest_port,
        }
    }
}

/// Why a relay's host addresses could not be bound.
#[derive(Debug)]
pub enum BindError {
    /// The port is in use, or below the unprivileged floor.
    PortUnavailable(String),
    /// An address is not assigned to this host.
    AddressUnavailable(String),
    /// This host cannot carry the protocol.
    ProtocolUnsupported(String),
    Other(io::Error),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortUnavailable(message)
            | Self::AddressUnavailable(message)
            | Self::ProtocolUnsupported(message) => f.write_str(message),
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BindError {}

/// Whether this host can listen for `protocol`.  Checking SCTP creates a socket, which can
/// make the kernel load its SCTP module, so it is checked only when a relay asks for it.
pub fn protocol_supported(protocol: RelayProtocol) -> bool {
    match protocol {
        RelayProtocol::Tcp | RelayProtocol::Udp => true,
        #[cfg(target_os = "linux")]
        RelayProtocol::Sctp => crate::sctp::is_available(),
        #[cfg(not(target_os = "linux"))]
        RelayProtocol::Sctp => false,
    }
}

/// Binds and releases `spec`'s addresses, for relays that another program will listen on.
pub fn check_available(spec: &RelaySpec) -> Result<(), BindError> {
    BoundRelay::bind(spec.clone()).map(drop)
}

/// A relay whose addresses are bound but which accepts nothing yet.
pub struct BoundRelay {
    spec: RelaySpec,
    sockets: Vec<BoundSocket>,
}

enum BoundSocket {
    Tcp(Socket),
    Udp(Socket),
    #[cfg(target_os = "linux")]
    Sctp(crate::sctp::Listener),
}

impl BoundRelay {
    pub fn bind(spec: RelaySpec) -> Result<Self, BindError> {
        if !protocol_supported(spec.protocol) {
            return Err(BindError::ProtocolUnsupported(format!(
                "this host cannot relay {}",
                spec.protocol.as_str().to_uppercase()
            )));
        }
        let mut sockets = Vec::with_capacity(spec.addresses.len());
        for address in &spec.addresses {
            let address = SocketAddr::new(*address, spec.host_port);
            sockets.push(bind_one(spec.protocol, address)?);
        }
        Ok(Self { spec, sockets })
    }

    /// The addresses bound, with the ports the system chose for a host port of zero.
    pub fn local_addresses(&self) -> Vec<SocketAddr> {
        self.sockets
            .iter()
            .filter_map(|socket| match socket {
                BoundSocket::Tcp(socket) | BoundSocket::Udp(socket) => {
                    socket.local_addr().ok()?.as_socket()
                }
                #[cfg(target_os = "linux")]
                BoundSocket::Sctp(_) => None,
            })
            .collect()
    }

    /// Starts accepting, carrying everything through `bridge`.  Runs inside a Tokio runtime.
    pub fn activate(self, bridge: BridgeHandle) -> io::Result<ActiveRelay> {
        let (stop, stopped) = watch::channel(false);
        let connections = Arc::new(AtomicUsize::new(0));
        let target = self.spec.target();
        for socket in self.sockets {
            let context = RelayContext {
                bridge: bridge.clone(),
                target,
                stopped: stopped.clone(),
                connections: connections.clone(),
            };
            match socket {
                BoundSocket::Tcp(socket) => {
                    socket.set_nonblocking(true)?;
                    let listener = TcpListener::from_std(socket.into())?;
                    tokio::spawn(accept_tcp(listener, context));
                }
                BoundSocket::Udp(socket) => {
                    socket.set_nonblocking(true)?;
                    let socket = Arc::new(UdpSocket::from_std(socket.into())?);
                    tokio::spawn(relay_udp(socket, context));
                }
                #[cfg(target_os = "linux")]
                BoundSocket::Sctp(listener) => {
                    listener.set_accept_poll(SCTP_ACCEPT_POLL)?;
                    let runtime = tokio::runtime::Handle::current();
                    tokio::task::spawn_blocking(move || accept_sctp(listener, context, runtime));
                }
            }
        }
        Ok(ActiveRelay { stop, connections })
    }
}

fn bind_one(protocol: RelayProtocol, address: SocketAddr) -> Result<BoundSocket, BindError> {
    match protocol {
        RelayProtocol::Tcp => {
            bind_socket(address, Type::STREAM, Protocol::TCP).map(BoundSocket::Tcp)
        }
        RelayProtocol::Udp => {
            bind_socket(address, Type::DGRAM, Protocol::UDP).map(BoundSocket::Udp)
        }
        #[cfg(target_os = "linux")]
        RelayProtocol::Sctp => crate::sctp::Listener::bind(address)
            .map(BoundSocket::Sctp)
            .map_err(|error| classify(error, address)),
        #[cfg(not(target_os = "linux"))]
        RelayProtocol::Sctp => Err(BindError::ProtocolUnsupported(
            "this host cannot relay SCTP".into(),
        )),
    }
}

/// Binds one address, and listens on it for TCP.  IPv6 sockets are IPv6-only, so `::`
/// never takes IPv4 traffic.  TCP sockets reuse addresses on Unix, so a closed relay's
/// port frees at once, and listening at once keeps a second relay off the port; UDP
/// sockets do not reuse addresses, because two could then share a port.
fn bind_socket(address: SocketAddr, kind: Type, protocol: Protocol) -> Result<Socket, BindError> {
    let socket = Socket::new(Domain::for_address(address), kind, Some(protocol))
        .map_err(BindError::Other)?;
    if address.is_ipv6() {
        socket.set_only_v6(true).map_err(BindError::Other)?;
    }
    #[cfg(unix)]
    if protocol == Protocol::TCP {
        socket.set_reuse_address(true).map_err(BindError::Other)?;
    }
    socket
        .bind(&address.into())
        .map_err(|error| classify(error, address))?;
    if protocol == Protocol::TCP {
        socket
            .listen(1024)
            .map_err(|error| classify(error, address))?;
    }
    Ok(socket)
}

fn classify(error: io::Error, address: SocketAddr) -> BindError {
    match error.kind() {
        io::ErrorKind::AddrInUse => BindError::PortUnavailable(format!("{address} is in use")),
        io::ErrorKind::PermissionDenied => BindError::PortUnavailable(format!(
            "listening on {address} needs privileges this process lacks"
        )),
        io::ErrorKind::AddrNotAvailable => BindError::AddressUnavailable(format!(
            "{} is not an address of this host",
            address.ip()
        )),
        _ => BindError::Other(error),
    }
}

/// A relay that accepts traffic until it is closed or dropped.
pub struct ActiveRelay {
    stop: watch::Sender<bool>,
    connections: Arc<AtomicUsize>,
}

impl ActiveRelay {
    /// The connections, flows, or associations it carries now.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Stops accepting and ends everything the relay carries.
    pub fn close(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for ActiveRelay {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone)]
struct RelayContext {
    bridge: BridgeHandle,
    target: RelayTarget,
    stopped: watch::Receiver<bool>,
    connections: Arc<AtomicUsize>,
}

impl RelayContext {
    /// Counts one connection for as long as the returned guard lives, unless the relay
    /// already carries `limit`.
    fn admit(&self, limit: usize) -> Option<ConnectionGuard> {
        let admitted = self
            .connections
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < limit).then_some(count + 1)
            })
            .is_ok();
        admitted.then(|| ConnectionGuard(self.connections.clone()))
    }

    async fn until_stopped(&self) {
        let mut stopped = self.stopped.clone();
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn accept_tcp(listener: TcpListener, context: RelayContext) {
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = context.until_stopped() => return,
        };
        let Ok((client, _)) = accepted else {
            continue;
        };
        let Some(guard) = context.admit(MAX_TCP_CONNECTIONS) else {
            continue;
        };
        let context = context.clone();
        tokio::spawn(async move {
            let _guard = guard;
            tokio::select! {
                _ = carry_tcp(client, &context) => {}
                _ = context.until_stopped() => {}
            }
        });
    }
}

async fn carry_tcp(mut client: TcpStream, context: &RelayContext) {
    let _ = client.set_nodelay(true);
    let Ok(mut stream) = context.bridge.open_stream(&context.target.channel()) else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut client, &mut stream).await;
}

struct Flow {
    datagrams: mpsc::Sender<Vec<u8>>,
    last_active: Arc<AtomicU64>,
    stop: watch::Sender<bool>,
    _guard: ConnectionGuard,
}

async fn relay_udp(socket: Arc<UdpSocket>, context: RelayContext) {
    let started = Instant::now();
    let mut flows: HashMap<SocketAddr, Flow> = HashMap::new();
    let mut buffer = vec![0u8; u16::MAX as usize];
    let mut sweep = tokio::time::interval(FLOW_SWEEP);
    loop {
        let received = tokio::select! {
            received = socket.recv_from(&mut buffer) => received,
            _ = sweep.tick() => {
                let now = started.elapsed().as_secs();
                flows.retain(|_, flow| {
                    let idle = now.saturating_sub(flow.last_active.load(Ordering::SeqCst));
                    let keep = idle < UDP_FLOW_IDLE.as_secs() && !flow.datagrams.is_closed();
                    if !keep {
                        let _ = flow.stop.send(true);
                    }
                    keep
                });
                continue;
            }
            _ = context.until_stopped() => return,
        };
        let Ok((length, client)) = received else {
            continue;
        };
        let datagram = buffer[..length].to_vec();
        if let Some(flow) = flows.get(&client) {
            if !flow.datagrams.is_closed() {
                flow.last_active
                    .store(started.elapsed().as_secs(), Ordering::SeqCst);
                // A full queue drops the datagram, as a congested path would.
                let _ = flow.datagrams.try_send(datagram);
                continue;
            }
            flows.remove(&client);
        }
        let Some(guard) = context.admit(MAX_UDP_FLOWS) else {
            continue;
        };
        let Some(flow) = open_flow(&socket, client, &context, started, guard) else {
            continue;
        };
        let _ = flow.datagrams.try_send(datagram);
        flows.insert(client, flow);
    }
}

/// Opens the bridge stream of one client's flow, with a task feeding it the client's
/// datagrams and a task returning replies to the client.
fn open_flow(
    socket: &Arc<UdpSocket>,
    client: SocketAddr,
    context: &RelayContext,
    started: Instant,
    guard: ConnectionGuard,
) -> Option<Flow> {
    let stream = context.bridge.open_stream(&context.target.channel()).ok()?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (datagrams, mut queued) = mpsc::channel::<Vec<u8>>(FLOW_QUEUE);
    let last_active = Arc::new(AtomicU64::new(started.elapsed().as_secs()));
    let (stop, stopped) = watch::channel(false);
    let flow_stopped = |mut stopped: watch::Receiver<bool>, relay: RelayContext| async move {
        tokio::select! {
            _ = stopped.wait_for(|stopped| *stopped) => {}
            _ = relay.until_stopped() => {}
        }
    };
    let sending = flow_stopped(stopped.clone(), context.clone());
    tokio::spawn(async move {
        tokio::select! {
            _ = async {
                while let Some(datagram) = queued.recv().await {
                    if writer.write_all(&Message::datagram(datagram).encode()).await.is_err() {
                        return;
                    }
                }
            } => {}
            _ = sending => {}
        }
        let _ = writer.shutdown().await;
    });
    let replies = socket.clone();
    let active = last_active.clone();
    let receiving = flow_stopped(stopped, context.clone());
    tokio::spawn(async move {
        let mut messages = MessageReader::default();
        let mut buffer = vec![0u8; 64 * 1024];
        tokio::select! {
            _ = async {
                loop {
                    let n = match reader.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    messages.push(&buffer[..n]);
                    while let Ok(Some(message)) = messages.next_message() {
                        active.store(started.elapsed().as_secs(), Ordering::SeqCst);
                        let _ = replies.send_to(&message.payload, client).await;
                    }
                }
            } => {}
            _ = receiving => {}
        }
    });
    Some(Flow {
        datagrams,
        last_active,
        stop,
        _guard: guard,
    })
}

/// Accepts SCTP associations on a blocking thread, each carried by two more, since the
/// kernel's SCTP sockets have no asynchronous interface here.
#[cfg(target_os = "linux")]
fn accept_sctp(
    listener: crate::sctp::Listener,
    context: RelayContext,
    runtime: tokio::runtime::Handle,
) {
    while !*context.stopped.borrow() {
        let Ok(Some(association)) = listener.accept() else {
            continue;
        };
        let Some(guard) = context.admit(MAX_SCTP_ASSOCIATIONS) else {
            continue;
        };
        let Ok(stream) = context.bridge.open_stream(&context.target.channel()) else {
            continue;
        };
        carry_sctp(Arc::new(association), stream, &context, &runtime, guard);
    }
}

#[cfg(target_os = "linux")]
fn carry_sctp(
    association: Arc<crate::sctp::Association>,
    stream: tokio::io::DuplexStream,
    context: &RelayContext,
    runtime: &tokio::runtime::Handle,
    guard: ConnectionGuard,
) {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let to_bridge = association.clone();
    let write_runtime = runtime.clone();
    let guard = Arc::new(guard);
    let sending_guard = guard.clone();
    std::thread::spawn(move || {
        let _guard = sending_guard;
        while let Ok(Some(message)) = to_bridge.receive() {
            if write_runtime
                .block_on(writer.write_all(&message.encode()))
                .is_err()
            {
                break;
            }
        }
        let _ = write_runtime.block_on(writer.shutdown());
    });
    let to_client = association.clone();
    let read_runtime = runtime.clone();
    std::thread::spawn(move || {
        let _guard = guard;
        let mut messages = MessageReader::default();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = match read_runtime.block_on(reader.read(&mut buffer)) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            messages.push(&buffer[..n]);
            while let Ok(Some(message)) = messages.next_message() {
                if to_client.send(&message).is_err() {
                    to_client.shutdown();
                    return;
                }
            }
        }
        to_client.shutdown_write();
    });
    let mut stopped = context.stopped.clone();
    runtime.spawn(async move {
        let _ = stopped.wait_for(|stopped| *stopped).await;
        association.shutdown();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{serve, ConnectFuture, Connector, Introduction};
    use crate::protocol::{Frame, Peer};
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::io::{duplex, DuplexStream};
    use tokio::sync::oneshot;

    struct NoChannels;

    impl Connector for NoChannels {
        fn connect<'a>(&'a self, _channel: &'a str, _peer: Option<&'a Peer>) -> ConnectFuture<'a> {
            Box::pin(async { Err(io::Error::new(io::ErrorKind::NotFound, "none")) })
        }
    }

    /// A bridge whose container end echoes every stream the host opens: bytes back as
    /// they came for TCP, and each message back whole for UDP.
    async fn echoing_bridge() -> BridgeHandle {
        let (mut to_host, host_reader) = duplex(1 << 20);
        let (host_writer, mut from_host): (DuplexStream, DuplexStream) = duplex(1 << 20);
        let (introduced, introduction) = oneshot::channel::<Introduction>();
        tokio::spawn(serve(
            host_reader,
            host_writer,
            Arc::new(NoChannels),
            introduced,
        ));
        to_host.write_all(&Frame::hello().encode()).await.unwrap();
        tokio::spawn(async move {
            while let Ok(Some(frame)) = crate::host::read_frame(&mut from_host).await {
                let reply = match frame {
                    Frame::Data { stream, bytes } => vec![
                        Frame::Credit {
                            stream,
                            bytes: bytes.len() as u32,
                        },
                        Frame::Data { stream, bytes },
                    ],
                    Frame::Shutdown { stream } => vec![Frame::Shutdown { stream }],
                    _ => Vec::new(),
                };
                for frame in reply {
                    if to_host.write_all(&frame.encode()).await.is_err() {
                        return;
                    }
                }
            }
        });
        introduction.await.unwrap().handle
    }

    fn spec(protocol: RelayProtocol, addresses: Vec<IpAddr>) -> RelaySpec {
        RelaySpec {
            protocol,
            host_port: 0,
            guest_port: 8080,
            addresses,
        }
    }

    #[tokio::test]
    async fn a_tcp_relay_carries_connections_through_the_bridge() {
        let bridge = echoing_bridge().await;
        let bound =
            BoundRelay::bind(spec(RelayProtocol::Tcp, vec![Ipv4Addr::LOCALHOST.into()])).unwrap();
        let address = bound.local_addresses()[0];
        let relay = bound.activate(bridge).unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut reply = [0u8; 4];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"ping");
        assert_eq!(relay.connections(), 1);
        relay.close();
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest)).await;
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn a_udp_relay_returns_replies_to_the_client_that_sent_them() {
        let bridge = echoing_bridge().await;
        let bound =
            BoundRelay::bind(spec(RelayProtocol::Udp, vec![Ipv4Addr::LOCALHOST.into()])).unwrap();
        let address = bound.local_addresses()[0];
        let relay = bound.activate(bridge).unwrap();
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        client.send_to(b"query", address).await.unwrap();
        let mut reply = [0u8; 16];
        let (n, from) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut reply))
            .await
            .expect("a reply")
            .unwrap();
        assert_eq!(&reply[..n], b"query");
        assert_eq!(from, address);
        assert_eq!(relay.connections(), 1);
    }

    #[test]
    fn a_busy_port_is_reported_before_anything_listens() {
        let first =
            BoundRelay::bind(spec(RelayProtocol::Tcp, vec![Ipv4Addr::LOCALHOST.into()])).unwrap();
        let port = first.local_addresses()[0].port();
        let busy = RelaySpec {
            host_port: port,
            ..spec(RelayProtocol::Tcp, vec![Ipv4Addr::LOCALHOST.into()])
        };
        assert!(matches!(
            BoundRelay::bind(busy.clone()),
            Err(BindError::PortUnavailable(_))
        ));
        assert!(matches!(
            check_available(&busy),
            Err(BindError::PortUnavailable(_))
        ));
        drop(first);
        assert!(check_available(&busy).is_ok());
    }

    #[test]
    fn a_udp_port_cannot_be_bound_twice() {
        let first =
            BoundRelay::bind(spec(RelayProtocol::Udp, vec![Ipv4Addr::LOCALHOST.into()])).unwrap();
        let port = first.local_addresses()[0].port();
        let again = RelaySpec {
            host_port: port,
            ..spec(RelayProtocol::Udp, vec![Ipv4Addr::LOCALHOST.into()])
        };
        assert!(matches!(
            BoundRelay::bind(again),
            Err(BindError::PortUnavailable(_))
        ));
    }

    #[test]
    fn an_address_the_host_lacks_is_reported() {
        let foreign = spec(RelayProtocol::Tcp, vec!["192.0.2.1".parse().unwrap()]);
        assert!(matches!(
            BoundRelay::bind(foreign),
            Err(BindError::AddressUnavailable(_))
        ));
    }

    #[test]
    fn ipv6_wildcards_leave_ipv4_alone() {
        let ipv4 =
            BoundRelay::bind(spec(RelayProtocol::Tcp, vec![Ipv4Addr::UNSPECIFIED.into()])).unwrap();
        let port = ipv4.local_addresses()[0].port();
        let ipv6 = RelaySpec {
            host_port: port,
            ..spec(RelayProtocol::Tcp, vec![Ipv6Addr::UNSPECIFIED.into()])
        };
        match BoundRelay::bind(ipv6) {
            Ok(_) => {}
            // A host without IPv6 cannot bind `::` at all, which says nothing about sharing.
            Err(BindError::AddressUnavailable(_)) | Err(BindError::Other(_)) => {}
            Err(other) => panic!("`::` collided with 0.0.0.0: {other}"),
        }
    }
}
