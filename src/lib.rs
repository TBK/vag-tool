//! KWP2000-over-TP2.0 diagnostics for VAG PQ35 cars.
//!
//! Written for an Audi A3 8P 2.0 TDI PD (engine code BMN, Siemens PPD1.x) and
//! a raw USB-CAN adapter such as the 8devices Korlan. The CAN layer comes from
//! the [`automotive`] crate, so the same code reaches a SocketCAN interface
//! today and a panda, Vector or J2534 device later; everything above it - VW
//! TP2.0 and VAG's KWP2000 dialect - lives here.

pub mod blocks;
pub mod dpf;
pub mod faults;
pub mod kwp2000;
pub mod link;
pub mod logdecode;
pub mod modules;
pub mod simecu;
pub mod tp20;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot reach the CAN interface: {0}")]
    Can(String),

    #[error("timed out {0}")]
    Timeout(String),

    #[error("TP2.0: {0}")]
    Tp20(String),

    /// The ECU answered `7F <sid> <code>`. The reason uses VAG's own wording
    /// where it has some, which is far more specific than the ISO table.
    #[error("{service} rejected: {reason}")]
    NegativeResponse {
        service: String,
        sid: u8,
        code: u8,
        reason: String,
    },

    #[error("unexpected response: {0}")]
    Unexpected(String),

    #[error("{0}")]
    Value(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn negative(sid: u8, code: u8) -> Self {
        Error::NegativeResponse {
            service: kwp2000::service_name(sid),
            sid,
            code,
            reason: kwp2000::negative_response_text(code),
        }
    }

    /// True when the ECU refused because nobody has logged in yet.
    pub fn needs_login(&self) -> bool {
        matches!(self, Error::NegativeResponse { code: 0x33, .. })
    }
}
