//! Offline reassembly of a sniffed TP2.0 session.
//!
//! Point this at a candump log of someone else's VCDS session and it prints the
//! KWP2000 messages, flagging everything that is not a plain read. That is how
//! the Coding-II sequence this tool cannot guess gets recovered: capture the
//! session, find the flagged message, replay it with `vag-tool raw`.
//!
//! Both candump formats are accepted:
//! ```text
//! (1612345678.123456) can0 740#1000022101
//! can0  740   [5]  10 00 02 21 01
//! ```

use crate::kwp2000::{changes_ecu, service_name};
use crate::tp20::{BROADCAST_ADDR, hex_bytes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Tester,
    Ecu,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub at: f64,
    pub direction: Direction,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn service(&self) -> Option<u8> {
        self.payload.first().copied()
    }

    /// True when the message is not a known read-only service, in either
    /// direction. Deliberately broad: a VAG-specific service nobody has
    /// identified is exactly what we are hunting for.
    pub fn is_write(&self) -> bool {
        match self.service() {
            None | Some(0x7F) => false,
            Some(sid) => match self.direction {
                Direction::Ecu => changes_ecu(sid.wrapping_sub(0x40)),
                Direction::Tester => changes_ecu(sid),
            },
        }
    }

    pub fn describe(&self) -> String {
        let Some(sid) = self.service() else {
            return "empty".to_string();
        };
        if sid == 0x7F {
            let target = self.payload.get(1).copied().unwrap_or(0);
            return format!("negative response to {}", service_name(target));
        }
        if self.direction == Direction::Ecu {
            let requested = sid.wrapping_sub(0x40);
            let name = service_name(requested);
            if !name.starts_with("service ") {
                return format!("{name} response");
            }
        }
        service_name(sid)
    }

    pub fn format(&self) -> String {
        let arrow = match self.direction {
            Direction::Tester => "->",
            Direction::Ecu => "<-",
        };
        let mark = if self.is_write() {
            "  *** not a known read-only service"
        } else {
            ""
        };
        format!(
            "{:.3} {arrow} {}    {}{mark}",
            self.at,
            hex_bytes(&self.payload),
            self.describe()
        )
    }
}

pub fn parse_frame_line(line: &str) -> Option<(f64, u16, Vec<u8>)> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('(') {
        // (timestamp) iface id#data
        let (timestamp, rest) = rest.split_once(')')?;
        let mut fields = rest.split_whitespace();
        let _iface = fields.next()?;
        let (id, data) = fields.next()?.split_once('#')?;
        return Some((
            timestamp.parse().ok()?,
            u16::from_str_radix(id, 16).ok()?,
            hex::decode(data.replace(' ', "")).ok()?,
        ));
    }

    // iface id [dlc] b0 b1 ...
    let mut fields = line.split_whitespace();
    let _iface = fields.next()?;
    let id = u16::from_str_radix(fields.next()?, 16).ok()?;
    let dlc = fields.next()?;
    if !dlc.starts_with('[') {
        return None;
    }
    let data = fields
        .map(|byte| u8::from_str_radix(byte, 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .ok()?;
    Some((0.0, id, data))
}

/// Reassembles TP2.0 frames into KWP2000 messages.
#[derive(Debug, Default)]
pub struct SessionDecoder {
    pub module: u8,
    pub tester_tx: Option<u16>,
    pub ecu_tx: Option<u16>,
    tester_buf: Vec<u8>,
    ecu_buf: Vec<u8>,
    pub messages: Vec<Message>,
}

impl SessionDecoder {
    pub fn new(module: u8) -> Self {
        SessionDecoder {
            module,
            ..Default::default()
        }
    }

    pub fn feed(&mut self, at: f64, id: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if id == BROADCAST_ADDR || id == BROADCAST_ADDR + u16::from(self.module) {
            if data.len() >= 6 && data[1] == 0xD0 {
                self.ecu_tx = Some(u16::from(data[2]) | (u16::from(data[3]) << 8));
                self.tester_tx = Some(u16::from(data[4]) | (u16::from(data[5]) << 8));
            }
            return;
        }

        let direction = if Some(id) == self.tester_tx {
            Direction::Tester
        } else if Some(id) == self.ecu_tx {
            Direction::Ecu
        } else {
            return;
        };

        let op = data[0];
        if matches!(op & 0xF0, 0xA0 | 0xB0 | 0xC0 | 0xD0) {
            return; // control frame or ack
        }
        let kind = op >> 4;
        if kind > 0x3 {
            return;
        }

        let buf = match direction {
            Direction::Tester => &mut self.tester_buf,
            Direction::Ecu => &mut self.ecu_buf,
        };
        buf.extend_from_slice(&data[1..]);

        if kind == 0x1 || kind == 0x3 {
            let complete = std::mem::take(buf);
            if complete.len() < 2 {
                return;
            }
            let length = usize::from(u16::from_be_bytes([complete[0], complete[1]]));
            let payload = &complete[2..];
            if payload.len() < length || length == 0 {
                return;
            }
            self.messages.push(Message {
                at,
                direction,
                payload: payload[..length].to_vec(),
            });
        }
    }

    pub fn feed_lines<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) -> &[Message] {
        for line in lines {
            if let Some((at, id, data)) = parse_frame_line(line) {
                self.feed(at, id, &data);
            }
        }
        &self.messages
    }
}
