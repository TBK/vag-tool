//! KWP2000 (ISO 14230-3) as VAG speaks it over TP2.0.
//!
//! Services needed for diagnostics on a PD/PPD engine ECU, plus a raw escape
//! hatch so a sequence captured from VCDS can be replayed verbatim.

use std::time::Duration;

use crate::link::CanLink;
use crate::tp20::{Tp20Channel, hex_bytes};
use crate::{Error, Result};

// services
pub const SID_START_DIAGNOSTIC_SESSION: u8 = 0x10;
pub const SID_CLEAR_DIAGNOSTIC_INFORMATION: u8 = 0x14;
pub const SID_READ_DTC_BY_STATUS: u8 = 0x18;
pub const SID_READ_ECU_IDENTIFICATION: u8 = 0x1A;
pub const SID_STOP_DIAGNOSTIC_SESSION: u8 = 0x20;
pub const SID_READ_DATA_BY_LOCAL_ID: u8 = 0x21;
pub const SID_SECURITY_ACCESS: u8 = 0x27;
pub const SID_INPUT_OUTPUT_CONTROL_BY_LOCAL_ID: u8 = 0x30;
pub const SID_START_ROUTINE_BY_LOCAL_ID: u8 = 0x31;
pub const SID_STOP_ROUTINE_BY_LOCAL_ID: u8 = 0x32;
pub const SID_WRITE_DATA_BY_LOCAL_ID: u8 = 0x3B;
pub const SID_TESTER_PRESENT: u8 = 0x3E;

/// Session ids. 0x89/0x86/0x84/0x83/0x81;
/// 0x85 is from pq-flasher.
pub const SESSION_DIAGNOSTIC: u8 = 0x89;
pub const SESSION_ENGINEERING: u8 = 0x86;
pub const SESSION_PROGRAMMING: u8 = 0x85;
pub const SESSION_EOL_ECU: u8 = 0x84;
pub const SESSION_EOL_VW: u8 = 0x83;
pub const SESSION_OBD: u8 = 0x81;

pub const IDENT_PART_NUMBER: u8 = 0x9B;
pub const IDENT_SHORT: u8 = 0x91;
pub const IDENT_MODULE_LIST: u8 = 0x9F;

/// Security access (VCDS function 16): request seed, then send key.
pub const ACCESS_REQUEST_SEED: u8 = 0x03;
pub const ACCESS_SEND_KEY: u8 = 0x04;

/// Routine identifiers behind basic settings and adaptation.
pub const ROUTINE_OPEN: u8 = 0xBA;
pub const ROUTINE_RUN: u8 = 0xB9;
pub const ROUTINE_CLOSE: u8 = 0xB8;

/// Second byte of those routines.
pub const MODE_BASIC_SETTING: u8 = 0x00;
pub const MODE_ADAPTATION: u8 = 0x01;
pub const ADAPTATION_SIMPLE: u8 = 0x03; // 2-byte channel value
pub const ADAPTATION_LONG: u8 = 0x0A;

/// Coding identifier, for both writeDataByLocalIdentifier and
/// readEcuIdentification.
pub const LID_CODING: u8 = 0x9A;
/// Local identifier carrying fault environment data (freeze frame).
pub const LID_FREEZE_FRAME: u8 = 0x6C;

pub fn service_name(sid: u8) -> String {
    let name = match sid {
        0x10 => "startDiagnosticSession",
        0x11 => "ecuReset",
        0x14 => "clearDiagnosticInformation",
        0x17 => "readStatusOfDTC",
        0x18 => "readDTCByStatus",
        0x1A => "readEcuIdentification",
        0x20 => "stopDiagnosticSession",
        0x21 => "readDataByLocalIdentifier",
        0x22 => "readDataByCommonIdentifier",
        0x23 => "readMemoryByAddress",
        0x27 => "securityAccess",
        0x28 => "disableNormalMessageTransmission",
        0x29 => "enableNormalMessageTransmission",
        0x2C => "dynamicallyDefineLocalId",
        0x2E => "writeDataByCommonIdentifier",
        0x30 => "inputOutputControlByLocalId",
        0x31 => "startRoutineByLocalIdentifier",
        0x32 => "stopRoutineByLocalIdentifier",
        0x33 => "requestRoutineResultsByLocalId",
        0x34 => "requestDownload",
        0x35 => "requestUpload",
        0x36 => "transferData",
        0x37 => "requestTransferExit",
        0x3B => "writeDataByLocalIdentifier",
        0x3D => "writeMemoryByAddress",
        0x3E => "testerPresent",
        0x82 => "stopCommunication",
        _ => return format!("service {sid:#04x}"),
    };
    name.to_string()
}

/// Services known to only read. Anything else either changes the ECU or is a
/// VAG-specific service nobody here has identified - both worth flagging.
pub fn changes_ecu(sid: u8) -> bool {
    !matches!(
        sid,
        0x10 | 0x12 | 0x13 | 0x17 | 0x18 | 0x1A | 0x20 | 0x21 | 0x22 | 0x23 | 0x33 | 0x3A | 0x3E
    )
}

/// VAG's own wording for the codes its KWP2000 ECUs return. Much more specific
/// than the ISO 14230 table, and several are exactly what a refused
/// regeneration looks like.
pub fn vag_negative_response_text(code: u8) -> Option<&'static str> {
    Some(match code {
        0x10 => "function cancelled, disruption in control module",
        0x11 | 0x12 | 0x31 => "function not available",
        0x13 => "function cannot be performed, message length or format incorrect",
        0x22 => "function cancelled, marginal conditions have not been met",
        0x24 => "procedure cannot be performed, incorrect sequence",
        0x25 => "function cancelled, subsystem does not respond",
        0x26 => "function cancelled, entry in DTC memory",
        0x33 => "access authorization required, perform Login",
        0x35 => "access authorization failed, incorrect Login code",
        0x36 => "login not possible, number of access attempts exceeded",
        0x37 => "login not possible, lock time not yet elapsed",
        0x70 => "function cancelled, upload/download not possible",
        0x71 => "function cancelled, data transfer error",
        0x72 => "function cancelled, delete/programming unsuccessful",
        0x73 => "function cancelled, incorrect data block sequence",
        0x78 => "function started, control module busy",
        0x7E => "function cannot be performed, incorrect diagnostic mode",
        0x81 => "function cancelled, engine RPM too high",
        0x82 => "function cancelled, engine RPM too low",
        0x83 => "function cancelled, engine running",
        0x84 => "function cancelled, engine not running",
        0x85 => "function cancelled, engine not running long enough",
        0x86 => "function cancelled, temperature too high",
        0x87 => "function cancelled, temperature too low",
        0x88 => "function cancelled, vehicle speed too high",
        0x89 => "function cancelled, vehicle speed too low",
        0x8A => "function cancelled, accelerator pedal position exceeded",
        0x8B => "function cancelled, accelerator pedal position not reached",
        0x8C => "function cancelled, selector lever not in position 'N'",
        0x8D => "function cancelled, driving mode/gear not selected",
        0x8F => "function cancelled, brake pedal not depressed",
        0x90 => "function cancelled, selector lever not in position 'P'",
        0x91 => "function cancelled, converter clutch locked",
        0x92 => "function cancelled, supply voltage too high",
        0x93 => "function cancelled, supply voltage too low",
        _ => return None,
    })
}

fn iso_negative_response_text(code: u8) -> Option<&'static str> {
    Some(match code {
        0x10 => "generalReject",
        0x11 => "serviceNotSupported",
        0x12 => "subFunctionNotSupported / invalidFormat",
        0x21 => "busy - repeat request",
        0x22 => "conditionsNotCorrect or requestSequenceError",
        0x23 => "routineNotComplete",
        0x31 => "requestOutOfRange",
        0x33 => "securityAccessDenied",
        0x35 => "invalidKey",
        0x36 => "exceedNumberOfAttempts",
        0x37 => "requiredTimeDelayNotExpired",
        0x78 => "requestCorrectlyReceived - responsePending",
        _ => return None,
    })
}

pub fn negative_response_text(code: u8) -> String {
    vag_negative_response_text(code)
        .or_else(|| iso_negative_response_text(code))
        .map(str::to_string)
        .unwrap_or_else(|| format!("code {code:#04x}"))
}

pub struct Kwp2000Client<L: CanLink> {
    pub channel: Tp20Channel<L>,
    pub debug: bool,
}

impl<L: CanLink> Kwp2000Client<L> {
    pub fn new(channel: Tp20Channel<L>) -> Self {
        let debug = channel.debug;
        Kwp2000Client { channel, debug }
    }

    /// Send one request and return the response without its service id.
    pub async fn request(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        self.request_with_retries(payload, 2).await
    }

    pub async fn request_with_retries(&mut self, payload: &[u8], retries: u32) -> Result<Vec<u8>> {
        let sid = *payload
            .first()
            .ok_or_else(|| Error::Value("empty KWP2000 request".into()))?;
        let mut attempt = 0;

        loop {
            if self.debug {
                println!("KWP TX  {}", hex_bytes(payload));
            }
            self.channel.send(payload).await?;

            let resp = match self.receive_settled().await {
                Ok(resp) => resp,
                Err(Error::Timeout(what)) if attempt < retries => {
                    attempt += 1;
                    if self.debug {
                        println!("KWP timeout ({what}), retrying");
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };

            if self.debug {
                println!("KWP RX  {}", hex_bytes(&resp));
            }

            if resp[0] == 0x7F {
                let code = resp.get(2).copied().unwrap_or(0);
                // 0x21 is "busy, repeat request".
                if code == 0x21 && attempt < retries {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                return Err(Error::negative(resp.get(1).copied().unwrap_or(sid), code));
            }

            if resp[0] != sid.wrapping_add(0x40) {
                return Err(Error::Unexpected(format!(
                    "asked for {}, got {}",
                    service_name(sid),
                    hex_bytes(&resp)
                )));
            }
            return Ok(resp[1..].to_vec());
        }
    }

    /// Swallow `7F .. 78` (response pending) replies.
    async fn receive_settled(&mut self) -> Result<Vec<u8>> {
        for _ in 0..20 {
            let resp = self.channel.recv().await?;
            if resp.is_empty() {
                continue;
            }
            if resp[0] == 0x7F && resp.get(2) == Some(&0x78) {
                if self.debug {
                    println!("KWP RX  response pending");
                }
                continue;
            }
            return Ok(resp);
        }
        Err(Error::Unexpected(
            "the ECU kept answering 'response pending'".into(),
        ))
    }

    // -- services ---------------------------------------------------------
    pub async fn start_diagnostic_session(&mut self, session: u8) -> Result<Vec<u8>> {
        self.request(&[SID_START_DIAGNOSTIC_SESSION, session]).await
    }

    pub async fn stop_diagnostic_session(&mut self) -> Result<Vec<u8>> {
        self.request(&[SID_STOP_DIAGNOSTIC_SESSION]).await
    }

    pub async fn tester_present(&mut self) -> Result<Vec<u8>> {
        self.request(&[SID_TESTER_PRESENT, 0x01]).await
    }

    pub async fn read_ecu_identification(&mut self, ident: u8) -> Result<Vec<u8>> {
        self.request(&[SID_READ_ECU_IDENTIFICATION, ident]).await
    }

    /// Measuring block ("Messwerteblock") `group`, 1..255. Returns the echoed
    /// group number stripped, leaving four (formula, A, B) triples.
    pub async fn read_group(&mut self, group: u8) -> Result<Vec<u8>> {
        if group == 0 {
            return Err(Error::Value("group must be 1..255".into()));
        }
        let body = self.request(&[SID_READ_DATA_BY_LOCAL_ID, group]).await?;
        if body.first() != Some(&group) {
            return Err(Error::Unexpected(format!(
                "asked for group {group}, got {}",
                hex_bytes(&body)
            )));
        }
        Ok(body[1..].to_vec())
    }

    /// readDTCByStatus, "all identified faults". Returns (code, status) pairs.
    pub async fn read_dtcs(&mut self) -> Result<Vec<(u16, u8)>> {
        let body = self
            .request(&[SID_READ_DTC_BY_STATUS, 0x02, 0xFF, 0xFF])
            .await?;
        if body.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in body[1..].as_chunks::<3>().0 {
            let code = u16::from_be_bytes([entry[0], entry[1]]);
            if code != 0 {
                out.push((code, entry[2]));
            }
        }
        Ok(out)
    }

    pub async fn clear_dtcs(&mut self) -> Result<Vec<u8>> {
        self.request(&[SID_CLEAR_DIAGNOSTIC_INFORMATION, 0xFF, 0x00])
            .await
    }

    /// Fault environment data. The response layout is known (`21 6C` ->
    /// `61 6C ...`); that this is the request is inferred, see the docs.
    pub async fn read_freeze_frame(&mut self) -> Result<Vec<u8>> {
        self.request(&[SID_READ_DATA_BY_LOCAL_ID, LID_FREEZE_FRAME])
            .await
    }

    // -- security access (VCDS function 16) -------------------------------
    /// Log in with a numeric code. VAG's key is simply `code + seed`.
    /// Returns the seed and key, so a caller can show its work.
    pub async fn security_access(&mut self, code: u32) -> Result<(u32, u32)> {
        let body = self
            .request(&[SID_SECURITY_ACCESS, ACCESS_REQUEST_SEED])
            .await?;
        if body.len() < 5 || body[0] != ACCESS_REQUEST_SEED {
            return Err(Error::Unexpected(format!(
                "unexpected seed response: {}",
                hex_bytes(&body)
            )));
        }
        let seed = u32::from_be_bytes([body[1], body[2], body[3], body[4]]);
        let key = code.wrapping_add(seed);
        let mut request = vec![SID_SECURITY_ACCESS, ACCESS_SEND_KEY];
        request.extend_from_slice(&key.to_be_bytes());
        self.request(&request).await?;
        Ok((seed, key))
    }

    // -- the 31 BA / 31 B9 / 32 B8 family ---------------------------------
    // Three routines with the same shape: <routine> <mode> <id> [data].
    // mode 0x00 addresses a basic settings group, mode 0x01 an adaptation form
    // (3 = the plain 2-byte channel kind, 10 = long adaptation).
    pub async fn open_function(&mut self, mode: u8, ident: u8) -> Result<Vec<u8>> {
        self.request(&[SID_START_ROUTINE_BY_LOCAL_ID, ROUTINE_OPEN, mode, ident])
            .await
    }

    pub async fn run_function(&mut self, mode: u8, ident: u8, data: &[u8]) -> Result<Vec<u8>> {
        let mut request = vec![SID_START_ROUTINE_BY_LOCAL_ID, ROUTINE_RUN, mode, ident];
        request.extend_from_slice(data);
        self.request(&request).await
    }

    pub async fn close_function(&mut self, mode: u8, ident: u8) -> Result<Vec<u8>> {
        self.request(&[SID_STOP_ROUTINE_BY_LOCAL_ID, ROUTINE_CLOSE, mode, ident])
            .await
    }

    pub async fn prepare_basic_setting(&mut self, group: u8) -> Result<Vec<u8>> {
        self.open_function(MODE_BASIC_SETTING, group).await
    }

    pub async fn start_basic_setting(&mut self, group: u8) -> Result<Vec<u8>> {
        self.run_function(MODE_BASIC_SETTING, group, &[]).await
    }

    pub async fn stop_basic_setting(&mut self, group: u8) -> Result<Vec<u8>> {
        self.close_function(MODE_BASIC_SETTING, group).await
    }
}

/// Decoded `1A 9B` response.
#[derive(Clone, Debug, Default)]
pub struct Identification {
    pub part_number: String,
    pub sw_version: String,
    pub coding_length: Option<u8>,
    pub coding: Option<u32>,
    pub system_description: String,
    pub subsystems: usize,
    pub raw: Vec<u8>,
}

impl Identification {
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("part number         {}", self.part_number),
            format!("software version    {}", self.sw_version),
        ];
        if !self.system_description.is_empty() {
            out.push(format!("system              {}", self.system_description));
        }
        if let Some(length) = self.coding_length {
            let coding = match self.coding {
                Some(value) => format!("{value:07}"),
                None => "-".to_string(),
            };
            out.push(format!("coding              {coding} ({length} bytes)"));
        }
        if self.subsystems > 0 {
            out.push(format!("subsystems          {}", self.subsystems));
        }
        out
    }
}

/// VAG sets the top bit of the first character as a flag.
fn ascii_field(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    let mut cleaned = vec![data[0] & 0x7F];
    cleaned.extend_from_slice(&data[1..]);
    cleaned
        .into_iter()
        .map(|b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Decode the body of a `1A 9B` response (identifier byte still first):
///
/// ```text
/// <9B> <part number:12> <software version:4> <coding length:1>
///      <coding + workshop identification:9> <system description:20>
///      <subsystem blocks of 47 bytes...>
/// ```
pub fn parse_identification(body: &[u8]) -> Result<Identification> {
    if body.first() != Some(&IDENT_PART_NUMBER) {
        return Err(Error::Unexpected(format!(
            "not a 1A 9B response: {}",
            hex_bytes(&body[..body.len().min(4)])
        )));
    }
    let data = &body[1..];
    let mut ident = Identification {
        raw: data.to_vec(),
        ..Default::default()
    };
    if data.len() >= 12 {
        ident.part_number = ascii_field(&data[0..12]);
    }
    if data.len() >= 16 {
        ident.sw_version = ascii_field(&data[12..16]);
    }
    if data.len() > 16 {
        ident.coding_length = Some(data[16]);
        if data[16] == 3 && data.len() >= 20 {
            let coding = u32::from_be_bytes([0, data[17], data[18], data[19]]);
            ident.coding = Some(coding & 0x7F_FFFF);
        }
    }
    if data.len() >= 46 {
        ident.system_description = ascii_field(&data[26..46]);
    }
    if data.len() > 46 {
        ident.subsystems = (data.len() - 46) / 47;
    }
    Ok(ident)
}

/// Assemble a Coding-II write:
///
/// ```text
/// 3B 9A <prefix:4> <wsc:2> <software version as ASCII> 03 <code:3>
/// ```
///
/// `prefix` has no safe default: the four bytes after `9A` look like workshop
/// identification, nothing proves which value an ECU expects,
/// so the caller passes the ones seen in a capture from this ECU.
/// `sw_version` is the ECU's own software version, which the request embeds
/// - that is why a sequence captured from another car cannot be replayed as-is.
///
/// Sending the result writes to the ECU.
pub fn build_coding_ii_request(
    code: u32,
    sw_version: &str,
    prefix: &[u8],
    wsc: u16,
) -> Result<Vec<u8>> {
    if prefix.len() != 4 {
        return Err(Error::Value("prefix must be exactly 4 bytes".into()));
    }
    if code > 0x7F_FFFF {
        return Err(Error::Value("coding value must fit in 3 bytes".into()));
    }
    let mut request = vec![SID_WRITE_DATA_BY_LOCAL_ID, LID_CODING];
    request.extend_from_slice(prefix);
    request.extend_from_slice(&wsc.to_be_bytes());
    request.extend_from_slice(sw_version.as_bytes());
    request.push(0x03);
    request.extend_from_slice(&code.to_be_bytes()[1..]);
    Ok(request)
}
