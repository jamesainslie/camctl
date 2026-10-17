//! UVC class requests and the transport that carries them.

use crate::control::ControlAddress;

/// UVC class-specific request codes (bRequest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RequestCode {
    SetCur = 0x01,
    GetCur = 0x81,
    GetMin = 0x82,
    GetMax = 0x83,
    GetRes = 0x84,
    GetLen = 0x85,
    GetInfo = 0x86,
    GetDef = 0x87,
}

impl From<RequestCode> for u8 {
    fn from(code: RequestCode) -> u8 {
        code as u8
    }
}

impl std::fmt::Display for RequestCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RequestCode::SetCur => "SET_CUR",
            RequestCode::GetCur => "GET_CUR",
            RequestCode::GetMin => "GET_MIN",
            RequestCode::GetMax => "GET_MAX",
            RequestCode::GetRes => "GET_RES",
            RequestCode::GetLen => "GET_LEN",
            RequestCode::GetInfo => "GET_INFO",
            RequestCode::GetDef => "GET_DEF",
        })
    }
}

impl RequestCode {
    /// Whether data flows from the host to the device.
    #[must_use]
    pub const fn is_out(self) -> bool {
        matches!(self, RequestCode::SetCur)
    }
}

/// One UVC control transfer on the default pipe. UVC unit requests are
/// always class requests to an interface, so only the code, value, index
/// and length vary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub code: RequestCode,
    /// Control selector in the high byte.
    pub value: u16,
    /// Unit id in the high byte, interface number in the low byte.
    pub index: u16,
    pub length: u16,
}

impl Request {
    /// A request for `code` against the control at `address`.
    #[must_use]
    pub fn new(code: RequestCode, address: ControlAddress, length: u16) -> Request {
        Request {
            code,
            value: u16::from(address.selector) << 8,
            index: (u16::from(address.unit_id) << 8) | u16::from(address.interface),
            length,
        }
    }

    /// The `bmRequestType` byte: class, interface recipient, and the
    /// direction implied by the code.
    #[must_use]
    pub const fn request_type(&self) -> u8 {
        if self.code.is_out() { 0x21 } else { 0xa1 }
    }
}

/// Why a control transfer failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The device stalled the request: usually unsupported, or not
    /// allowed in the current mode (manual focus while auto focus is on).
    #[error("request stalled by the device")]
    Stall,
    #[error("request timed out")]
    Timeout,
    #[error("device disconnected")]
    Disconnected,
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// Carries control transfers to one device. Implementations do the I/O;
/// everything above them is pure.
pub trait Transport {
    /// Device-to-host transfer of up to `req.length` bytes.
    fn control_in(&self, req: &Request) -> Result<Vec<u8>, TransportError>;
    /// Host-to-device transfer of `data`, which is `req.length` bytes.
    fn control_out(&self, req: &Request, data: &[u8]) -> Result<(), TransportError>;
}
