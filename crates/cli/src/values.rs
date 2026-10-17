//! Parse control values from the command line and format them for display.

use anyhow::{Context, Result, bail};
use camctl_uvc::{Control, Shape, Value};

/// Parse `text` as a value for `control`.
///
/// Integers for scalar controls; `on`/`off` for switches; `off`, `50`, `60`
/// or `auto` for anti-flicker; `PAN,TILT` in degrees for pan-tilt; comma
/// lists for the relative and component controls.
pub fn parse(control: Control, text: &str) -> Result<Value> {
    let shape = control.spec().shape;
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let int =
        |s: &str| -> Result<i64> { s.parse().with_context(|| format!("not an integer: {s}")) };
    let small =
        |s: &str| -> Result<i8> { s.parse().with_context(|| format!("not -128..127: {s}")) };
    let byte = |s: &str| -> Result<u8> { s.parse().with_context(|| format!("not 0..255: {s}")) };
    let want = |n: usize| -> Result<()> {
        if parts.len() == n {
            Ok(())
        } else {
            bail!("{} takes {n} comma-separated values", control.key())
        }
    };
    Ok(match shape {
        Shape::Bool => Value::Int(match text {
            "on" | "true" | "1" => 1,
            "off" | "false" | "0" => 0,
            _ => bail!("{} is on or off", control.key()),
        }),
        Shape::Enum if control == Control::PowerLineFrequency => Value::Int(match text {
            "off" | "0" => 0,
            "50" => 1,
            "60" => 2,
            "auto" => 3,
            _ => bail!("anti-flicker is off, 50, 60 or auto"),
        }),
        Shape::Bitmap if control == Control::AeMode => Value::Int(match text {
            "manual" => 1,
            "auto" => 2,
            "shutter" => 4,
            "aperture" => 8,
            other => int(other)?,
        }),
        Shape::Scalar { .. } | Shape::Enum | Shape::Bitmap => {
            want(1)?;
            Value::Int(int(parts[0])?)
        }
        Shape::PanTilt => {
            want(2)?;
            let deg =
                |s: &str| -> Result<f64> { s.parse().with_context(|| format!("not degrees: {s}")) };
            Value::pan_tilt_from_degrees(deg(parts[0])?, deg(parts[1])?)?
        }
        Shape::PanTiltRelative => {
            want(4)?;
            Value::PanTiltSpeed {
                pan: small(parts[0])?,
                pan_speed: byte(parts[1])?,
                tilt: small(parts[2])?,
                tilt_speed: byte(parts[3])?,
            }
        }
        Shape::ZoomRelative => {
            want(2)?;
            Value::ZoomSpeed {
                direction: small(parts[0])?,
                digital: false,
                speed: byte(parts[1])?,
            }
        }
        Shape::DirectionSpeed => {
            want(2)?;
            Value::DirectionSpeed {
                direction: small(parts[0])?,
                speed: byte(parts[1])?,
            }
        }
        Shape::WhiteBalanceComponent => {
            want(2)?;
            let n = |s: &str| -> Result<u16> {
                s.parse().with_context(|| format!("not 0..65535: {s}"))
            };
            Value::WhiteBalance {
                blue: n(parts[0])?,
                red: n(parts[1])?,
            }
        }
    })
}

/// Human-readable form of `value` for `control`.
pub fn format(control: Control, value: &Value) -> String {
    match (control.spec().shape, value) {
        (Shape::Bool, Value::Int(v)) => if *v == 0 { "off" } else { "on" }.to_owned(),
        (Shape::Enum, Value::Int(v)) if control == Control::PowerLineFrequency => match v {
            0 => "off".to_owned(),
            1 => "50 Hz".to_owned(),
            2 => "60 Hz".to_owned(),
            3 => "auto".to_owned(),
            other => format!("{other}"),
        },
        (Shape::Bitmap, Value::Int(v)) if control == Control::AeMode => ae_modes(*v),
        (_, Value::PanTilt { .. }) => {
            let (pan, tilt) = value.pan_tilt_degrees().unwrap_or_default();
            format!("pan {pan:.1}°, tilt {tilt:.1}°")
        }
        (_, Value::Int(v)) => v.to_string(),
        (_, other) => format!("{other:?}"),
    }
}

/// Auto-exposure mode bits (UVC 1.1 `CT_AE_MODE_CONTROL`).
fn ae_modes(bits: i64) -> String {
    let names = [
        (1, "manual"),
        (2, "auto"),
        (4, "shutter priority"),
        (8, "aperture priority"),
    ];
    let set: Vec<&str> = names
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if set.is_empty() {
        format!("{bits:#b}")
    } else {
        set.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_for_scalars() {
        assert_eq!(parse(Control::ZoomAbsolute, "40").unwrap(), Value::Int(40));
        assert_eq!(parse(Control::Brightness, "-5").unwrap(), Value::Int(-5));
        assert!(parse(Control::ZoomAbsolute, "forty").is_err());
    }

    #[test]
    fn switches_and_anti_flicker() {
        assert_eq!(parse(Control::FocusAuto, "on").unwrap(), Value::Int(1));
        assert_eq!(parse(Control::FocusAuto, "off").unwrap(), Value::Int(0));
        assert!(parse(Control::FocusAuto, "maybe").is_err());
        assert_eq!(
            parse(Control::PowerLineFrequency, "60").unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            parse(Control::PowerLineFrequency, "auto").unwrap(),
            Value::Int(3)
        );
    }

    #[test]
    fn pan_tilt_in_degrees() {
        assert_eq!(
            parse(Control::PanTiltAbsolute, "-20, 5.5").unwrap(),
            Value::PanTilt {
                pan: -72_000,
                tilt: 19_800
            }
        );
        assert!(parse(Control::PanTiltAbsolute, "10").is_err());
    }

    #[test]
    fn relative_moves() {
        assert_eq!(
            parse(Control::PanTiltRelative, "1,5,-1,3").unwrap(),
            Value::PanTiltSpeed {
                pan: 1,
                pan_speed: 5,
                tilt: -1,
                tilt_speed: 3
            }
        );
        assert_eq!(
            parse(Control::ZoomRelative, "-1,2").unwrap(),
            Value::ZoomSpeed {
                direction: -1,
                digital: false,
                speed: 2
            }
        );
    }

    #[test]
    fn exposure_modes_by_name() {
        assert_eq!(parse(Control::AeMode, "manual").unwrap(), Value::Int(1));
        assert_eq!(parse(Control::AeMode, "aperture").unwrap(), Value::Int(8));
        assert_eq!(
            format(Control::AeMode, &Value::Int(0b1011)),
            "manual, auto, aperture priority"
        );
    }

    #[test]
    fn formatting() {
        assert_eq!(
            format(
                Control::PanTiltAbsolute,
                &Value::PanTilt {
                    pan: 32400,
                    tilt: -302_400
                }
            ),
            "pan 9.0°, tilt -84.0°"
        );
        assert_eq!(format(Control::FocusAuto, &Value::Int(1)), "on");
        assert_eq!(format(Control::PowerLineFrequency, &Value::Int(2)), "60 Hz");
        assert_eq!(format(Control::ZoomAbsolute, &Value::Int(40)), "40");
    }
}
