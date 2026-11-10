//! Framed request / reply layer over a [`Transport`].
//!
//! Outgoing [`HostMessage`]s get a sequence number and are COBS-framed by `attinav-core`; incoming
//! bytes are decoded, every [`DeviceMessage`] is forwarded to the frontend as a Tauri event
//! (`device:message`) and, when its `seq` matches a pending request, collected into that request's
//! reply according to the message's [`ReplyShape`] (see `PROTOCOL.md`, section 3).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use attinav_core::protocol::{Decoder, DeviceMessage, HostMessage, MAX_ENCODED, Status, shot_flags};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

/// Where device events go. The application hands in its [`AppHandle`]; tests use [`NullSink`].
pub trait Sink: Send + Sync + 'static {
    /// Delivers `payload` under the event name `event`.
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

impl Sink for AppHandle {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        let _ = Emitter::emit(self, event, payload);
    }
}

/// Discards every event.
pub struct NullSink;

impl Sink for NullSink {
    fn emit_json(&self, _event: &str, _payload: serde_json::Value) {}
}

fn emit<T: Serialize>(sink: &dyn Sink, event: &str, payload: &T) {
    if let Ok(v) = serde_json::to_value(payload) {
        sink.emit_json(event, v);
    }
}
use tracing::{debug, info, warn};

use crate::error::{ClientError, Result};
use crate::transport::Transport;

/// Event sent to the frontend for every decoded device frame.
#[derive(Clone, Serialize)]
pub struct MessageEvent {
    /// Sequence number (0 = unsolicited).
    pub seq: u8,
    /// The message.
    pub message: DeviceMessage,
}

/// Event sent when the link opens or closes.
#[derive(Clone, Serialize)]
pub struct LinkEvent {
    /// `true` after a successful open, `false` on close.
    pub connected: bool,
    /// Human-readable transport description.
    pub description: String,
    /// `true` for the built-in simulator.
    pub simulator: bool,
    /// Why the link closed (`None` for a requested disconnect).
    pub reason: Option<String>,
}

/// How many frames answer a request.
#[derive(Clone, Copy, Debug)]
pub enum ReplyShape {
    /// Exactly one frame (an `Ack` or a dedicated reply).
    Single,
    /// `ShotResult`, then five `ShotRaw` when raw data was requested; or an `Ack`.
    Shot {
        /// `WANT_RAW` was set.
        want_raw: bool,
    },
    /// Any number of frames terminated by an `Ack` (`CalGetShots`).
    UntilAck,
}

impl ReplyShape {
    /// Shape of the reply to `msg`.
    #[must_use]
    pub const fn of(msg: &HostMessage) -> Self {
        match msg {
            HostMessage::Shot(r) => Self::Shot {
                want_raw: r.flags & shot_flags::WANT_RAW != 0,
            },
            HostMessage::CalGetShots => Self::UntilAck,
            _ => Self::Single,
        }
    }

    fn complete(self, got: &[DeviceMessage]) -> bool {
        match got.last() {
            None => false,
            Some(DeviceMessage::Ack(_)) => true,
            Some(_) => match self {
                Self::Single => true,
                Self::Shot { want_raw } => {
                    !want_raw || got.iter().filter(|m| matches!(m, DeviceMessage::ShotRaw(_))).count() >= 5
                }
                Self::UntilAck => false,
            },
        }
    }
}

struct Pending {
    shape: ReplyShape,
    got: Vec<DeviceMessage>,
    done: oneshot::Sender<Vec<DeviceMessage>>,
}

struct Inner {
    to_device: mpsc::Sender<Vec<u8>>,
    pending: Mutex<HashMap<u8, Pending>>,
    next_seq: AtomicU8,
    open: std::sync::atomic::AtomicBool,
    description: String,
    simulator: bool,
    transport_stop: Arc<std::sync::atomic::AtomicBool>,
}

/// A live connection to one device. Cheap to clone; all clones share the connection.
#[derive(Clone)]
pub struct Link {
    inner: Arc<Inner>,
}

impl Link {
    /// Starts the reader task on `transport` and announces the link to the frontend.
    pub fn start(app: Arc<dyn Sink>, transport: Transport, description: String, simulator: bool) -> Self {
        let stop = transport.stop_flag();
        let Transport {
            to_device,
            mut from_device,
            error,
            ..
        } = transport;
        let inner = Arc::new(Inner {
            to_device,
            pending: Mutex::new(HashMap::new()),
            next_seq: AtomicU8::new(1),
            open: std::sync::atomic::AtomicBool::new(true),
            description: description.clone(),
            simulator,
            transport_stop: stop,
        });
        let link = Self {
            inner: Arc::clone(&inner),
        };

        emit(
            app.as_ref(),
            "device:link",
            &LinkEvent {
                connected: true,
                description: description.clone(),
                simulator,
                reason: None,
            },
        );
        info!("link open: {description}");

        tauri::async_runtime::spawn(async move {
            let mut dec = Box::new(Decoder::new());
            while let Some(bytes) = from_device.recv().await {
                for b in bytes {
                    match dec.push(b) {
                        None => {}
                        Some(Err(e)) => warn!("dropped frame: {e:?}"),
                        Some(Ok(hdr)) => match DeviceMessage::read_payload(hdr.kind, dec.payload()) {
                            Err(e) => warn!("bad payload for kind {:#04x}: {e:?}", hdr.kind),
                            Ok(msg) => {
                                emit(
                                    app.as_ref(),
                                    "device:message",
                                    &MessageEvent {
                                        seq: hdr.seq,
                                        message: msg,
                                    },
                                );
                                if hdr.seq != 0 {
                                    inner.deliver(hdr.seq, msg);
                                }
                            }
                        },
                    }
                }
            }
            inner.open.store(false, Ordering::SeqCst);
            inner
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            let reason = error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            info!("link closed: {description} ({reason:?})");
            emit(
                app.as_ref(),
                "device:link",
                &LinkEvent {
                    connected: false,
                    description,
                    simulator,
                    reason,
                },
            );
        });
        link
    }

    /// `true` while the transport is alive.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.inner.open.load(Ordering::SeqCst)
    }

    /// Transport description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.inner.description
    }

    /// `true` for the simulator.
    #[must_use]
    pub fn is_simulator(&self) -> bool {
        self.inner.simulator
    }

    /// Closes the transport; the reader task ends and announces the closure.
    pub fn close(&self) {
        self.inner.transport_stop.store(true, Ordering::SeqCst);
        self.inner.open.store(false, Ordering::SeqCst);
    }

    /// Sends `msg` and collects its reply frames.
    pub async fn request(&self, msg: &HostMessage, timeout: Duration) -> Result<Vec<DeviceMessage>> {
        if !self.is_open() {
            return Err(ClientError::NotConnected);
        }
        let shape = ReplyShape::of(msg);
        let (seq, rx) = self.inner.register(shape)?;
        let mut buf = vec![0u8; MAX_ENCODED];
        let n = msg.encode(seq, &mut buf)?;
        buf.truncate(n);
        debug!("→ seq {seq} kind {:#04x} ({n} bytes)", msg.kind());
        if self.inner.to_device.send(buf).await.is_err() {
            self.inner.forget(seq);
            return Err(ClientError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(got)) => Ok(got),
            Ok(Err(_)) => Err(ClientError::Closed),
            Err(_) => {
                self.inner.forget(seq);
                Err(ClientError::Timeout)
            }
        }
    }

    /// Sends `msg` expecting one frame; a non-`Ok` `Ack` becomes [`ClientError::Nack`].
    pub async fn request_one(&self, msg: &HostMessage, timeout: Duration) -> Result<DeviceMessage> {
        let got = self.request(msg, timeout).await?;
        let first = got.into_iter().next().ok_or(ClientError::Closed)?;
        if let DeviceMessage::Ack(a) = first
            && a.status != Status::Ok
        {
            return Err(ClientError::Nack {
                request: a.request,
                status: a.status,
                detail: a.detail,
            });
        }
        Ok(first)
    }

    /// Sends `msg` expecting an `Ack Ok`; returns its detail.
    pub async fn ack(&self, msg: &HostMessage, timeout: Duration) -> Result<u16> {
        match self.request_one(msg, timeout).await? {
            DeviceMessage::Ack(a) => Ok(a.detail),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }
}

impl Inner {
    fn register(&self, shape: ReplyShape) -> Result<(u8, oneshot::Receiver<Vec<DeviceMessage>>)> {
        let mut pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // 255 outstanding requests would be a bug; pick the next free non-zero number.
        for _ in 0..255 {
            let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
            if seq == 0 {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = pending.entry(seq) {
                let (tx, rx) = oneshot::channel();
                e.insert(Pending {
                    shape,
                    got: Vec::new(),
                    done: tx,
                });
                return Ok((seq, rx));
            }
        }
        Err(ClientError::Invalid("too many outstanding requests".into()))
    }

    fn forget(&self, seq: u8) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&seq);
    }

    #[allow(
        clippy::large_types_passed_by_value,
        reason = "the message is moved into the reply vector"
    )]
    fn deliver(&self, seq: u8, msg: DeviceMessage) {
        let mut pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(p) = pending.get_mut(&seq) else {
            debug!("reply for unknown seq {seq}: {:#04x}", msg.kind());
            return;
        };
        p.got.push(msg);
        if p.shape.complete(&p.got)
            && let Some(p) = pending.remove(&seq)
        {
            let _ = p.done.send(p.got);
        }
    }
}
