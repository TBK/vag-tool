//! VW Transport Protocol 2.0 (TP 2.0) over CAN.
//!
//! Channel setup:
//! ```text
//! tester -> 0x200: <module> C0 00 10 00 03 01     ask the ECU to talk on 0x300
//! ECU  -> 0x200+m: 00 D0 <ecu tx lo hi> <our tx lo hi> 01
//! ```
//! Data frames are `<type nibble><seq nibble>` plus up to 7 bytes, and the
//! first frame of a message starts with a 16-bit big-endian length. Type 0x1
//! is the last frame and expects an ACK (`B0 | ((seq + 1) & 0x0F)`), 0x2 means
//! more follow. `A3` keeps an idle channel alive, `A8` drops it.

use std::time::Duration;

use tokio::time::Instant;

use crate::link::CanLink;
use crate::{Error, Result};

pub const BROADCAST_ADDR: u16 = 0x200;

// TP2.0 opcodes
pub const OP_SETUP_REQ: u8 = 0xC0;
pub const OP_SETUP_OK: u8 = 0xD0;
pub const OP_PARAM_REQ: u8 = 0xA0;
pub const OP_PARAM_RESP: u8 = 0xA1;
pub const OP_CHANNEL_TEST: u8 = 0xA3;
pub const OP_BREAK: u8 = 0xA4;
pub const OP_DISCONNECT: u8 = 0xA8;

/// Logical module addresses, numbered as VCDS shows them.
pub const MODULE_ENGINE: u8 = crate::modules::ENGINE;
/// 0x19, as VCDS and VAG's own symbol `DID_19` have it. This said 0x1F until
/// `crate::modules` arrived with a sourced table; 0x1F is not a module.
pub const MODULE_GATEWAY: u8 = crate::modules::GATEWAY;

/// TP2.0 timing parameter: the top two bits pick the base, the low six the count.
pub fn decode_timing(byte: u8) -> Duration {
    let base_ns = match byte >> 6 {
        0 => 100,
        1 => 1_000,
        2 => 10_000,
        _ => 100_000,
    };
    Duration::from_nanos(base_ns * u64::from(byte & 0x3F))
}

pub struct Tp20Channel<L: CanLink> {
    pub link: L,
    pub module: u8,
    pub debug: bool,
    timeout: Duration,
    rx_addr: u16,
    tx_addr: u16,
    tx_seq: u8,
    rx_seq: u8,
    gap: Duration,
    ack_timeout: Duration,
    last_tx: Instant,
    closed: bool,
}

impl<L: CanLink> Tp20Channel<L> {
    pub fn new(link: L, module: u8) -> Self {
        Tp20Channel {
            link,
            module,
            debug: false,
            timeout: Duration::from_millis(500),
            rx_addr: 0,
            tx_addr: 0,
            tx_seq: 0,
            rx_seq: 0,
            gap: Duration::from_millis(1),
            ack_timeout: Duration::from_millis(100),
            last_tx: Instant::now(),
            closed: true,
        }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn rx_addr(&self) -> u16 {
        self.rx_addr
    }

    pub fn tx_addr(&self) -> u16 {
        self.tx_addr
    }

    pub fn is_open(&self) -> bool {
        !self.closed
    }

    // -- plumbing ---------------------------------------------------------
    async fn put(&mut self, data: &[u8]) -> Result<()> {
        self.put_to(self.tx_addr, data).await
    }

    async fn put_to(&mut self, addr: u16, data: &[u8]) -> Result<()> {
        if self.debug {
            println!("TX {addr:03x}  {}", hex_bytes(data));
        }
        self.link.send(addr, data).await?;
        self.last_tx = Instant::now();
        if !self.gap.is_zero() {
            tokio::time::sleep(self.gap).await;
        }
        Ok(())
    }

    /// Next frame from `addr`, ignoring every other id on the bus.
    async fn take_from(&mut self, addr: u16, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "waiting for a TP2.0 frame from {addr:#05x}"
                )));
            }
            match self.link.recv(remaining).await? {
                Some(frame) if frame.id == addr && !frame.data.is_empty() => {
                    if self.debug {
                        println!("RX {:03x}  {}", frame.id, hex_bytes(&frame.data));
                    }
                    return Ok(frame.data);
                }
                Some(frame) => {
                    if self.debug {
                        println!(
                            "rx {:03x}  {}  (not ours)",
                            frame.id,
                            hex_bytes(&frame.data)
                        );
                    }
                    continue;
                }
                None => continue,
            }
        }
    }

    async fn take(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        self.take_from(self.rx_addr, deadline).await
    }

    fn deadline(&self) -> Instant {
        Instant::now() + self.timeout
    }

    // -- channel ----------------------------------------------------------
    pub async fn open(&mut self) -> Result<()> {
        let module = self.module;
        self.put_to(
            BROADCAST_ADDR,
            &[module, OP_SETUP_REQ, 0x00, 0x10, 0x00, 0x03, 0x01],
        )
        .await?;

        let deadline = Instant::now() + self.timeout.max(Duration::from_secs(1));
        let resp = self
            .take_from(BROADCAST_ADDR + u16::from(module), deadline)
            .await?;
        if resp.len() < 6 {
            return Err(Error::Tp20(format!(
                "short channel setup response: {}",
                hex_bytes(&resp)
            )));
        }
        if resp[1] != OP_SETUP_OK {
            return Err(Error::Tp20(format!(
                "ECU refused channel setup (opcode {:#04x}): {}",
                resp[1],
                hex_bytes(&resp)
            )));
        }

        let rx = u16::from(resp[2]) | (u16::from(resp[3]) << 8);
        let tx = u16::from(resp[4]) | (u16::from(resp[5]) << 8);
        if rx & 0xF000 != 0 || tx & 0xF000 != 0 {
            return Err(Error::Tp20(format!(
                "ECU marked a CAN id invalid: rx={rx:#06x} tx={tx:#06x}"
            )));
        }
        self.rx_addr = rx;
        self.tx_addr = tx;
        self.tx_seq = 0;
        self.rx_seq = 0;
        self.closed = false;
        if self.debug {
            println!(
                "channel open: we send on {:#05x}, listen on {:#05x}",
                self.tx_addr, self.rx_addr
            );
        }

        // Timing parameters: block size 15, T1 100 ms, T3 5 ms.
        self.put(&[OP_PARAM_REQ, 0x0F, 0x8A, 0xFF, 0x32, 0xFF])
            .await?;
        let deadline = self.deadline();
        let params = self.take(deadline).await?;
        if params.first() != Some(&OP_PARAM_RESP) {
            return Err(Error::Tp20(format!(
                "unexpected answer to the parameter request: {}",
                hex_bytes(&params)
            )));
        }
        if params.len() >= 5 {
            self.ack_timeout = decode_timing(params[2]).max(Duration::from_millis(50));
            self.gap = decode_timing(params[4]).max(Duration::from_millis(1));
        }
        if self.debug {
            println!(
                "timing: T1={}ms T3={}ms",
                self.ack_timeout.as_millis(),
                self.gap.as_millis()
            );
        }
        Ok(())
    }

    pub async fn close(&mut self) {
        if self.closed {
            return;
        }
        let _ = self.put(&[OP_DISCONNECT]).await;
        self.closed = true;
    }

    // -- acks -------------------------------------------------------------
    async fn wait_for_ack(&mut self) -> Result<()> {
        let want = 0xB0 | ((self.tx_seq + 1) & 0x0F);
        let deadline = Instant::now() + (self.ack_timeout * 3).max(Duration::from_millis(200));
        loop {
            let data = self.take(deadline).await.map_err(|_| {
                Error::Timeout("waiting for the ECU to acknowledge our message".into())
            })?;
            if data[0] == want {
                return Ok(());
            }
            if data[0] & 0xF0 == 0xB0 {
                return Err(Error::Tp20(format!(
                    "out of sequence ACK: got {:#04x}, wanted {want:#04x}",
                    data[0]
                )));
            }
            // Control frames can overtake the ack; keep looking.
        }
    }

    async fn send_ack(&mut self) -> Result<()> {
        let ack = 0xB0 | ((self.rx_seq + 1) & 0x0F);
        self.put(&[ack]).await
    }

    // -- messages ---------------------------------------------------------
    pub async fn send(&mut self, payload: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::Tp20("the channel is not open".into()));
        }
        if payload.len() > 0xFFFF {
            return Err(Error::Value("payload too long for TP2.0".into()));
        }

        let mut body = Vec::with_capacity(payload.len() + 2);
        body.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        body.extend_from_slice(payload);

        let frames = body.len().div_ceil(7);
        for (i, chunk) in body.chunks(7).enumerate() {
            let last = i + 1 == frames;
            let header = if last { 0x10 } else { 0x20 } | self.tx_seq;
            let mut frame = Vec::with_capacity(8);
            frame.push(header);
            frame.extend_from_slice(chunk);
            self.put(&frame).await?;
            if last {
                self.wait_for_ack().await?;
            }
            self.tx_seq = (self.tx_seq + 1) & 0x0F;
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        self.recv_within(self.timeout).await
    }

    pub async fn recv_within(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        if self.closed {
            return Err(Error::Tp20("the channel is not open".into()));
        }
        let deadline = Instant::now() + timeout;
        let mut buf: Vec<u8> = Vec::new();

        loop {
            let data = self.take(deadline).await?;
            let op = data[0];

            if op == OP_CHANNEL_TEST {
                // The ECU is probing us.
                self.put(&[OP_PARAM_RESP, 0x0F, 0x8A, 0xFF, 0x32, 0xFF])
                    .await?;
                continue;
            }
            if op == OP_PARAM_RESP || op == OP_BREAK || op & 0xF0 == 0xB0 {
                continue; // stale keepalive answer or a stray ack
            }

            let kind = op >> 4;
            let seq = op & 0x0F;
            if kind > 0x3 {
                return Err(Error::Tp20(format!(
                    "unexpected TP2.0 frame: {}",
                    hex_bytes(&data)
                )));
            }

            buf.extend_from_slice(&data[1..]);
            self.rx_seq = seq;
            if kind == 0x0 || kind == 0x1 {
                self.send_ack().await?;
            }
            if kind == 0x1 || kind == 0x3 {
                break;
            }
        }

        if buf.len() < 2 {
            return Err(Error::Tp20("truncated TP2.0 message".into()));
        }
        let length = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
        let payload = &buf[2..];
        if payload.len() < length {
            return Err(Error::Tp20(format!(
                "message says {length} bytes, got {}",
                payload.len()
            )));
        }
        Ok(payload[..length].to_vec())
    }

    /// Channel test. The ECU drops a channel left idle for about a second.
    pub async fn keepalive(&mut self) -> Result<()> {
        self.put(&[OP_CHANNEL_TEST]).await?;
        let deadline = Instant::now() + self.timeout.max(Duration::from_millis(300));
        loop {
            let data = self.take(deadline).await?;
            if data[0] == OP_PARAM_RESP {
                return Ok(());
            }
        }
    }

    pub async fn keepalive_if_idle(&mut self, interval: Duration) -> Result<()> {
        if self.last_tx.elapsed() >= interval {
            self.keepalive().await?;
        }
        Ok(())
    }
}

pub fn hex_bytes(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
