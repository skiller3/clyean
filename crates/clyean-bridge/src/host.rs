// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The host end of the bridge.  It reads the frames the bridge writes to the standard
//! output of its `podman exec` session, connects every stream the bridge opens to the
//! host side of its channel through a [`Connector`], and writes the replies to the
//! session's standard input.  Through a [`BridgeHandle`], the host also opens streams of
//! its own, which the bridge connects to a port on the container's loopback.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::{mpsc, oneshot, watch, Notify};

use crate::protocol::{
    checked_length, is_bridge_stream, Frame, Peer, MAX_DATA, MAX_HOST_STREAMS, PROTOCOL_VERSION,
    STREAM_WINDOW,
};

/// The host side of one stream.
pub trait LocalStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> LocalStream for T {}

pub type ConnectFuture<'a> =
    Pin<Box<dyn Future<Output = io::Result<Box<dyn LocalStream>>> + Send + 'a>>;

/// Opens the host side of a channel for each stream the bridge opens, knowing the process
/// that connected inside the container when the bridge could tell.  An error refuses the
/// stream, which closes the connection inside the container.
pub trait Connector: Send + Sync + 'static {
    fn connect<'a>(&'a self, channel: &'a str, peer: Option<&'a Peer>) -> ConnectFuture<'a>;
}

/// What the host learns once the bridge has introduced itself: its release version, and
/// the handle through which the host opens streams into the container.
pub struct Introduction {
    pub version: String,
    pub handle: BridgeHandle,
}

/// Reads one frame, or `None` when the input ends.
pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut prefix = [0u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = checked_length(prefix)?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Frame::decode(&body).map(Some)
}

/// Serves one bridge session until the bridge's output ends.  `introduced` receives the
/// bridge's release version and a [`BridgeHandle`] once its first frame proves it speaks
/// this protocol; a bridge that speaks another one is refused.
pub async fn serve<R, W>(
    bridge_output: R,
    bridge_input: W,
    connector: Arc<dyn Connector>,
    introduced: oneshot::Sender<Introduction>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(bridge_output);
    let version = expect_hello(&mut reader).await?;
    let (frames, outgoing) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write_frames(bridge_input, outgoing));
    let session = Session {
        frames,
        streams: Arc::new(Mutex::new(HashMap::new())),
        connector,
        next_host_stream: Arc::new(AtomicU32::new(2)),
    };
    let _ = introduced.send(Introduction {
        version,
        handle: BridgeHandle {
            session: session.clone(),
        },
    });
    let result = session.pump(&mut reader).await;
    session.close_all();
    writer.abort();
    result
}

/// Opens streams from the host into the container of one bridge session.
#[derive(Clone)]
pub struct BridgeHandle {
    session: Session,
}

impl BridgeHandle {
    /// Opens a stream on `channel`, such as `relay:tcp:3000`, and returns the host's end
    /// of it.  The stream reads as ended when the bridge refuses it or the connection
    /// inside the container ends.
    pub fn open_stream(&self, channel: &str) -> io::Result<DuplexStream> {
        self.session.open_host_stream(channel)
    }

    /// Whether the bridge session is still running.
    pub fn is_open(&self) -> bool {
        !self.session.frames.is_closed()
    }
}

async fn expect_hello(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<String> {
    match read_frame(reader).await? {
        Some(Frame::Hello { protocol, version }) if protocol == PROTOCOL_VERSION => Ok(version),
        Some(Frame::Hello { protocol, version }) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the bridge (clyean-bridge {version}) speaks bridge protocol {protocol}, but this clyean speaks protocol {PROTOCOL_VERSION}; use a bridge from the same clyean release"
            ),
        )),
        Some(other) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the bridge began with {other:?} instead of introducing itself"),
        )),
        None => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the bridge ended before introducing itself",
        )),
    }
}

async fn write_frames<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut frames: mpsc::UnboundedReceiver<Frame>,
) {
    while let Some(frame) = frames.recv().await {
        if writer.write_all(&frame.encode()).await.is_err() {
            return;
        }
        if frames.is_empty() && writer.flush().await.is_err() {
            return;
        }
    }
}

#[derive(Clone)]
struct Session {
    frames: mpsc::UnboundedSender<Frame>,
    streams: Arc<Mutex<HashMap<u32, Arc<HostStream>>>>,
    connector: Arc<dyn Connector>,
    next_host_stream: Arc<AtomicU32>,
}

struct HostStream {
    inbound: mpsc::UnboundedSender<Inbound>,
    credit: Credit,
    open_directions: AtomicU8,
    cancel: watch::Sender<bool>,
    host_opened: bool,
}

enum Inbound {
    Data(Vec<u8>),
    Shutdown,
}

impl Session {
    async fn pump(&self, reader: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
        while let Some(frame) = read_frame(reader).await? {
            match frame {
                Frame::Open {
                    stream,
                    channel,
                    peer,
                } if is_bridge_stream(stream) => self.open(stream, channel, peer),
                Frame::Open { stream, .. } => {
                    let _ = self.frames.send(Frame::Close { stream });
                }
                Frame::Data { stream, bytes } => self.deliver(stream, Inbound::Data(bytes)),
                Frame::Shutdown { stream } => self.deliver(stream, Inbound::Shutdown),
                Frame::Credit { stream, bytes } => {
                    if let Some(entry) = self.stream(stream) {
                        entry.credit.add(bytes as usize);
                    }
                }
                Frame::Close { stream } => self.abort(stream, false),
                Frame::Hello { .. } => {}
            }
        }
        Ok(())
    }

    fn open(&self, id: u32, channel: String, peer: Option<Peer>) {
        let (entry, receiver, cancelled) = HostStream::new(false);
        self.streams
            .lock()
            .expect("bridge streams lock")
            .insert(id, entry.clone());
        let session = self.clone();
        tokio::spawn(async move {
            match session.connector.connect(&channel, peer.as_ref()).await {
                Ok(local) => {
                    session
                        .run_stream(id, entry, local, receiver, cancelled)
                        .await
                }
                Err(_) => session.abort(id, true),
            }
        });
    }

    /// Registers a stream the host opens, asks the bridge to connect it, and serves it
    /// through one end of an in-memory pipe whose other end the caller receives.
    fn open_host_stream(&self, channel: &str) -> io::Result<DuplexStream> {
        if self.frames.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the bridge session has ended",
            ));
        }
        let (entry, receiver, cancelled) = HostStream::new(true);
        let id = {
            let mut streams = self.streams.lock().expect("bridge streams lock");
            if streams.values().filter(|stream| stream.host_opened).count() >= MAX_HOST_STREAMS {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "the host already has {MAX_HOST_STREAMS} streams open into the container"
                    ),
                ));
            }
            let id = self.next_host_stream.fetch_add(2, Ordering::SeqCst);
            streams.insert(id, entry.clone());
            id
        };
        let (caller_end, local_end) = tokio::io::duplex(STREAM_WINDOW as usize);
        let opened = Frame::Open {
            stream: id,
            channel: channel.to_string(),
            peer: None,
        };
        if self.frames.send(opened).is_err() {
            self.abort(id, false);
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the bridge session has ended",
            ));
        }
        let session = self.clone();
        tokio::spawn(async move {
            session
                .run_stream(id, entry, Box::new(local_end), receiver, cancelled)
                .await
        });
        Ok(caller_end)
    }

    async fn run_stream(
        self,
        id: u32,
        entry: Arc<HostStream>,
        local: Box<dyn LocalStream>,
        receiver: mpsc::UnboundedReceiver<Inbound>,
        mut cancelled: watch::Receiver<bool>,
    ) {
        let (local_reader, local_writer) = tokio::io::split(local);
        let to_bridge = self.clone().forward_to_bridge(id, entry, local_reader);
        let to_local = self.clone().forward_to_local(id, local_writer, receiver);
        tokio::select! {
            _ = async { tokio::join!(to_bridge, to_local) } => {}
            _ = cancelled.wait_for(|cancelled| *cancelled) => {}
        }
    }

    async fn forward_to_bridge(
        self,
        id: u32,
        entry: Arc<HostStream>,
        mut local: impl AsyncRead + Unpin,
    ) {
        let mut buffer = vec![0u8; MAX_DATA];
        while let Some(allowance) = entry.credit.wait_available(MAX_DATA).await {
            match local.read(&mut buffer[..allowance]).await {
                Ok(0) => {
                    if self.stream(id).is_some() {
                        let _ = self.frames.send(Frame::Shutdown { stream: id });
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
                    if self.frames.send(data).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    self.abort(id, true);
                    return;
                }
            }
        }
    }

    async fn forward_to_local(
        self,
        id: u32,
        mut local: impl AsyncWrite + Unpin,
        mut inbound: mpsc::UnboundedReceiver<Inbound>,
    ) {
        while let Some(message) = inbound.recv().await {
            match message {
                Inbound::Data(bytes) => {
                    if local.write_all(&bytes).await.is_err() || local.flush().await.is_err() {
                        self.abort(id, true);
                        return;
                    }
                    let credit = Frame::Credit {
                        stream: id,
                        bytes: bytes.len() as u32,
                    };
                    if self.frames.send(credit).is_err() {
                        return;
                    }
                }
                Inbound::Shutdown => {
                    let _ = local.shutdown().await;
                    self.finish_direction(id);
                    return;
                }
            }
        }
    }

    fn stream(&self, id: u32) -> Option<Arc<HostStream>> {
        self.streams
            .lock()
            .expect("bridge streams lock")
            .get(&id)
            .cloned()
    }

    fn deliver(&self, id: u32, message: Inbound) {
        if let Some(entry) = self.stream(id) {
            let _ = entry.inbound.send(message);
        }
    }

    fn finish_direction(&self, id: u32) {
        let Some(entry) = self.stream(id) else {
            return;
        };
        if entry.open_directions.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.streams
                .lock()
                .expect("bridge streams lock")
                .remove(&id);
            entry.credit.close();
        }
    }

    fn abort(&self, id: u32, notify_bridge: bool) {
        let removed = self
            .streams
            .lock()
            .expect("bridge streams lock")
            .remove(&id);
        let Some(entry) = removed else {
            return;
        };
        entry.credit.close();
        let _ = entry.cancel.send(true);
        if notify_bridge {
            let _ = self.frames.send(Frame::Close { stream: id });
        }
    }

    /// Ends every stream at once, when the bridge itself is gone.
    fn close_all(&self) {
        let streams: Vec<Arc<HostStream>> = self
            .streams
            .lock()
            .expect("bridge streams lock")
            .drain()
            .map(|(_, entry)| entry)
            .collect();
        for entry in streams {
            entry.credit.close();
            let _ = entry.cancel.send(true);
        }
    }
}

impl HostStream {
    fn new(
        host_opened: bool,
    ) -> (
        Arc<Self>,
        mpsc::UnboundedReceiver<Inbound>,
        watch::Receiver<bool>,
    ) {
        let (inbound, receiver) = mpsc::unbounded_channel();
        let (cancel, cancelled) = watch::channel(false);
        let entry = Arc::new(Self {
            inbound,
            credit: Credit::new(STREAM_WINDOW as usize),
            open_directions: AtomicU8::new(2),
            cancel,
            host_opened,
        });
        (entry, receiver, cancelled)
    }
}

/// How many bytes the host may still send on a stream before the bridge returns credit.
struct Credit {
    state: Mutex<CreditState>,
    changed: Notify,
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
            changed: Notify::new(),
        }
    }

    async fn wait_available(&self, max: usize) -> Option<usize> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.state.lock().expect("credit lock");
                if state.closed {
                    return None;
                }
                if state.available > 0 {
                    return Some(state.available.min(max));
                }
            }
            notified.await;
        }
    }

    fn consume(&self, used: usize) {
        let mut state = self.state.lock().expect("credit lock");
        state.available = state.available.saturating_sub(used);
    }

    fn add(&self, granted: usize) {
        let mut state = self.state.lock().expect("credit lock");
        state.available = state.available.saturating_add(granted);
        drop(state);
        self.changed.notify_waiters();
    }

    fn close(&self) {
        self.state.lock().expect("credit lock").closed = true;
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, DuplexStream};

    /// Serves `echo`, which returns every byte, and refuses every other channel.
    struct EchoConnector;

    impl Connector for EchoConnector {
        fn connect<'a>(&'a self, channel: &'a str, _peer: Option<&'a Peer>) -> ConnectFuture<'a> {
            Box::pin(async move {
                if channel != "echo" {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "no such channel"));
                }
                let (client, mut server) = duplex(1024);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    loop {
                        match server.read(&mut buffer).await {
                            Ok(0) | Err(_) => {
                                let _ = server.shutdown().await;
                                return;
                            }
                            Ok(n) => {
                                if server.write_all(&buffer[..n]).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
                Ok(Box::new(client) as Box<dyn LocalStream>)
            })
        }
    }

    struct FakeBridge {
        to_host: DuplexStream,
        from_host: DuplexStream,
        session: tokio::task::JoinHandle<io::Result<()>>,
        introduced: Option<oneshot::Receiver<Introduction>>,
    }

    fn start() -> FakeBridge {
        let (to_host, host_reader) = duplex(1 << 20);
        let (host_writer, from_host) = duplex(1 << 20);
        let (introduced, introduction) = oneshot::channel();
        let session = tokio::spawn(serve(
            host_reader,
            host_writer,
            Arc::new(EchoConnector),
            introduced,
        ));
        FakeBridge {
            to_host,
            from_host,
            session,
            introduced: Some(introduction),
        }
    }

    impl FakeBridge {
        async fn send(&mut self, frame: Frame) {
            self.to_host.write_all(&frame.encode()).await.unwrap();
        }

        async fn next_frame(&mut self) -> Frame {
            read_frame(&mut self.from_host)
                .await
                .unwrap()
                .expect("a frame")
        }

        /// Introduces the bridge and returns the host's handle.
        async fn introduce(&mut self) -> BridgeHandle {
            self.send(Frame::hello()).await;
            let introduction = self.introduced.take().unwrap().await.unwrap();
            assert_eq!(introduction.version, crate::VERSION);
            introduction.handle
        }
    }

    #[tokio::test]
    async fn a_bridge_speaking_another_protocol_is_refused() {
        let mut bridge = start();
        bridge
            .send(Frame::Hello {
                protocol: PROTOCOL_VERSION + 1,
                version: "9.9.9".into(),
            })
            .await;
        let error = bridge.session.await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("clyean-bridge 9.9.9"));
    }

    #[tokio::test]
    async fn streams_reach_their_channel_and_return_credit() {
        let mut bridge = start();
        bridge.send(Frame::hello()).await;
        bridge
            .send(Frame::Open {
                stream: 1,
                channel: "echo".into(),
                peer: None,
            })
            .await;
        bridge
            .send(Frame::Data {
                stream: 1,
                bytes: b"hello".to_vec(),
            })
            .await;
        let mut credit_seen = false;
        let mut echoed = Vec::new();
        while !credit_seen || echoed.len() < 5 {
            match bridge.next_frame().await {
                Frame::Credit { stream: 1, bytes } => {
                    assert_eq!(bytes, 5);
                    credit_seen = true;
                }
                Frame::Data { stream: 1, bytes } => echoed.extend(bytes),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(echoed, b"hello");
        bridge.send(Frame::Shutdown { stream: 1 }).await;
        assert_eq!(bridge.next_frame().await, Frame::Shutdown { stream: 1 });
    }

    #[tokio::test]
    async fn unknown_channels_and_host_numbered_streams_are_closed() {
        let mut bridge = start();
        bridge.send(Frame::hello()).await;
        bridge
            .send(Frame::Open {
                stream: 3,
                channel: "nowhere".into(),
                peer: None,
            })
            .await;
        assert_eq!(bridge.next_frame().await, Frame::Close { stream: 3 });
        bridge
            .send(Frame::Open {
                stream: 4,
                channel: "echo".into(),
                peer: None,
            })
            .await;
        assert_eq!(bridge.next_frame().await, Frame::Close { stream: 4 });
    }

    #[tokio::test]
    async fn host_opened_streams_use_even_ids_and_end_when_the_bridge_closes_them() {
        let mut bridge = start();
        let handle = bridge.introduce().await;
        let mut local = handle.open_stream("relay:tcp:3000").unwrap();
        assert_eq!(
            bridge.next_frame().await,
            Frame::Open {
                stream: 2,
                channel: "relay:tcp:3000".into(),
                peer: None
            }
        );
        local.write_all(b"hi").await.unwrap();
        assert_eq!(
            bridge.next_frame().await,
            Frame::Data {
                stream: 2,
                bytes: b"hi".to_vec()
            }
        );
        bridge
            .send(Frame::Data {
                stream: 2,
                bytes: b"yo".to_vec(),
            })
            .await;
        let mut reply = [0u8; 2];
        local.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"yo");
        assert_eq!(
            bridge.next_frame().await,
            Frame::Credit {
                stream: 2,
                bytes: 2
            }
        );
        let _second = handle.open_stream("relay:udp:53").unwrap();
        assert!(matches!(
            bridge.next_frame().await,
            Frame::Open { stream: 4, .. }
        ));
        bridge.send(Frame::Close { stream: 2 }).await;
        let mut rest = Vec::new();
        local.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn the_host_may_not_open_more_streams_than_its_share() {
        let mut bridge = start();
        let handle = bridge.introduce().await;
        let mut streams = Vec::new();
        for _ in 0..MAX_HOST_STREAMS {
            streams.push(handle.open_stream("relay:tcp:3000").unwrap());
        }
        let refused = handle.open_stream("relay:tcp:3000").unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        assert!(handle.is_open());
    }

    #[tokio::test]
    async fn the_session_ends_when_the_bridge_output_ends() {
        let mut bridge = start();
        bridge.send(Frame::hello()).await;
        bridge
            .send(Frame::Open {
                stream: 1,
                channel: "echo".into(),
                peer: None,
            })
            .await;
        drop(bridge.to_host);
        bridge.session.await.unwrap().unwrap();
    }
}
