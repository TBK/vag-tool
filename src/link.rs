//! The CAN link the protocol code talks to.
//!
//! Two implementations: [`AdapterLink`] over an [`automotive`] adapter (a real
//! interface), and [`LoopbackLink`], an in-memory pair so the TP2.0 and
//! KWP2000 layers can be driven against [`crate::simecu`] with no hardware.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use automotive::StreamExt;
use automotive::can::AsyncCanAdapter;
use tokio::sync::mpsc;

use crate::{Error, Result};

/// One classic CAN frame, reduced to what TP2.0 needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u16,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(id: u16, data: impl Into<Vec<u8>>) -> Self {
        Frame {
            id,
            data: data.into(),
        }
    }
}

pub trait CanLink: Send {
    /// Put a frame on the bus.
    fn send(&self, id: u16, data: &[u8]) -> impl Future<Output = Result<()>> + Send;

    /// Next frame, or `None` if `timeout` passed first.
    fn recv(&mut self, timeout: Duration) -> impl Future<Output = Result<Option<Frame>>> + Send;
}

/// A real CAN interface.
///
/// Frames arrive through a forwarding task rather than a borrowed stream: the
/// adapter's `recv()` borrows it, so the task owns both and hands frames over a
/// channel. The task also drops loopback echoes of our own frames, which the
/// adapter needs internally to match ACKs but TP2.0 must never see.
pub struct AdapterLink {
    adapter: Arc<AsyncCanAdapter>,
    rx: mpsc::UnboundedReceiver<Frame>,
    send_timeout: Duration,
    _forwarder: tokio::task::JoinHandle<()>,
}

impl AdapterLink {
    /// Open a SocketCAN interface by name, e.g. `can0` or `vcan0`.
    pub fn open(interface: &str) -> Result<Self> {
        let adapter = automotive::socketcan::SocketCan::new_async(interface)
            .map_err(|e| Error::Can(format!("{interface}: {e}")))?;
        Ok(Self::from_adapter(adapter))
    }

    /// Wrap an adapter the caller already opened.
    pub fn from_adapter(adapter: AsyncCanAdapter) -> Self {
        let adapter = Arc::new(adapter);
        let (tx, rx) = mpsc::unbounded_channel();
        let owned = Arc::clone(&adapter);
        let forwarder = tokio::spawn(async move {
            let mut stream = owned.recv();
            while let Some(frame) = stream.next().await {
                if frame.loopback {
                    continue;
                }
                let id: u32 = frame.id.into();
                if tx.send(Frame::new(id as u16, frame.data)).is_err() {
                    break;
                }
            }
        });
        AdapterLink {
            adapter,
            rx,
            send_timeout: Duration::from_millis(500),
            _forwarder: forwarder,
        }
    }
}

impl CanLink for AdapterLink {
    async fn send(&self, id: u16, data: &[u8]) -> Result<()> {
        let frame = automotive::can::Frame::new(0, (id as u32).into(), data)
            .map_err(|e| Error::Can(format!("{e}")))?;
        // The adapter resolves this once the frame is ACKed on the bus; on a
        // quiet bus that never happens, so it is bounded.
        tokio::time::timeout(self.send_timeout, self.adapter.send(&frame))
            .await
            .map_err(|_| {
                Error::Timeout(format!(
                    "waiting for {id:#05x} to be acknowledged on the bus"
                ))
            })
    }

    async fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        match tokio::time::timeout(timeout, self.rx.recv()).await {
            Ok(Some(frame)) => Ok(Some(frame)),
            Ok(None) => Err(Error::Can("the CAN interface closed".into())),
            Err(_) => Ok(None),
        }
    }
}

/// One end of an in-memory two-node bus.
pub struct LoopbackLink {
    tx: mpsc::UnboundedSender<Frame>,
    rx: mpsc::UnboundedReceiver<Frame>,
}

impl LoopbackLink {
    /// Two ends of the same bus: what one sends, the other receives.
    pub fn pair() -> (LoopbackLink, LoopbackLink) {
        let (tx_a, rx_b) = mpsc::unbounded_channel();
        let (tx_b, rx_a) = mpsc::unbounded_channel();
        (
            LoopbackLink { tx: tx_a, rx: rx_a },
            LoopbackLink { tx: tx_b, rx: rx_b },
        )
    }
}

impl CanLink for LoopbackLink {
    async fn send(&self, id: u16, data: &[u8]) -> Result<()> {
        self.tx
            .send(Frame::new(id, data.to_vec()))
            .map_err(|_| Error::Can("the other end of the loopback bus is gone".into()))
    }

    async fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        match tokio::time::timeout(timeout, self.rx.recv()).await {
            Ok(Some(frame)) => Ok(Some(frame)),
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }
}
