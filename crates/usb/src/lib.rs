//! USB transport for [`camctl_uvc`] over `nusb` (IOKit on macOS).
//!
//! Control transfers go to the default pipe and need no exclusive access,
//! so they work while the system camera driver owns the device and other
//! apps are streaming from it.
//!
//! Note: on macOS nusb opens the device exclusively when it can, and keeps
//! that until the device is dropped. With a camera the system driver has
//! claimed, which is the usual case, the exclusive open fails and nusb
//! falls back to shared access. Keep opened cameras only as long as needed
//! all the same: a vendor firmware updater may want the device to itself.
//!
//! macOS only: device matching relies on IOKit location IDs.

use std::time::Duration;

use camctl_uvc::{Camera, Request, Topology, Transport, TransportError, parse_configuration};
use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};

const CC_VIDEO: u8 = 0x0e;
const SC_VIDEOCONTROL: u8 = 0x01;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// Why a camera could not be listed or opened.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UsbError {
    #[error("USB enumeration failed")]
    List(#[source] nusb::Error),
    #[error("could not open the camera")]
    Open(#[source] nusb::Error),
    #[error("could not read the active configuration")]
    Configuration(#[source] nusb::ActiveConfigurationError),
    #[error(transparent)]
    Parse(#[from] camctl_uvc::ParseError),
}

/// A UVC camera found on the bus, not yet opened.
#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    /// IOKit location ID: stable per port, distinguishes identical cameras.
    pub location_id: u32,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    info: nusb::DeviceInfo,
}

impl CameraInfo {
    /// Product name, or "VID:PID" when the device has none.
    #[must_use]
    pub fn name(&self) -> String {
        self.product
            .clone()
            .unwrap_or_else(|| format!("{:04x}:{:04x}", self.vendor_id, self.product_id))
    }

    /// Open the camera and parse its configuration descriptor.
    pub fn open(&self) -> Result<Camera<UsbTransport>, UsbError> {
        let device = self.info.open().wait().map_err(UsbError::Open)?;
        let topology = read_topology(&device)?;
        Ok(Camera::new(
            topology,
            UsbTransport {
                device,
                timeout: DEFAULT_TIMEOUT,
            },
        ))
    }
}

fn read_topology(device: &nusb::Device) -> Result<Topology, UsbError> {
    let config = device
        .active_configuration()
        .map_err(UsbError::Configuration)?;
    Ok(parse_configuration(config.as_bytes())?)
}

/// Every connected device with a UVC video control interface, in a
/// stable order (by USB location).
pub fn list_cameras() -> Result<Vec<CameraInfo>, UsbError> {
    let devices = nusb::list_devices().wait().map_err(UsbError::List)?;
    let mut cameras: Vec<CameraInfo> = devices
        .filter(|d| {
            d.interfaces()
                .any(|i| i.class() == CC_VIDEO && i.subclass() == SC_VIDEOCONTROL)
        })
        .map(|d| CameraInfo {
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            location_id: d.location_id(),
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
            serial: d.serial_number().map(str::to_owned),
            info: d,
        })
        .collect();
    cameras.sort_by_key(|c| c.location_id);
    Ok(cameras)
}

/// [`Transport`] over an opened `nusb` device.
pub struct UsbTransport {
    device: nusb::Device,
    timeout: Duration,
}

impl std::fmt::Debug for UsbTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbTransport")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

fn map_error(e: TransferError) -> TransportError {
    match e {
        TransferError::Stall => TransportError::Stall,
        TransferError::Cancelled => TransportError::Timeout,
        TransferError::Disconnected => TransportError::Disconnected,
        other => TransportError::Other(Box::new(other)),
    }
}

impl Transport for UsbTransport {
    fn control_in(&self, req: &Request) -> Result<Vec<u8>, TransportError> {
        self.device
            .control_in(
                ControlIn {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: req.code.into(),
                    value: req.value,
                    index: req.index,
                    length: req.length,
                },
                self.timeout,
            )
            .wait()
            .map_err(map_error)
    }

    fn control_out(&self, req: &Request, data: &[u8]) -> Result<(), TransportError> {
        // nusb takes the length from `data`; the two must agree.
        debug_assert_eq!(usize::from(req.length), data.len());
        self.device
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: req.code.into(),
                    value: req.value,
                    index: req.index,
                    data,
                },
                self.timeout,
            )
            .wait()
            .map_err(map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_timeout_and_disconnect_map_through() {
        assert!(matches!(
            map_error(TransferError::Stall),
            TransportError::Stall
        ));
        assert!(matches!(
            map_error(TransferError::Disconnected),
            TransportError::Disconnected
        ));
        assert!(matches!(
            map_error(TransferError::Cancelled),
            TransportError::Timeout
        ));
    }

    #[test]
    fn other_errors_keep_their_source() {
        let TransportError::Other(source) = map_error(TransferError::Fault) else {
            panic!("a fault is an Other error");
        };
        assert!(matches!(
            source.downcast_ref::<TransferError>(),
            Some(TransferError::Fault)
        ));
    }
}
