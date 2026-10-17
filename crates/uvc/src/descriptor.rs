//! Parse a USB configuration descriptor into the UVC topology: the video
//! control units and their advertised controls, and the streaming formats.

use crate::control::{Control, ControlAddress, UnitKind};

const DT_INTERFACE: u8 = 0x04;
const CS_INTERFACE: u8 = 0x24;
const CC_VIDEO: u8 = 0x0e;
const SC_VIDEOCONTROL: u8 = 0x01;
const SC_VIDEOSTREAMING: u8 = 0x02;

const VC_HEADER: u8 = 0x01;
const VC_INPUT_TERMINAL: u8 = 0x02;
const VC_PROCESSING_UNIT: u8 = 0x05;
const VC_EXTENSION_UNIT: u8 = 0x06;
const ITT_CAMERA: u16 = 0x0201;

const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
const VS_FRAME_UNCOMPRESSED: u8 = 0x05;
const VS_FORMAT_MJPEG: u8 = 0x06;
const VS_FRAME_MJPEG: u8 = 0x07;
const VS_FORMAT_FRAME_BASED: u8 = 0x10;
const VS_FRAME_FRAME_BASED: u8 = 0x11;

/// Why a configuration descriptor could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    #[error("descriptor at byte {offset} needs {needed} bytes, {available} available")]
    Truncated {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("zero-length descriptor at byte {offset}")]
    ZeroLength { offset: usize },
    #[error("no UVC video control interface in this configuration")]
    NoVideoControl,
    #[error("the video control interface has no class-specific header")]
    MissingHeader,
}

/// A unit's `bmControls` capability bitmap.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlBitmap(Vec<u8>);

impl ControlBitmap {
    /// Whether bit `bit` (D0 is the low bit of the first byte) is set.
    #[must_use]
    pub fn has(&self, bit: u8) -> bool {
        let byte = usize::from(bit / 8);
        self.0.get(byte).is_some_and(|b| (b >> (bit % 8)) & 1 == 1)
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ControlBitmap {
    fn from(bytes: Vec<u8>) -> Self {
        ControlBitmap(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraTerminal {
    pub id: u8,
    pub controls: ControlBitmap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessingUnit {
    pub id: u8,
    pub source_id: u8,
    /// wMaxMultiplier: digital zoom limit times 100.
    pub max_multiplier: u16,
    pub controls: ControlBitmap,
}

/// A vendor extension unit. Listed for completeness, never driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionUnit {
    pub id: u8,
    pub guid: [u8; 16],
    pub num_controls: u8,
    pub controls: ControlBitmap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Mjpeg,
    /// Uncompressed, with the `FourCC` from the format GUID (e.g. `YUY2`).
    Uncompressed([u8; 4]),
    /// Frame-based (H.264, H.265), with its `FourCC`.
    FrameBased([u8; 4]),
}

/// Frame intervals in 100 ns units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameIntervals {
    Discrete(Vec<u32>),
    Continuous { min: u32, max: u32, step: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub index: u8,
    pub width: u16,
    pub height: u16,
    pub default_interval: u32,
    pub intervals: FrameIntervals,
}

impl Frame {
    /// Supported frame rates, fastest first, rounded to 0.01 fps. For a
    /// continuous range, the two ends.
    #[must_use]
    pub fn fps(&self) -> Vec<f64> {
        let ends;
        let raw: &[u32] = match &self.intervals {
            FrameIntervals::Discrete(v) => v,
            FrameIntervals::Continuous { min, max, .. } => {
                ends = [*min, *max];
                &ends
            }
        };
        let mut fps: Vec<f64> = raw
            .iter()
            .filter(|&&i| i > 0)
            .map(|&i| (1e9 / f64::from(i)).round() / 100.0)
            .collect();
        fps.sort_by(|a, b| b.total_cmp(a));
        fps.dedup();
        fps
    }

    #[must_use]
    pub fn max_fps(&self) -> Option<f64> {
        self.fps().first().copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    /// The streaming interface it belongs to (bFormatIndex is only unique
    /// within one interface).
    pub interface: u8,
    pub index: u8,
    pub kind: FormatKind,
    pub frames: Vec<Frame>,
}

/// Everything the configuration descriptor says about a UVC camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topology {
    /// bInterfaceNumber of the video control interface (wIndex low byte).
    pub control_interface: u8,
    /// bcdUVC, e.g. 0x0100 or 0x0110.
    pub uvc_version: u16,
    pub camera_terminal: Option<CameraTerminal>,
    pub processing_units: Vec<ProcessingUnit>,
    pub extension_units: Vec<ExtensionUnit>,
    pub formats: Vec<Format>,
}

impl Topology {
    /// Whether the camera advertises `control`.
    #[must_use]
    pub fn supports(&self, control: Control) -> bool {
        self.address(control).is_some()
    }

    /// Where to send requests for `control`, if the camera advertises it.
    #[must_use]
    pub fn address(&self, control: Control) -> Option<ControlAddress> {
        let spec = control.spec();
        let (unit_id, controls) = match spec.unit {
            UnitKind::CameraTerminal => {
                let ct = self.camera_terminal.as_ref()?;
                (ct.id, &ct.controls)
            }
            UnitKind::ProcessingUnit => {
                let pu = self.processing_units.first()?;
                (pu.id, &pu.controls)
            }
        };
        controls.has(spec.bit).then_some(ControlAddress {
            unit_id,
            interface: self.control_interface,
            selector: spec.selector,
        })
    }

    /// Every advertised standard control, in [`Control::ALL`] order.
    #[must_use]
    pub fn supported_controls(&self) -> Vec<Control> {
        Control::ALL
            .into_iter()
            .filter(|&c| self.supports(c))
            .collect()
    }
}

/// One descriptor's bytes, with bounds-checked field access.
struct Desc<'a> {
    offset: usize,
    bytes: &'a [u8],
}

impl Desc<'_> {
    fn need(&self, end: usize) -> Result<(), ParseError> {
        if end > self.bytes.len() {
            return Err(ParseError::Truncated {
                offset: self.offset,
                needed: end,
                available: self.bytes.len(),
            });
        }
        Ok(())
    }

    fn u8(&self, i: usize) -> Result<u8, ParseError> {
        self.need(i + 1)?;
        Ok(self.bytes[i])
    }

    fn u16(&self, i: usize) -> Result<u16, ParseError> {
        self.need(i + 2)?;
        Ok(u16::from_le_bytes([self.bytes[i], self.bytes[i + 1]]))
    }

    fn u32(&self, i: usize) -> Result<u32, ParseError> {
        self.need(i + 4)?;
        let mut b = [0; 4];
        b.copy_from_slice(&self.bytes[i..i + 4]);
        Ok(u32::from_le_bytes(b))
    }

    fn slice(&self, i: usize, n: usize) -> Result<&[u8], ParseError> {
        self.need(i + n)?;
        Ok(&self.bytes[i..i + n])
    }

    fn fourcc(&self, i: usize) -> Result<[u8; 4], ParseError> {
        let s = self.slice(i, 4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
}

/// Split a configuration descriptor into its descriptors.
fn descriptors(bytes: &[u8]) -> impl Iterator<Item = Result<Desc<'_>, ParseError>> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset >= bytes.len() {
            return None;
        }
        let len = usize::from(bytes[offset]);
        let item = if len == 0 {
            Err(ParseError::ZeroLength { offset })
        } else if offset + len > bytes.len() {
            Err(ParseError::Truncated {
                offset,
                needed: len,
                available: bytes.len() - offset,
            })
        } else {
            Ok(Desc {
                offset,
                bytes: &bytes[offset..offset + len],
            })
        };
        // Stop after an error so a bad length never loops or misaligns.
        offset = if item.is_ok() {
            offset + len
        } else {
            bytes.len()
        };
        Some(item)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Current {
    Other,
    Control,
    Streaming(u8),
}

const DT_CONFIGURATION: u8 = 0x02;

/// The configuration's declared wTotalLength, checked against `bytes`, so a
/// buffer cut on a descriptor boundary is still reported as truncated.
fn declared_bytes(bytes: &[u8]) -> Result<&[u8], ParseError> {
    if bytes.len() < 4 || bytes[1] != DT_CONFIGURATION {
        return Ok(bytes);
    }
    let total = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    if bytes.len() < total {
        return Err(ParseError::Truncated {
            offset: 0,
            needed: total,
            available: bytes.len(),
        });
    }
    Ok(&bytes[..total])
}

/// Parse a full configuration descriptor (as returned by `GET_DESCRIPTOR`).
///
/// Only the first video function is read: its video control interface and
/// the streaming interfaces its header lists. A second function (an IR
/// camera, say) and every other interface are skipped.
pub fn parse_configuration(bytes: &[u8]) -> Result<Topology, ParseError> {
    let bytes = declared_bytes(bytes)?;
    let mut control_interface: Option<u8> = None;
    let mut units = ControlUnits::default();
    let mut formats: Vec<Format> = Vec::new();
    let mut current = Current::Other;

    for d in descriptors(bytes) {
        let d = d?;
        let kind = d.u8(1)?;
        if kind == DT_INTERFACE {
            let number = d.u8(2)?;
            let class = d.u8(5)?;
            let subclass = d.u8(6)?;
            current = match (class, subclass) {
                (CC_VIDEO, SC_VIDEOCONTROL) if control_interface.is_none_or(|n| n == number) => {
                    control_interface = Some(number);
                    Current::Control
                }
                (CC_VIDEO, SC_VIDEOSTREAMING) if units.streaming.contains(&number) => {
                    Current::Streaming(number)
                }
                _ => Current::Other,
            };
            continue;
        }
        if kind != CS_INTERFACE || current == Current::Other {
            continue;
        }
        let subtype = d.u8(2)?;
        match current {
            Current::Control => read_control(&d, subtype, &mut units)?,
            Current::Streaming(interface) => {
                let kind = match subtype {
                    VS_FORMAT_MJPEG => Some(FormatKind::Mjpeg),
                    VS_FORMAT_UNCOMPRESSED => Some(FormatKind::Uncompressed(d.fourcc(5)?)),
                    VS_FORMAT_FRAME_BASED => Some(FormatKind::FrameBased(d.fourcc(5)?)),
                    _ => None,
                };
                if let Some(kind) = kind {
                    formats.push(Format {
                        interface,
                        index: d.u8(3)?,
                        kind,
                        frames: Vec::new(),
                    });
                } else if matches!(
                    subtype,
                    VS_FRAME_UNCOMPRESSED | VS_FRAME_MJPEG | VS_FRAME_FRAME_BASED
                ) {
                    let frame = parse_frame(&d, subtype == VS_FRAME_FRAME_BASED)?;
                    if let Some(format) = formats.last_mut() {
                        format.frames.push(frame);
                    }
                }
            }
            Current::Other => {}
        }
    }

    let control_interface = control_interface.ok_or(ParseError::NoVideoControl)?;
    Ok(Topology {
        control_interface,
        uvc_version: units.uvc_version.ok_or(ParseError::MissingHeader)?,
        camera_terminal: units.camera_terminal,
        processing_units: units.processing_units,
        extension_units: units.extension_units,
        formats,
    })
}

/// What the video control interface describes.
#[derive(Default)]
struct ControlUnits {
    uvc_version: Option<u16>,
    /// baInterfaceNr from the header: this function's streaming interfaces.
    streaming: Vec<u8>,
    camera_terminal: Option<CameraTerminal>,
    processing_units: Vec<ProcessingUnit>,
    extension_units: Vec<ExtensionUnit>,
}

fn read_control(d: &Desc<'_>, subtype: u8, units: &mut ControlUnits) -> Result<(), ParseError> {
    match subtype {
        VC_HEADER => {
            units.uvc_version = Some(d.u16(3)?);
            let count = usize::from(d.u8(11)?);
            units.streaming = d.slice(12, count)?.to_vec();
        }
        VC_INPUT_TERMINAL if units.camera_terminal.is_none() && d.u16(4)? == ITT_CAMERA => {
            let size = usize::from(d.u8(14)?);
            units.camera_terminal = Some(CameraTerminal {
                id: d.u8(3)?,
                controls: ControlBitmap(d.slice(15, size)?.to_vec()),
            });
        }
        VC_PROCESSING_UNIT => {
            let size = usize::from(d.u8(7)?);
            units.processing_units.push(ProcessingUnit {
                id: d.u8(3)?,
                source_id: d.u8(4)?,
                max_multiplier: d.u16(5)?,
                controls: ControlBitmap(d.slice(8, size)?.to_vec()),
            });
        }
        VC_EXTENSION_UNIT => {
            let mut guid = [0; 16];
            guid.copy_from_slice(d.slice(4, 16)?);
            let pins = usize::from(d.u8(21)?);
            let size = usize::from(d.u8(22 + pins)?);
            units.extension_units.push(ExtensionUnit {
                id: d.u8(3)?,
                guid,
                num_controls: d.u8(20)?,
                controls: ControlBitmap(d.slice(23 + pins, size)?.to_vec()),
            });
        }
        _ => {}
    }
    Ok(())
}

fn parse_frame(d: &Desc<'_>, frame_based: bool) -> Result<Frame, ParseError> {
    // Frame-based frames drop dwMaxVideoFrameBufferSize and add
    // dwBytesPerLine after the interval type; both layouts put the interval
    // table at byte 26.
    let (default_at, type_at) = if frame_based { (17, 21) } else { (21, 25) };
    let interval_type = d.u8(type_at)?;
    let intervals = if interval_type == 0 {
        FrameIntervals::Continuous {
            min: d.u32(26)?,
            max: d.u32(30)?,
            step: d.u32(34)?,
        }
    } else {
        FrameIntervals::Discrete(
            (0..usize::from(interval_type))
                .map(|k| d.u32(26 + 4 * k))
                .collect::<Result<_, _>>()?,
        )
    };
    Ok(Frame {
        index: d.u8(3)?,
        width: d.u16(5)?,
        height: d.u16(7)?,
        default_interval: d.u32(default_at)?,
        intervals,
    })
}
