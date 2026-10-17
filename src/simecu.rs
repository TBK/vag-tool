//! A small fake PPD1-style engine ECU speaking TP2.0 and KWP2000.
//!
//! It exists so the rest of this crate can be developed and tested without the
//! car: run it on a vcan interface, or hand it one end of a
//! [`crate::link::LoopbackLink`] in a test.
//!
//! It is a stand-in, not a model of the real ECU. Because the real Coding-II
//! byte sequence is not known, the simulator starts a
//! forced regeneration when it sees the value 21295 in a coding write or in
//! any other service carrying it.

use std::time::Duration;

use crate::Result;
use crate::kwp2000 as kwp;
use crate::link::CanLink;
use crate::tp20::{
    BROADCAST_ADDR, OP_CHANNEL_TEST, OP_DISCONNECT, OP_PARAM_REQ, OP_PARAM_RESP, OP_SETUP_OK,
    OP_SETUP_REQ, hex_bytes,
};

pub const REGEN_CODE: u32 = 21295;
pub const PART_NUMBER: &[u8] = b"03G906016AB";
pub const SW_VERSION: &[u8] = b"0003";
pub const SYSTEM_DESC: &[u8] = b"R4 2.0L EDC  G000SG";
pub const SEED: u32 = 0x1A2B_3C4D;

/// Encode a physical value back into (formula, A, B), the inverse of
/// [`crate::blocks`] for the formulas the simulator uses.
fn cell(formula: u8, value: f64) -> [u8; 3] {
    let (a, b) = match formula {
        0x01 => factor((value * 5.0).round() as i64, 255), // rpm = A*B/5
        0x05 => {
            let (a, x) = factor((value * 10.0).round() as i64, 155); // C = A*(B-100)*0.1
            (a, x + 100)
        }
        0x7E => factor((value * 10.0).round() as i64, 255), // g = A*B*0.1
        0x36 | 0x08 => {
            let raw = value as i64;
            (((raw >> 8) & 0xFF) as u8, (raw & 0xFF) as u8)
        }
        other => panic!("the simulator cannot encode formula {other:#04x}"),
    };
    [formula, a, b]
}

/// Split a value into A*B with both bytes in range, as close as possible.
fn factor(raw: i64, b_max: i64) -> (u8, u8) {
    if raw <= 0 {
        return (0, 1);
    }
    let mut best = (i64::MAX, 1i64, 1i64);
    for a in 1..=255i64 {
        let b = ((raw as f64 / a as f64).round() as i64).clamp(0, b_max);
        let error = (a * b - raw).abs();
        if error < best.0 {
            best = (error, a, b);
            if error == 0 {
                break;
            }
        }
    }
    (best.1 as u8, best.2 as u8)
}

pub struct SimEcu<L: CanLink> {
    pub link: L,
    pub module: u8,
    pub debug: bool,

    tx_addr: u16, // ECU -> tester
    rx_addr: u16, // tester -> ECU
    open: bool,
    tx_seq: u8,

    // simulated vehicle state
    pub coolant: f64,
    pub rpm: f64,
    pub load_g: f64,
    pub egt_pre_turbo: f64,
    pub egt_pre_dpf: f64,
    pub egt_post_dpf: f64,
    pub forced_regen: bool,
    pub regen_counter: i64,
    pub dtcs: Vec<(u16, u8)>,
    pub logged_in: bool,
    pub basic_setting_group: Option<u8>,
    pub coding: u32,
    pub fault_mileage: u32,
}

impl<L: CanLink> SimEcu<L> {
    pub fn new(link: L) -> Self {
        SimEcu {
            link,
            module: 0x01,
            debug: false,
            tx_addr: 0x300,
            rx_addr: 0x740,
            open: false,
            tx_seq: 0,
            coolant: 85.0,
            rpm: 2100.0,
            load_g: 28.0,
            egt_pre_turbo: 320.0,
            egt_pre_dpf: 290.0,
            egt_post_dpf: 270.0,
            forced_regen: false,
            regen_counter: 0,
            // 0x41D7 = VAG 16855 = P0471 exhaust pressure sensor
            // range/performance (intermittent); 0x4802 = VAG 18434 = P2002
            // particulate trap efficiency below threshold (static).
            dtcs: vec![(0x41D7, 0x88), (0x4802, 0x02)],
            logged_in: false,
            basic_setting_group: None,
            coding: 11,
            fault_mileage: 184_233,
        }
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    // -- physics-ish ------------------------------------------------------
    fn tick(&mut self) {
        if self.forced_regen {
            self.regen_counter += 1;
            self.egt_pre_turbo = (self.egt_pre_turbo + 25.0).min(740.0);
            self.egt_pre_dpf = (self.egt_pre_dpf + 22.0).min(660.0);
            self.egt_post_dpf = (self.egt_post_dpf + 18.0).min(600.0);
            self.load_g = (self.load_g - 0.3).max(0.0);
            if self.load_g <= 0.5 {
                self.forced_regen = false;
            }
        } else {
            self.egt_pre_turbo = (self.egt_pre_turbo - 5.0).max(300.0);
            self.egt_pre_dpf = (self.egt_pre_dpf - 5.0).max(280.0);
            self.egt_post_dpf = (self.egt_post_dpf - 5.0).max(260.0);
        }
    }

    // -- KWP2000 ----------------------------------------------------------
    fn handle_kwp(&mut self, req: &[u8]) -> Vec<u8> {
        let sid = req[0];

        match sid {
            kwp::SID_START_DIAGNOSTIC_SESSION if req.len() > 1 => {
                return vec![0x50, req[1]];
            }
            kwp::SID_READ_ECU_IDENTIFICATION if req.len() > 1 => {
                if req[1] == kwp::IDENT_PART_NUMBER {
                    let mut out = vec![0x5A, req[1]];
                    out.extend_from_slice(&self.identification());
                    return out;
                }
                let mut out = vec![0x5A, req[1]];
                out.extend_from_slice(PART_NUMBER);
                return out;
            }
            kwp::SID_READ_DATA_BY_LOCAL_ID if req.len() > 1 => {
                if req[1] == kwp::LID_FREEZE_FRAME {
                    let mut out = vec![0x61, kwp::LID_FREEZE_FRAME];
                    out.extend_from_slice(&self.freeze_frame());
                    return out;
                }
                return match self.group_data(req[1]) {
                    Some(body) => {
                        let mut out = vec![0x61, req[1]];
                        out.extend_from_slice(&body);
                        out
                    }
                    None => vec![0x7F, sid, 0x31],
                };
            }
            kwp::SID_READ_DTC_BY_STATUS => {
                let mut out = vec![0x58, self.dtcs.len() as u8];
                for (code, status) in &self.dtcs {
                    out.extend_from_slice(&code.to_be_bytes());
                    out.push(*status);
                }
                return out;
            }
            kwp::SID_CLEAR_DIAGNOSTIC_INFORMATION => {
                self.dtcs.clear();
                return vec![0x54];
            }
            kwp::SID_TESTER_PRESENT => return vec![0x7E, 0x01],
            kwp::SID_SECURITY_ACCESS => return self.handle_security_access(req),
            kwp::SID_START_ROUTINE_BY_LOCAL_ID | kwp::SID_STOP_ROUTINE_BY_LOCAL_ID => {
                return self.handle_routine(req);
            }
            _ => {}
        }

        // A coding write: 3B 9A ... 03 <coding:3>
        if sid == kwp::SID_WRITE_DATA_BY_LOCAL_ID
            && req.len() > 5
            && req[1] == kwp::LID_CODING
            && req[req.len() - 4] == 0x03
        {
            let tail = &req[req.len() - 3..];
            self.coding = u32::from_be_bytes([0, tail[0], tail[1], tail[2]]);
            if self.coding == REGEN_CODE {
                self.forced_regen = true;
                self.regen_counter = 0;
            }
            if self.debug {
                println!("sim: coding written: {}", self.coding);
            }
            return vec![0x7B, req[1]];
        }

        // Stand-in for Coding-II: any service carrying the code starts a regen.
        if req[1..]
            .windows(2)
            .any(|w| w == (REGEN_CODE as u16).to_be_bytes())
        {
            self.forced_regen = true;
            self.regen_counter = 0;
            if self.debug {
                println!("sim: forced regeneration enabled");
            }
            return vec![sid.wrapping_add(0x40), req[1]];
        }

        vec![0x7F, sid, 0x11]
    }

    /// VAG seed and key: the key is just code + seed.
    fn handle_security_access(&mut self, req: &[u8]) -> Vec<u8> {
        let sid = req[0];
        if req.len() < 2 {
            return vec![0x7F, sid, 0x13];
        }
        match req[1] {
            kwp::ACCESS_REQUEST_SEED => {
                let mut out = vec![0x67, kwp::ACCESS_REQUEST_SEED];
                out.extend_from_slice(&SEED.to_be_bytes());
                out
            }
            kwp::ACCESS_SEND_KEY => {
                if req.len() != 6 {
                    return vec![0x7F, sid, 0x13];
                }
                let key = u32::from_be_bytes([req[2], req[3], req[4], req[5]]);
                if key == REGEN_CODE.wrapping_add(SEED) {
                    self.logged_in = true;
                    if self.debug {
                        println!("sim: login accepted");
                    }
                    vec![0x67, kwp::ACCESS_SEND_KEY]
                } else {
                    // 0x35: access authorization failed, incorrect Login code
                    vec![0x7F, sid, 0x35]
                }
            }
            _ => vec![0x7F, sid, 0x12],
        }
    }

    /// Basic settings: 31 BA / 31 B9 to run, 32 B8 to stop.
    fn handle_routine(&mut self, req: &[u8]) -> Vec<u8> {
        let sid = req[0];
        if req.len() != 4 {
            return vec![0x7F, sid, 0x13];
        }
        let (routine, group) = (req[1], req[3]);

        if sid == kwp::SID_STOP_ROUTINE_BY_LOCAL_ID {
            if routine != kwp::ROUTINE_CLOSE {
                return vec![0x7F, sid, 0x12];
            }
            self.basic_setting_group = None;
            self.forced_regen = false;
            return vec![0x72, routine];
        }

        if routine != kwp::ROUTINE_OPEN && routine != kwp::ROUTINE_RUN {
            return vec![0x7F, sid, 0x12];
        }
        if !self.logged_in {
            // 0x33: access authorization required, perform Login
            return vec![0x7F, sid, 0x33];
        }
        if self.coolant < 50.0 {
            // 0x87: temperature too low
            return vec![0x7F, sid, 0x87];
        }

        if routine == kwp::ROUTINE_OPEN {
            self.basic_setting_group = Some(group);
        } else {
            if self.basic_setting_group != Some(group) {
                // 0x24: incorrect sequence
                return vec![0x7F, sid, 0x24];
            }
            self.forced_regen = true;
            self.regen_counter = 0;
            if self.debug {
                println!("sim: basic setting {group} running, forced regeneration on");
            }
        }
        vec![0x71, routine]
    }

    /// The 1A 9B layout: part number, software version, coding, system.
    fn identification(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut field = |bytes: &[u8], width: usize| {
            let mut padded = bytes.to_vec();
            padded.resize(width, b' ');
            out.extend_from_slice(&padded[..width]);
        };
        field(PART_NUMBER, 12);
        field(SW_VERSION, 4);
        out.push(0x03);
        out.extend_from_slice(&self.coding.to_be_bytes()[1..]);
        out.extend_from_slice(&hex::decode("059738DE").unwrap());
        out.extend_from_slice(&12345u16.to_be_bytes());
        let mut system = SYSTEM_DESC.to_vec();
        system.resize(20, b' ');
        out.extend_from_slice(&system);
        out
    }

    /// Fault environment data for the most recent fault.
    fn freeze_frame(&self) -> Vec<u8> {
        let packed: u64 = (25 << 26) | (10 << 22) | (14 << 17) | (9 << 12) | (35 << 6) | 12;
        let mut out = vec![0x41, 0xD7, 0x02, 0x05, 0x28];
        out.extend_from_slice(&self.fault_mileage.to_be_bytes()[1..]);
        out.push(0x00);
        out.extend_from_slice(&packed.to_be_bytes()[3..]);
        out
    }

    fn group_data(&mut self, group: u8) -> Option<Vec<u8>> {
        self.tick();
        let mut body = Vec::with_capacity(12);
        match group {
            2 => {
                body.extend_from_slice(&cell(0x01, self.rpm));
                body.extend_from_slice(&cell(0x08, 0.0));
                body.extend_from_slice(&cell(0x08, 0.0));
                body.extend_from_slice(&cell(0x05, self.coolant));
            }
            70 => {
                let bits = if self.forced_regen { 0x02 } else { 0x00 };
                body.extend_from_slice(&[0x08, 0x00, bits]);
                body.extend_from_slice(&[0x08, 0x00, 0x01]);
                body.extend_from_slice(&cell(0x36, self.regen_counter as f64));
                body.extend_from_slice(&[0x08, 0x00, 0x00]);
            }
            75 => {
                body.extend_from_slice(&cell(0x05, self.egt_pre_turbo));
                body.extend_from_slice(&cell(0x05, self.egt_pre_dpf));
                body.extend_from_slice(&cell(0x7E, self.load_g));
                body.extend_from_slice(&cell(0x05, self.egt_post_dpf));
            }
            _ => return None,
        }
        Some(body)
    }

    // -- TP2.0 ------------------------------------------------------------
    async fn put(&mut self, addr: u16, data: &[u8]) -> Result<()> {
        if self.debug {
            println!("sim TX {addr:03x}  {}", hex_bytes(data));
        }
        self.link.send(addr, data).await
    }

    async fn send_message(&mut self, payload: &[u8]) -> Result<()> {
        let mut body = Vec::with_capacity(payload.len() + 2);
        body.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        body.extend_from_slice(payload);

        let frames = body.len().div_ceil(7);
        for (i, chunk) in body.chunks(7).enumerate() {
            let header = if i + 1 == frames { 0x10 } else { 0x20 } | self.tx_seq;
            let mut frame = Vec::with_capacity(8);
            frame.push(header);
            frame.extend_from_slice(chunk);
            let addr = self.tx_addr;
            self.put(addr, &frame).await?;
            self.tx_seq = (self.tx_seq + 1) & 0x0F;
        }
        Ok(())
    }

    /// Serve until the link goes away, or `idle_timeout` passes with no frames.
    pub async fn run(&mut self, idle_timeout: Option<Duration>) -> Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        let mut idle = Duration::ZERO;

        loop {
            let frame = self.link.recv(Duration::from_millis(100)).await?;
            let Some(frame) = frame else {
                if let Some(limit) = idle_timeout {
                    idle += Duration::from_millis(100);
                    if idle >= limit {
                        return Ok(());
                    }
                }
                continue;
            };
            idle = Duration::ZERO;
            if frame.data.is_empty() {
                continue;
            }
            if self.debug {
                println!("sim RX {:03x}  {}", frame.id, hex_bytes(&frame.data));
            }

            if frame.id == BROADCAST_ADDR {
                let data = &frame.data;
                if data.len() >= 7 && data[0] == self.module && data[1] == OP_SETUP_REQ {
                    let reply = [
                        0x00,
                        OP_SETUP_OK,
                        (self.tx_addr & 0xFF) as u8,
                        (self.tx_addr >> 8) as u8,
                        (self.rx_addr & 0xFF) as u8,
                        (self.rx_addr >> 8) as u8,
                        0x01,
                    ];
                    let addr = BROADCAST_ADDR + u16::from(self.module);
                    self.put(addr, &reply).await?;
                    self.open = true;
                    self.tx_seq = 0;
                    buf.clear();
                    // A new channel is a new session: the login does not carry over.
                    self.logged_in = false;
                    self.basic_setting_group = None;
                }
                continue;
            }

            if frame.id != self.rx_addr || !self.open {
                continue;
            }

            let op = frame.data[0];
            if op == OP_PARAM_REQ || op == OP_CHANNEL_TEST {
                let addr = self.tx_addr;
                self.put(addr, &[OP_PARAM_RESP, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF])
                    .await?;
                continue;
            }
            if op == OP_DISCONNECT {
                self.open = false;
                continue;
            }
            if op & 0xF0 == 0xB0 || op == OP_PARAM_RESP {
                continue;
            }

            let kind = op >> 4;
            let seq = op & 0x0F;
            if kind > 0x3 {
                continue;
            }
            buf.extend_from_slice(&frame.data[1..]);
            if kind == 0x0 || kind == 0x1 {
                let addr = self.tx_addr;
                self.put(addr, &[0xB0 | ((seq + 1) & 0x0F)]).await?;
            }
            if kind == 0x1 || kind == 0x3 {
                if buf.len() >= 2 {
                    let length = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
                    if buf.len() >= 2 + length && length > 0 {
                        let req = buf[2..2 + length].to_vec();
                        let response = self.handle_kwp(&req);
                        self.send_message(&response).await?;
                    }
                }
                buf.clear();
            }
        }
    }
}
