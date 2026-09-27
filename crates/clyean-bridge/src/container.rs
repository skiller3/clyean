// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The container end of the bridge.  It listens on one Unix socket per channel and turns
//! each accepted connection into a stream to the host, and it connects each stream the
//! host opens on a `relay:<protocol>:<port>` channel to that port on the container's
//! loopback.  Two threads serve every stream: one reads the connection while it holds
//! credit, the other writes what the host sends, so a slow connection never stalls the
//! others.  When its input ends, which happens as soon as the host process dies, the
//! bridge returns and its process exits, closing every connection it served.

use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, UdpSocket};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use crate::protocol::{
    is_bridge_stream, Frame, Message, MessageReader, Peer, RelayProtocol, RelayTarget, MAX_DATA,
    MAX_HOST_STREAMS, MAX_STREAMS, STREAM_WINDOW,
};

#[cfg(target_os = "linux")]
use crate::sctp;

/// How long connecting to a relay's port on the container's loopback may take.
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How often a thread waiting on a datagram socket checks whether its stream is gone.
const DATAGRAM_POLL: Duration = Duration::from_millis(500);

/// A named channel and the socket that serves it inside the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub socket_path: PathBuf,
}

impl Channel {
    /// Parses `NAME=PATH`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        match spec.split_once('=') {
            Some((name, path)) if !name.is_empty() && path.starts_with('/') => Ok(Self {
                name: name.to_string(),
                socket_path: PathBuf::from(path),
            }),
            _ => Err(format!(
                "a channel is NAME=/absolute/socket/path, not {spec:?}"
            )),
        }
    }
}

/// Serves `channels` over `input` and `output` until `input` ends, creating `ready_file`
/// once every socket accepts connections.
pub fn run(
    channels: &[Channel],
    ready_file: &Path,
    input: impl Read,
    output: impl Write + Send + 'static,
) -> io::Result<()> {
    let bridge = Bridge::new(output);
    bridge.send(&Frame::hello())?;
    for channel in channels {
        let listener = bind(&channel.socket_path)?;
        bridge.serve(listener, channel.name.clone());
    }
    mark_ready(ready_file)?;
    bridge.pump(input)
}

/// Binds a socket that only the container's own user can reach.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn mark_ready(ready_file: &Path) -> io::Result<()> {
    if let Some(parent) = ready_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = ready_file.with_extension("partial");
    std::fs::write(&partial, b"ready\n")?;
    std::fs::rename(&partial, ready_file)
}

/// Copies standard input to a Unix socket and the socket to standard output, like
/// `socat - UNIX-CONNECT:<path>`.  It returns when the socket's other end closes.
pub fn connect(path: &Path) -> io::Result<()> {
    let socket = UnixStream::connect(path)?;
    let mut to_socket = socket.try_clone()?;
    std::thread::spawn(move || {
        let _ = relay(&mut io::stdin().lock(), &mut to_socket);
        let _ = to_socket.shutdown(Shutdown::Write);
    });
    let mut from_socket = &socket;
    relay(&mut from_socket, &mut io::stdout().lock())
}

/// Copies until the end of `input`, flushing every chunk so that nothing waits in a buffer
/// (standard output would otherwise hold whole lines, or more, until it fills).
fn relay(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    let mut buffer = [0u8; 8192];
    loop {
        let n = match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        output.write_all(&buffer[..n])?;
        output.flush()?;
    }
}

/// The shared state of one bridge session.
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    output: Mutex<Box<dyn Write + Send>>,
    streams: Mutex<HashMap<u32, Arc<Stream>>>,
    next_stream: AtomicU32,
}

struct Stream {
    inbound: Mutex<Option<mpsc::Sender<Inbound>>>,
    credit: Credit,
    /// The connection the stream carries, once it is connected.
    endpoint: Mutex<Option<Endpoint>>,
    open_directions: AtomicU8,
    host_opened: bool,
}

impl Stream {
    fn new(inbound: mpsc::Sender<Inbound>, endpoint: Option<Endpoint>, host_opened: bool) -> Self {
        Self {
            inbound: Mutex::new(Some(inbound)),
            credit: Credit::new(STREAM_WINDOW as usize),
            endpoint: Mutex::new(endpoint),
            open_directions: AtomicU8::new(2),
            host_opened,
        }
    }

    fn set_endpoint(&self, endpoint: Endpoint) {
        *self.endpoint.lock().expect("stream endpoint lock") = Some(endpoint);
    }
}

/// What one stream is connected to inside the container.
enum Endpoint {
    Unix(UnixStream),
    Tcp(TcpStream),
    Udp,
    #[cfg(target_os = "linux")]
    Sctp(Arc<sctp::Association>),
}

impl Endpoint {
    /// Unblocks the stream's threads.  A datagram socket has nothing to shut down: its
    /// reader notices the stream is gone within [`DATAGRAM_POLL`].
    fn shutdown(&self) {
        match self {
            Self::Unix(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
            Self::Tcp(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
            Self::Udp => {}
            #[cfg(target_os = "linux")]
            Self::Sctp(association) => association.shutdown(),
        }
    }
}

enum Inbound {
    Data(Vec<u8>),
    Shutdown,
}

/// A connection whose writing half can be closed on its own.
trait HalfClose {
    fn close_write(&self);
}

impl HalfClose for UnixStream {
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
}

impl HalfClose for TcpStream {
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
}

impl Bridge {
    pub fn new(output: impl Write + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                output: Mutex::new(Box::new(output)),
                streams: Mutex::new(HashMap::new()),
                next_stream: AtomicU32::new(1),
            }),
        }
    }

    /// Writes one frame to the host.
    pub fn send(&self, frame: &Frame) -> io::Result<()> {
        let mut output = self.inner.output.lock().expect("bridge output lock");
        output.write_all(&frame.encode())?;
        output.flush()
    }

    /// Accepts connections on `listener` in the background, each becoming a stream on
    /// `channel`.
    pub fn serve(&self, listener: UnixListener, channel: String) {
        let bridge = self.clone();
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                bridge.open(&channel, socket);
            }
        });
    }

    /// Reads frames from the host until its input ends.
    pub fn pump(&self, input: impl Read) -> io::Result<()> {
        let mut input = BufReader::new(input);
        while let Some(frame) = Frame::read_from(&mut input)? {
            match frame {
                Frame::Data { stream, bytes } => self.deliver(stream, Inbound::Data(bytes)),
                Frame::Shutdown { stream } => self.deliver(stream, Inbound::Shutdown),
                Frame::Credit { stream, bytes } => {
                    if let Some(entry) = self.stream(stream) {
                        entry.credit.add(bytes as usize);
                    }
                }
                Frame::Close { stream } => self.abort(stream, false),
                Frame::Open {
                    stream, channel, ..
                } if !is_bridge_stream(stream) => self.open_relay(stream, &channel),
                Frame::Open { stream, .. } => self.send(&Frame::Close { stream })?,
                Frame::Hello { .. } => {}
            }
        }
        Ok(())
    }

    /// Registers a stream, allocating the next bridge id for a bridge-opened one, and
    /// refuses it when the bridge already holds as many streams as it may.
    fn register(&self, entry: Arc<Stream>, host_id: Option<u32>) -> Option<u32> {
        let mut streams = self.inner.streams.lock().expect("bridge streams lock");
        if streams.len() >= MAX_STREAMS {
            return None;
        }
        let id = match host_id {
            Some(id) => {
                let host_streams = streams.values().filter(|s| s.host_opened).count();
                if host_streams >= MAX_HOST_STREAMS || streams.contains_key(&id) {
                    return None;
                }
                id
            }
            None => self.inner.next_stream.fetch_add(2, Ordering::SeqCst),
        };
        streams.insert(id, entry);
        Some(id)
    }

    fn open(&self, channel: &str, socket: UnixStream) {
        let peer = peer_of(&socket);
        let (Ok(reader), Ok(shutdown_handle)) = (socket.try_clone(), socket.try_clone()) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let entry = Arc::new(Stream::new(
            sender,
            Some(Endpoint::Unix(shutdown_handle)),
            false,
        ));
        let Some(id) = self.register(entry.clone(), None) else {
            return;
        };
        let opened = Frame::Open {
            stream: id,
            channel: channel.to_string(),
            peer,
        };
        if self.send(&opened).is_err() {
            self.abort(id, false);
            return;
        }
        let bridge = self.clone();
        std::thread::spawn(move || bridge.forward_bytes_to_host(id, reader, &entry));
        let bridge = self.clone();
        std::thread::spawn(move || bridge.forward_bytes_to_endpoint(id, socket, receiver));
    }

    /// Accepts a stream the host opens on a relay channel.  The stream is registered at
    /// once, so that data the host sends before the connection completes waits for it.
    fn open_relay(&self, id: u32, channel: &str) {
        let Some(target) = RelayTarget::from_channel(channel) else {
            let _ = self.send(&Frame::Close { stream: id });
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let entry = Arc::new(Stream::new(sender, None, true));
        if self.register(entry.clone(), Some(id)).is_none() {
            let _ = self.send(&Frame::Close { stream: id });
            return;
        }
        let bridge = self.clone();
        std::thread::spawn(move || bridge.connect_relay(id, target, entry, receiver));
    }

    fn connect_relay(
        &self,
        id: u32,
        target: RelayTarget,
        entry: Arc<Stream>,
        receiver: mpsc::Receiver<Inbound>,
    ) {
        match target.protocol {
            RelayProtocol::Tcp => {
                let Some((reader, writer, handle)) = connect_tcp(target.port) else {
                    self.abort(id, true);
                    return;
                };
                entry.set_endpoint(Endpoint::Tcp(handle));
                let bridge = self.clone();
                std::thread::spawn(move || bridge.forward_bytes_to_host(id, reader, &entry));
                self.forward_bytes_to_endpoint(id, writer, receiver);
            }
            RelayProtocol::Udp => {
                let Ok(socket) = connect_udp(target.port) else {
                    self.abort(id, true);
                    return;
                };
                let socket = Arc::new(socket);
                entry.set_endpoint(Endpoint::Udp);
                let bridge = self.clone();
                let reader = socket.clone();
                std::thread::spawn(move || bridge.forward_datagrams_to_host(id, &reader, &entry));
                self.forward_datagrams_to_endpoint(id, &socket, receiver);
            }
            RelayProtocol::Sctp => self.connect_sctp(id, target.port, entry, receiver),
        }
    }

    #[cfg(target_os = "linux")]
    fn connect_sctp(
        &self,
        id: u32,
        port: u16,
        entry: Arc<Stream>,
        receiver: mpsc::Receiver<Inbound>,
    ) {
        let Some(association) = sctp::Association::connect_loopback(port, RELAY_CONNECT_TIMEOUT)
        else {
            self.abort(id, true);
            return;
        };
        let association = Arc::new(association);
        entry.set_endpoint(Endpoint::Sctp(association.clone()));
        let bridge = self.clone();
        let reader = association.clone();
        std::thread::spawn(move || bridge.forward_sctp_to_host(id, &reader, &entry));
        self.forward_sctp_to_endpoint(id, &association, receiver);
    }

    /// SCTP relays exist only where the container's kernel provides SCTP.
    #[cfg(not(target_os = "linux"))]
    fn connect_sctp(
        &self,
        id: u32,
        _port: u16,
        _entry: Arc<Stream>,
        _receiver: mpsc::Receiver<Inbound>,
    ) {
        self.abort(id, true);
    }

    fn forward_bytes_to_host(&self, id: u32, mut reader: impl Read, entry: &Stream) {
        let mut buffer = vec![0u8; MAX_DATA];
        while let Some(allowance) = entry.credit.wait_available(MAX_DATA) {
            match reader.read(&mut buffer[..allowance]) {
                Ok(0) => {
                    if self.stream(id).is_some() {
                        let _ = self.send(&Frame::Shutdown { stream: id });
                        self.finish_direction(id);
                    }
                    return;
                }
                Ok(n) => {
                    entry.credit.consume(n);
                    let data = Frame::Data {
                        stream: id,
                        bytes: buffer[..n].to_vec(),
                    };
                    if self.send(&data).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.abort(id, true);
                    return;
                }
            }
        }
    }

    fn forward_bytes_to_endpoint(
        &self,
        id: u32,
        mut writer: impl Write + HalfClose,
        inbound: mpsc::Receiver<Inbound>,
    ) {
        for message in inbound {
            match message {
                Inbound::Data(bytes) => {
                    if writer.write_all(&bytes).is_err() {
                        self.abort(id, true);
                        return;
                    }
                    if self.return_credit(id, bytes.len()).is_err() {
                        return;
                    }
                }
                Inbound::Shutdown => {
                    writer.close_write();
                    self.finish_direction(id);
                    return;
                }
            }
        }
    }

    /// Forwards datagrams while the stream holds credit for them and drops the rest, as a
    /// congested path would, so a stalled host never makes the bridge buffer.
    fn forward_datagrams_to_host(&self, id: u32, socket: &UdpSocket, entry: &Stream) {
        let mut buffer = vec![0u8; u16::MAX as usize];
        while self.stream(id).is_some() {
            match socket.recv(&mut buffer) {
                Ok(n) => {
                    let encoded = Message::datagram(buffer[..n].to_vec()).encode();
                    if !entry.credit.try_consume(encoded.len()) {
                        continue;
                    }
                    if self.send_message_bytes(id, &encoded).is_err() {
                        return;
                    }
                }
                Err(error) if is_transient_datagram_error(&error) => continue,
                Err(_) => {
                    self.abort(id, true);
                    return;
                }
            }
        }
    }

    fn forward_datagrams_to_endpoint(
        &self,
        id: u32,
        socket: &UdpSocket,
        inbound: mpsc::Receiver<Inbound>,
    ) {
        let mut reader = MessageReader::default();
        for message in inbound {
            match message {
                Inbound::Data(bytes) => {
                    reader.push(&bytes);
                    loop {
                        match reader.next_message() {
                            // A datagram the socket refuses is lost, as UDP allows.
                            Ok(Some(message)) => {
                                let _ = socket.send(&message.payload);
                            }
                            Ok(None) => break,
                            Err(_) => {
                                self.abort(id, true);
                                return;
                            }
                        }
                    }
                    if self.return_credit(id, bytes.len()).is_err() {
                        return;
                    }
                }
                Inbound::Shutdown => {
                    self.finish_direction(id);
                    return;
                }
            }
        }
    }

    /// Forwards SCTP messages, waiting for credit rather than dropping, since SCTP is
    /// reliable.
    #[cfg(target_os = "linux")]
    fn forward_sctp_to_host(&self, id: u32, association: &sctp::Association, entry: &Stream) {
        loop {
            match association.receive() {
                Ok(Some(message)) => {
                    let encoded = message.encode();
                    let mut sent = 0;
                    while sent < encoded.len() {
                        let Some(allowance) = entry.credit.wait_available(MAX_DATA) else {
                            return;
                        };
                        let end = (sent + allowance).min(encoded.len());
                        entry.credit.consume(end - sent);
                        let data = Frame::Data {
                            stream: id,
                            bytes: encoded[sent..end].to_vec(),
                        };
                        if self.send(&data).is_err() {
                            return;
                        }
                        sent = end;
                    }
                }
                Ok(None) => {
                    if self.stream(id).is_some() {
                        let _ = self.send(&Frame::Shutdown { stream: id });
                        self.finish_direction(id);
                    }
                    return;
                }
                Err(_) => {
                    self.abort(id, true);
                    return;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn forward_sctp_to_endpoint(
        &self,
        id: u32,
        association: &sctp::Association,
        inbound: mpsc::Receiver<Inbound>,
    ) {
        let mut reader = MessageReader::default();
        for message in inbound {
            match message {
                Inbound::Data(bytes) => {
                    reader.push(&bytes);
                    loop {
                        match reader.next_message() {
                            Ok(Some(message)) => {
                                if association.send(&message).is_err() {
                                    self.abort(id, true);
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => {
                                self.abort(id, true);
                                return;
                            }
                        }
                    }
                    if self.return_credit(id, bytes.len()).is_err() {
                        return;
                    }
                }
                Inbound::Shutdown => {
                    association.shutdown_write();
                    self.finish_direction(id);
                    return;
                }
            }
        }
    }

    /// Sends an encoded message in as many data frames as it needs.
    fn send_message_bytes(&self, id: u32, encoded: &[u8]) -> io::Result<()> {
        for chunk in encoded.chunks(MAX_DATA) {
            self.send(&Frame::Data {
                stream: id,
                bytes: chunk.to_vec(),
            })?;
        }
        Ok(())
    }

    fn return_credit(&self, id: u32, bytes: usize) -> io::Result<()> {
        self.send(&Frame::Credit {
            stream: id,
            bytes: bytes as u32,
        })
    }

    fn stream(&self, id: u32) -> Option<Arc<Stream>> {
        self.inner
            .streams
            .lock()
            .expect("bridge streams lock")
            .get(&id)
            .cloned()
    }

    fn deliver(&self, id: u32, message: Inbound) {
        if let Some(entry) = self.stream(id) {
            if let Some(sender) = entry.inbound.lock().expect("stream inbound lock").as_ref() {
                let _ = sender.send(message);
            }
        }
    }

    /// Records that one direction of a stream ended; the stream is forgotten once both have.
    fn finish_direction(&self, id: u32) {
        let Some(entry) = self.stream(id) else {
            return;
        };
        if entry.open_directions.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner
                .streams
                .lock()
                .expect("bridge streams lock")
                .remove(&id);
            entry.credit.close();
        }
    }

    /// Drops a stream at once, telling the host when the bridge is the side giving up.
    fn abort(&self, id: u32, notify_host: bool) {
        let removed = self
            .inner
            .streams
            .lock()
            .expect("bridge streams lock")
            .remove(&id);
        let Some(entry) = removed else {
            return;
        };
        entry.credit.close();
        entry.inbound.lock().expect("stream inbound lock").take();
        if let Some(endpoint) = entry
            .endpoint
            .lock()
            .expect("stream endpoint lock")
            .as_ref()
        {
            endpoint.shutdown();
        }
        if notify_host {
            let _ = self.send(&Frame::Close { stream: id });
        }
    }
}

/// Connects to `port` on the container's loopback, IPv4 first, returning a reader, a
/// writer, and a handle that shuts the connection down.
fn connect_tcp(port: u16) -> Option<(TcpStream, TcpStream, TcpStream)> {
    let addresses = [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ];
    let stream = addresses
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, RELAY_CONNECT_TIMEOUT).ok())?;
    let _ = stream.set_nodelay(true);
    let reader = stream.try_clone().ok()?;
    let handle = stream.try_clone().ok()?;
    Some((reader, stream, handle))
}

/// A UDP socket connected to `port` on the container's loopback: IPv4 unless only an
/// IPv6 socket is bound to the port.  UDP reports nothing at connect time, so the choice
/// comes from the container's socket tables.
fn connect_udp(port: u16) -> io::Result<UdpSocket> {
    let ipv4 = std::fs::read_to_string("/proc/net/udp").unwrap_or_default();
    let ipv6 = std::fs::read_to_string("/proc/net/udp6").unwrap_or_default();
    let use_ipv6 = !table_binds_port(&ipv4, port) && table_binds_port(&ipv6, port);
    let (local, target): (SocketAddr, SocketAddr) = if use_ipv6 {
        (
            (Ipv6Addr::LOCALHOST, 0).into(),
            (Ipv6Addr::LOCALHOST, port).into(),
        )
    } else {
        (
            (Ipv4Addr::LOCALHOST, 0).into(),
            (Ipv4Addr::LOCALHOST, port).into(),
        )
    };
    let socket = UdpSocket::bind(local)?;
    socket.connect(target)?;
    socket.set_read_timeout(Some(DATAGRAM_POLL))?;
    Ok(socket)
}

/// Whether a `/proc/net/udp` or `udp6` table lists a socket bound to `port`.
fn table_binds_port(table: &str, port: u16) -> bool {
    table.lines().skip(1).any(|line| {
        line.split_whitespace()
            .nth(1)
            .and_then(|local| local.rsplit_once(':'))
            .and_then(|(_, hex_port)| u16::from_str_radix(hex_port, 16).ok())
            == Some(port)
    })
}

fn is_transient_datagram_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::Interrupted
            // An earlier datagram met a closed port; the flow itself stays usable.
            | io::ErrorKind::ConnectionRefused
    )
}

/// The process that connected to one of the bridge's sockets, read with `SO_PEERCRED`.
#[cfg(target_os = "linux")]
fn peer_of(socket: &UnixStream) -> Option<Peer> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: getsockopt writes at most `length` bytes into the zeroed ucred it is given.
    let credentials = unsafe {
        let mut credentials: libc::ucred = std::mem::zeroed();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        );
        (result == 0).then_some(credentials)
    }?;
    let pid = u32::try_from(credentials.pid).ok().filter(|pid| *pid > 0)?;
    let command = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    Some(Peer::new(pid, &command))
}

#[cfg(not(target_os = "linux"))]
fn peer_of(_socket: &UnixStream) -> Option<Peer> {
    None
}

/// How many bytes one side of a stream may still send before the receiver returns credit.
struct Credit {
    state: Mutex<CreditState>,
    changed: Condvar,
}

struct CreditState {
    available: usize,
    closed: bool,
}

impl Credit {
    fn new(initial: usize) -> Self {
        Self {
            state: Mutex::new(CreditState {
                available: initial,
                closed: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// Waits until some credit is available and returns how much may be used, up to
    /// `max`, or `None` once the stream is gone.
    fn wait_available(&self, max: usize) -> Option<usize> {
        let mut state = self.state.lock().expect("credit lock");
        while state.available == 0 && !state.closed {
            state = self.changed.wait(state).expect("credit lock");
        }
        if state.closed {
            None
        } else {
            Some(state.available.min(max))
        }
    }

    fn consume(&self, used: usize) {
        let mut state = self.state.lock().expect("credit lock");
        state.available = state.available.saturating_sub(used);
    }

    /// Takes `wanted` bytes of credit when that much is available, without waiting.
    fn try_consume(&self, wanted: usize) -> bool {
        let mut state = self.state.lock().expect("credit lock");
        if state.closed || state.available < wanted {
            return false;
        }
        state.available -= wanted;
        true
    }

    fn add(&self, granted: usize) {
        let mut state = self.state.lock().expect("credit lock");
        state.available = state.available.saturating_add(granted);
        self.changed.notify_all();
    }

    fn close(&self) {
        self.state.lock().expect("credit lock").closed = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    struct Harness {
        host: UnixStream,
        socket_path: PathBuf,
        ready_file: PathBuf,
        bridge: std::thread::JoinHandle<io::Result<()>>,
        _dir: tempfile::TempDir,
    }

    fn start() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("orchestrator.sock");
        let ready_file = dir.path().join("bridge.ready");
        let (host, bridge_end) = UnixStream::pair().unwrap();
        let input = bridge_end.try_clone().unwrap();
        let channels = vec![Channel {
            name: "orchestrator".into(),
            socket_path: socket_path.clone(),
        }];
        let ready = ready_file.clone();
        let bridge = std::thread::spawn(move || run(&channels, &ready, input, bridge_end));
        let mut harness = Harness {
            host,
            socket_path,
            ready_file,
            bridge,
            _dir: dir,
        };
        assert_eq!(harness.next_frame(), Frame::hello());
        for _ in 0..200 {
            if harness.ready_file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(harness.ready_file.exists(), "the bridge never became ready");
        harness
    }

    impl Harness {
        fn next_frame(&mut self) -> Frame {
            Frame::read_from(&mut self.host).unwrap().expect("a frame")
        }

        fn send(&mut self, frame: Frame) {
            self.host.write_all(&frame.encode()).unwrap();
        }

        fn connect(&mut self) -> (UnixStream, u32) {
            let client = UnixStream::connect(&self.socket_path).unwrap();
            match self.next_frame() {
                Frame::Open {
                    stream, channel, ..
                } => {
                    assert_eq!(channel, "orchestrator");
                    (client, stream)
                }
                other => panic!("expected an open frame, got {other:?}"),
            }
        }

        fn open_relay(&mut self, stream: u32, channel: &str) {
            self.send(Frame::Open {
                stream,
                channel: channel.into(),
                peer: None,
            });
        }
    }

    #[test]
    fn channel_specs_need_a_name_and_an_absolute_path() {
        assert_eq!(
            Channel::parse("herdr=/run/herdr/herdr.sock").unwrap(),
            Channel {
                name: "herdr".into(),
                socket_path: PathBuf::from("/run/herdr/herdr.sock")
            }
        );
        assert!(Channel::parse("herdr").is_err());
        assert!(Channel::parse("=/x").is_err());
        assert!(Channel::parse("herdr=relative").is_err());
    }

    #[test]
    fn bytes_flow_both_ways_and_delivered_data_returns_credit() {
        let mut harness = start();
        let (mut client, stream) = harness.connect();
        assert_eq!(stream % 2, 1);
        client.write_all(b"ping\n").unwrap();
        assert_eq!(
            harness.next_frame(),
            Frame::Data {
                stream,
                bytes: b"ping\n".to_vec()
            }
        );
        harness.send(Frame::Data {
            stream,
            bytes: b"pong\n".to_vec(),
        });
        let mut reply = [0u8; 5];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong\n");
        assert_eq!(harness.next_frame(), Frame::Credit { stream, bytes: 5 });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_bridge_opened_stream_names_the_process_that_connected() {
        let mut harness = start();
        let _client = UnixStream::connect(&harness.socket_path).unwrap();
        match harness.next_frame() {
            Frame::Open {
                peer: Some(peer), ..
            } => {
                assert_eq!(peer.pid, std::process::id());
                assert!(!peer.command.is_empty());
            }
            other => panic!("expected an open frame naming its peer, got {other:?}"),
        }
    }

    #[test]
    fn half_closes_propagate_in_each_direction() {
        let mut harness = start();
        let (mut client, stream) = harness.connect();
        client.shutdown(Shutdown::Write).unwrap();
        assert_eq!(harness.next_frame(), Frame::Shutdown { stream });
        harness.send(Frame::Shutdown { stream });
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn a_close_from_the_host_drops_the_connection() {
        let mut harness = start();
        let (mut client, stream) = harness.connect();
        harness.send(Frame::Close { stream });
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn a_stream_sends_no_more_than_its_window_until_credit_returns() {
        let mut harness = start();
        let (client, stream) = harness.connect();
        let total = STREAM_WINDOW as usize + 64 * 1024;
        let writer = std::thread::spawn(move || {
            let mut client = client;
            client.write_all(&vec![7u8; total]).unwrap();
            client
        });
        let mut received = 0usize;
        while received < STREAM_WINDOW as usize {
            match harness.next_frame() {
                Frame::Data { bytes, .. } => received += bytes.len(),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(received, STREAM_WINDOW as usize);
        harness
            .host
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut probe = [0u8; 1];
        assert!(
            harness.host.read(&mut probe).is_err(),
            "data beyond the window arrived"
        );
        harness.host.set_read_timeout(None).unwrap();
        harness.send(Frame::Credit {
            stream,
            bytes: STREAM_WINDOW,
        });
        while received < total {
            match harness.next_frame() {
                Frame::Data { bytes, .. } => received += bytes.len(),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(received, total);
        drop(writer.join().unwrap());
    }

    #[test]
    fn a_host_opened_tcp_stream_reaches_a_loopback_server() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = server.local_addr().unwrap().port();
        let echo = std::thread::spawn(move || {
            let (mut connection, _) = server.accept().unwrap();
            let mut request = [0u8; 4];
            connection.read_exact(&mut request).unwrap();
            connection.write_all(b"pong").unwrap();
            request
        });
        let mut harness = start();
        harness.open_relay(2, &format!("relay:tcp:{port}"));
        harness.send(Frame::Data {
            stream: 2,
            bytes: b"ping".to_vec(),
        });
        let mut reply = Vec::new();
        let mut credit = 0;
        while reply.len() < 4 || credit < 4 {
            match harness.next_frame() {
                Frame::Data { stream: 2, bytes } => reply.extend(bytes),
                Frame::Credit { stream: 2, bytes } => credit += bytes,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(reply, b"pong");
        assert_eq!(&echo.join().unwrap(), b"ping");
    }

    #[test]
    fn a_host_opened_udp_stream_carries_datagrams_as_messages() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = server.local_addr().unwrap().port();
        let echo = std::thread::spawn(move || {
            let mut buffer = [0u8; 64];
            let (n, from) = server.recv_from(&mut buffer).unwrap();
            server.send_to(&buffer[..n], from).unwrap();
            buffer[..n].to_vec()
        });
        let mut harness = start();
        harness.open_relay(4, &format!("relay:udp:{port}"));
        harness.send(Frame::Data {
            stream: 4,
            bytes: Message::datagram(b"query".to_vec()).encode(),
        });
        let mut reader = MessageReader::default();
        let reply = loop {
            match harness.next_frame() {
                Frame::Data { stream: 4, bytes } => {
                    reader.push(&bytes);
                    if let Some(message) = reader.next_message().unwrap() {
                        break message;
                    }
                }
                Frame::Credit { stream: 4, .. } => {}
                other => panic!("unexpected {other:?}"),
            }
        };
        assert_eq!(reply.payload, b"query");
        assert_eq!(echo.join().unwrap(), b"query");
    }

    #[test]
    fn relay_streams_to_unknown_channels_or_closed_ports_are_closed() {
        let unused = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let closed_port = unused.local_addr().unwrap().port();
        drop(unused);
        let mut harness = start();
        harness.open_relay(2, "relay:54549");
        assert_eq!(harness.next_frame(), Frame::Close { stream: 2 });
        harness.open_relay(4, "orchestrator");
        assert_eq!(harness.next_frame(), Frame::Close { stream: 4 });
        harness.open_relay(6, &format!("relay:tcp:{closed_port}"));
        assert_eq!(harness.next_frame(), Frame::Close { stream: 6 });
    }

    #[test]
    fn host_streams_numbered_like_bridge_streams_are_refused() {
        let mut harness = start();
        harness.open_relay(3, "relay:tcp:80");
        assert_eq!(harness.next_frame(), Frame::Close { stream: 3 });
    }

    #[test]
    fn the_host_may_not_hold_more_streams_than_its_share() {
        let bridge = Bridge::new(io::sink());
        for index in 0..MAX_HOST_STREAMS {
            let (sender, _receiver) = mpsc::channel();
            let entry = Arc::new(Stream::new(sender, None, true));
            assert!(bridge
                .register(entry, Some(2 * (index as u32 + 1)))
                .is_some());
        }
        let (sender, _receiver) = mpsc::channel();
        let over = Arc::new(Stream::new(sender, None, true));
        assert!(bridge.register(over, Some(100_000)).is_none());
        let (sender, _receiver) = mpsc::channel();
        let bridge_stream = Arc::new(Stream::new(sender, None, false));
        assert!(
            bridge.register(bridge_stream, None).is_some(),
            "the bridge's own channels keep their share"
        );
    }

    #[test]
    fn udp_tables_are_read_for_the_bound_port() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n  482: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 3131 2 0000000000000000 0\n";
        assert!(table_binds_port(table, 5353));
        assert!(!table_binds_port(table, 53));
        assert!(!table_binds_port("", 5353));
    }

    #[test]
    fn the_bridge_returns_when_its_input_ends_and_connections_close() {
        let mut harness = start();
        let (mut client, _stream) = harness.connect();
        harness.host.shutdown(Shutdown::Write).unwrap();
        harness.bridge.join().unwrap().unwrap();
        // In production the process exits here; the test drops the listener's streams.
        drop(harness.host);
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let _ = client.read(&mut [0u8; 1]);
    }
}
