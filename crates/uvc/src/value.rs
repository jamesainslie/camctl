//! Control values and their wire encoding.

use crate::control::{Shape, Width};

/// Arc-seconds per degree, the unit of UVC pan, tilt and roll.
pub const ARC_SECONDS_PER_DEGREE: f64 = 3600.0;

/// A control value, decoded from or encoded to its wire bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// Scalars, switches (0 or 1), enums and the auto-exposure mode bitmap.
    Int(i64),
    /// Absolute pan and tilt in arc-seconds.
    PanTilt { pan: i32, tilt: i32 },
    /// Relative pan/tilt: direction (-1, 0, 1) and speed for each axis.
    PanTiltSpeed {
        pan: i8,
        pan_speed: u8,
        tilt: i8,
        tilt_speed: u8,
    },
    /// Relative zoom: direction (-1, 0, 1), digital zoom on or off, speed.
    ZoomSpeed {
        direction: i8,
        digital: bool,
        speed: u8,
    },
    /// Relative focus or roll: direction (-1, 0, 1) and speed.
    DirectionSpeed { direction: i8, speed: u8 },
    /// White balance component: blue and red.
    WhiteBalance { blue: u16, red: u16 },
}

/// A value that does not fit its control, or wire bytes that do not.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    #[error("value {value:?} does not match a {shape:?} control")]
    WrongShape { value: Value, shape: Shape },
    #[error("{value} does not fit a {}-byte {} field", .width.bytes(), if *.signed { "signed" } else { "unsigned" })]
    OutOfRange {
        value: i64,
        width: Width,
        signed: bool,
    },
    #[error("{0} is not a valid switch value (0 or 1)")]
    NotASwitch(i64),
    #[error("{0:#b} is not a single auto-exposure mode")]
    NotOneMode(i64),
    #[error("{0} degrees is not a usable angle")]
    BadDegrees(String),
    #[error("expected {expected} bytes, got {got}")]
    Short { expected: u16, got: usize },
}

impl Value {
    /// Absolute pan and tilt from degrees, rounded to whole arc-seconds.
    /// Non-finite or out-of-range angles are refused.
    pub fn pan_tilt_from_degrees(pan: f64, tilt: f64) -> Result<Value, ValueError> {
        Ok(Value::PanTilt {
            pan: degrees_to_arc_seconds(pan)?,
            tilt: degrees_to_arc_seconds(tilt)?,
        })
    }

    /// Pan and tilt in degrees, for a [`Value::PanTilt`].
    #[must_use]
    pub fn pan_tilt_degrees(&self) -> Option<(f64, f64)> {
        match *self {
            Value::PanTilt { pan, tilt } => Some((
                f64::from(pan) / ARC_SECONDS_PER_DEGREE,
                f64::from(tilt) / ARC_SECONDS_PER_DEGREE,
            )),
            _ => None,
        }
    }

    /// The integer of a [`Value::Int`].
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match *self {
            Value::Int(v) => Some(v),
            _ => None,
        }
    }

    /// Decode wire bytes for a control of `shape`. Extra bytes are ignored.
    pub fn decode(shape: Shape, bytes: &[u8]) -> Result<Value, ValueError> {
        let b = bytes
            .get(..usize::from(shape.size()))
            .ok_or(ValueError::Short {
                expected: shape.size(),
                got: bytes.len(),
            })?;
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let i32_at = |i: usize| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(match shape {
            Shape::Scalar { width, signed } => Value::Int(match (width, signed) {
                (Width::One, false) => i64::from(b[0]),
                (Width::One, true) => i64::from(b[0].cast_signed()),
                (Width::Two, false) => i64::from(u16_at(0)),
                (Width::Two, true) => i64::from(u16_at(0).cast_signed()),
                (Width::Four, false) => i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                (Width::Four, true) => i64::from(i32_at(0)),
            }),
            Shape::Bool | Shape::Enum | Shape::Bitmap => Value::Int(i64::from(b[0])),
            Shape::PanTilt => Value::PanTilt {
                pan: i32_at(0),
                tilt: i32_at(4),
            },
            Shape::PanTiltRelative => Value::PanTiltSpeed {
                pan: b[0].cast_signed(),
                pan_speed: b[1],
                tilt: b[2].cast_signed(),
                tilt_speed: b[3],
            },
            Shape::ZoomRelative => Value::ZoomSpeed {
                direction: b[0].cast_signed(),
                digital: b[1] != 0,
                speed: b[2],
            },
            Shape::DirectionSpeed => Value::DirectionSpeed {
                direction: b[0].cast_signed(),
                speed: b[1],
            },
            Shape::WhiteBalanceComponent => Value::WhiteBalance {
                blue: u16_at(0),
                red: u16_at(2),
            },
        })
    }

    /// Encode for a control of `shape`, checking that the value fits.
    pub fn encode(&self, shape: Shape) -> Result<Vec<u8>, ValueError> {
        match (shape, *self) {
            (Shape::Scalar { width, signed }, Value::Int(v)) => encode_int(v, width, signed),
            (Shape::Bool, Value::Int(v)) => match v {
                0 | 1 => encode_int(v, Width::One, false),
                _ => Err(ValueError::NotASwitch(v)),
            },
            (Shape::Bitmap, Value::Int(v)) => {
                let bytes = encode_int(v, Width::One, false)?;
                if bytes[0].is_power_of_two() {
                    Ok(bytes)
                } else {
                    Err(ValueError::NotOneMode(v))
                }
            }
            (Shape::Enum, Value::Int(v)) => encode_int(v, Width::One, false),
            (Shape::PanTilt, Value::PanTilt { pan, tilt }) => {
                let mut out = pan.to_le_bytes().to_vec();
                out.extend_from_slice(&tilt.to_le_bytes());
                Ok(out)
            }
            (
                Shape::PanTiltRelative,
                Value::PanTiltSpeed {
                    pan,
                    pan_speed,
                    tilt,
                    tilt_speed,
                },
            ) => Ok(vec![
                pan.cast_unsigned(),
                pan_speed,
                tilt.cast_unsigned(),
                tilt_speed,
            ]),
            (
                Shape::ZoomRelative,
                Value::ZoomSpeed {
                    direction,
                    digital,
                    speed,
                },
            ) => Ok(vec![direction.cast_unsigned(), u8::from(digital), speed]),
            (Shape::DirectionSpeed, Value::DirectionSpeed { direction, speed }) => {
                Ok(vec![direction.cast_unsigned(), speed])
            }
            (Shape::WhiteBalanceComponent, Value::WhiteBalance { blue, red }) => {
                let mut out = blue.to_le_bytes().to_vec();
                out.extend_from_slice(&red.to_le_bytes());
                Ok(out)
            }
            _ => Err(ValueError::WrongShape {
                value: *self,
                shape,
            }),
        }
    }
}

fn degrees_to_arc_seconds(degrees: f64) -> Result<i32, ValueError> {
    let arc = (degrees * ARC_SECONDS_PER_DEGREE).round();
    if !arc.is_finite() || arc < f64::from(i32::MIN) || arc > f64::from(i32::MAX) {
        return Err(ValueError::BadDegrees(degrees.to_string()));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "arc is a whole number inside i32's range"
    )]
    Ok(arc as i32)
}

fn encode_int(v: i64, width: Width, signed: bool) -> Result<Vec<u8>, ValueError> {
    let out_of_range = || ValueError::OutOfRange {
        value: v,
        width,
        signed,
    };
    Ok(match (width, signed) {
        (Width::One, false) => u8::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
        (Width::One, true) => i8::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
        (Width::Two, false) => u16::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
        (Width::Two, true) => i16::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
        (Width::Four, false) => u32::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
        (Width::Four, true) => i32::try_from(v)
            .map_err(|_| out_of_range())?
            .to_le_bytes()
            .to_vec(),
    })
}
