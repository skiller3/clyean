// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The container end of the bridge.  It listens on one Unix socket per channel and turns
//! each accepted connection into a stream to the host.  Two threads serve every stream:
//! one reads the connection while it holds credit, the other writes what the host sends,
//! so a slow connection never stalls the others.  When its input ends, which happens as
//! soon as the host process dies, the bridge returns and its process exits, closing every
//! connection it served.

use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};

use crate::protocol::{Frame, MAX_DATA, MAX_STREAMS, STREAM_WINDOW};

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
        let _ = io::copy(&mut io::stdin().lock(), &mut to_socket);
        let _ = to_socket.shutdown(Shutdown::Write);
    });
    let mut from_socket = &socket;
    let mut stdout = io::stdout().lock();
    io::copy(&mut from_socket, &mut stdout)?;
    stdout.flush()
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
    socket: UnixStream,
    open_directions: AtomicU8,
}

enum Inbound {
    Data(Vec<u8>),
    Shutdown,
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
                // Streams the host opens are reserved for a later protocol version.
                Frame::Open { stream, .. } => self.send(&Frame::Close { stream })?,
                Frame::Hello { .. } => {}
            }
        }
        Ok(())
    }

    fn open(&self, channel: &str, socket: UnixStream) {
        let Ok(socket_for_stream) = socket.try_clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let entry = Arc::new(Stream {
            inbound: Mutex::new(Some(sender)),
            credit: Credit::new(STREAM_WINDOW as usize),
            socket: socket_for_stream,
            open_directions: AtomicU8::new(2),
        });
        let id = {
            let mut streams = self.inner.streams.lock().expect("bridge streams lock");
            if streams.len() >= MAX_STREAMS {
                return;
            }
            let id = self.inner.next_stream.fetch_add(2, Ordering::SeqCst);
            streams.insert(id, entry.clone());
            id
        };
        let opened = Frame::Open {
            stream: id,
            channel: channel.to_string(),
        };
        if self.send(&opened).is_err() {
            self.abort(id, false);
            return;
        }
        let Ok(reader) = socket.try_clone() else {
            self.abort(id, true);
            return;
        };
        let bridge = self.clone();
        let reader_entry = entry.clone();
        std::thread::spawn(move || bridge.forward_to_host(id, reader, &reader_entry));
        let bridge = self.clone();
        std::thread::spawn(move || bridge.forward_to_socket(id, socket, receiver));
    }

    fn forward_to_host(&self, id: u32, mut socket: UnixStream, entry: &Stream) {
        let mut buffer = vec![0u8; MAX_DATA];
        while let Some(allowance) = entry.credit.wait_available(MAX_DATA) {
            match socket.read(&mut buffer[..allowance]) {
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

    fn forward_to_socket(&self, id: u32, mut socket: UnixStream, inbound: mpsc::Receiver<Inbound>) {
        for message in inbound {
            match message {
                Inbound::Data(bytes) => {
                    if socket.write_all(&bytes).is_err() {
                        self.abort(id, true);
                        return;
                    }
                    let credit = Frame::Credit {
                        stream: id,
                        bytes: bytes.len() as u32,
                    };
                    if self.send(&credit).is_err() {
                        return;
                    }
                }
                Inbound::Shutdown => {
                    let _ = socket.shutdown(Shutdown::Write);
                    self.finish_direction(id);
                    return;
                }
            }
        }
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
        let _ = entry.socket.shutdown(Shutdown::Both);
        if notify_host {
            let _ = self.send(&Frame::Close { stream: id });
        }
    }
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
    use std::time::Duration;

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
                Frame::Open { stream, channel } => {
                    assert_eq!(channel, "orchestrator");
                    (client, stream)
                }
                other => panic!("expected an open frame, got {other:?}"),
            }
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
    fn streams_opened_by_the_host_are_refused() {
        let mut harness = start();
        harness.send(Frame::Open {
            stream: 2,
            channel: "relay:54549".into(),
        });
        assert_eq!(harness.next_frame(), Frame::Close { stream: 2 });
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
