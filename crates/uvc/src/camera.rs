//! Typed control access for one camera.

use crate::control::{Control, ControlAddress};
use crate::descriptor::Topology;
use crate::transport::{Request, RequestCode, Transport, TransportError};
use crate::value::{Value, ValueError};

/// Why a control operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CameraError {
    #[error("{0} is not supported by this camera")]
    Unsupported(Control),
    #[error("{control}: {code} failed")]
    Request {
        control: Control,
        code: RequestCode,
        #[source]
        source: TransportError,
    },
    #[error(transparent)]
    Value(#[from] ValueError),
}

/// A control's limits. Each part is `None` when the device does not
/// answer that request (UVC only requires `GET_MIN`/`GET_MAX` for some
/// controls; auto-exposure mode, for one, reports its modes in `GET_RES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlRange {
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub step: Option<Value>,
    pub default: Option<Value>,
}

/// A control's `GET_INFO` capability bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlInfo(u8);

impl ControlInfo {
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }
    /// D0: the control can be read.
    #[must_use]
    pub const fn can_get(self) -> bool {
        self.0 & 0x01 != 0
    }
    /// D1: the control can be written.
    #[must_use]
    pub const fn can_set(self) -> bool {
        self.0 & 0x02 != 0
    }
    /// D2: currently read-only because its auto mode is on.
    #[must_use]
    pub const fn disabled_by_auto(self) -> bool {
        self.0 & 0x04 != 0
    }
    /// D3: the device may change it by itself.
    #[must_use]
    pub const fn autoupdate(self) -> bool {
        self.0 & 0x08 != 0
    }
    /// D4: changes complete asynchronously.
    #[must_use]
    pub const fn asynchronous(self) -> bool {
        self.0 & 0x10 != 0
    }
}

/// One UVC camera: its topology plus a transport to reach it.
#[derive(Debug)]
pub struct Camera<T> {
    topology: Topology,
    transport: T,
}

impl<T: Transport> Camera<T> {
    #[must_use]
    pub fn new(topology: Topology, transport: T) -> Self {
        Camera {
            topology,
            transport,
        }
    }

    #[must_use]
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn address(&self, control: Control) -> Result<ControlAddress, CameraError> {
        self.topology
            .address(control)
            .ok_or(CameraError::Unsupported(control))
    }

    fn request_in(
        &self,
        control: Control,
        code: RequestCode,
        len: u16,
    ) -> Result<Vec<u8>, CameraError> {
        let req = Request::new(code, self.address(control)?, len);
        self.transport
            .control_in(&req)
            .map_err(|source| CameraError::Request {
                control,
                code,
                source,
            })
    }

    fn read(&self, control: Control, code: RequestCode) -> Result<Value, CameraError> {
        let shape = control.spec().shape;
        let bytes = self.request_in(control, code, shape.size())?;
        Ok(Value::decode(shape, &bytes)?)
    }

    /// Like [`Self::read`], but a stall (request not supported) is `None`.
    fn read_optional(
        &self,
        control: Control,
        code: RequestCode,
    ) -> Result<Option<Value>, CameraError> {
        match self.read(control, code) {
            Ok(v) => Ok(Some(v)),
            Err(CameraError::Request {
                source: TransportError::Stall,
                ..
            }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The control's current value.
    pub fn get(&self, control: Control) -> Result<Value, CameraError> {
        self.read(control, RequestCode::GetCur)
    }

    /// The control's minimum, maximum, step and default, as far as the
    /// device reports them.
    pub fn range(&self, control: Control) -> Result<ControlRange, CameraError> {
        Ok(ControlRange {
            min: self.read_optional(control, RequestCode::GetMin)?,
            max: self.read_optional(control, RequestCode::GetMax)?,
            step: self.read_optional(control, RequestCode::GetRes)?,
            default: self.read_optional(control, RequestCode::GetDef)?,
        })
    }

    /// The control's capability bits.
    pub fn info(&self, control: Control) -> Result<ControlInfo, CameraError> {
        let bytes = self.request_in(control, RequestCode::GetInfo, 1)?;
        let b = *bytes.first().ok_or(ValueError::Short {
            expected: 1,
            got: 0,
        })?;
        Ok(ControlInfo(b))
    }

    /// Set the control. The value is checked against the control's shape
    /// and width before anything is sent.
    pub fn set(&self, control: Control, value: Value) -> Result<(), CameraError> {
        let address = self.address(control)?;
        let shape = control.spec().shape;
        let data = value.encode(shape)?;
        let code = RequestCode::SetCur;
        let req = Request::new(code, address, shape.size());
        self.transport
            .control_out(&req, &data)
            .map_err(|source| CameraError::Request {
                control,
                code,
                source,
            })
    }
}
