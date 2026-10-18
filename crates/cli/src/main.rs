//! `camctl`: list, inspect and drive UVC camera controls from the shell.

mod values;

use anyhow::{Context, Result, bail};
use camctl_core::motion::{Limits, sleep_aim};
use camctl_uvc::{Camera, Control, ControlRange, FormatKind, Shape, Transport, Value};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "camctl",
    version,
    about = "Control UVC cameras: zoom, pan, tilt, exposure and more"
)]
struct Cli {
    /// Camera to use, by index from `camctl list` (default: the first).
    #[arg(short, long, global = true, default_value_t = 0)]
    camera: usize,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List connected UVC cameras.
    List,
    /// Show every supported control with its value and range.
    Controls,
    /// Show the streaming formats and frame rates.
    Formats,
    /// Read one control.
    Get { control: String },
    /// Set one control, for example `set zoom 40` or `set pan-tilt -20,5`.
    /// Relative moves (`*-move`) keep going until you send a zero direction,
    /// for example `set pan-tilt-move 0,0,0,0`. Values are the camera's own:
    /// some cameras (OBSBOT) turn left for a positive pan.
    Set {
        control: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Point the camera: degrees right and up from centre, whichever way the
    /// camera's own pan axis runs.
    Point {
        #[arg(allow_negative_numbers = true)]
        pan: f64,
        #[arg(allow_negative_numbers = true)]
        tilt: f64,
    },
    /// Centre the camera and zoom all the way out.
    Home,
    /// Put the camera to sleep: pan centred and the lens tilted down as far
    /// as the camera obeys, facing the desk. Zoom is left alone. A Tiny 2
    /// Lite then enters its own sleep, and stays there until an app opens
    /// its video.
    Sleep,
    /// Wake the camera: pan and tilt back to centre, zoom left alone. A
    /// Tiny 2 Lite in its own sleep ignores this. It wakes when an app opens
    /// its video.
    Wake,
}

fn main() -> Result<()> {
    // Exit quietly when the reader goes away (`camctl controls | head`)
    // instead of panicking on a broken pipe.
    // SAFETY: restoring the default disposition of SIGPIPE at startup, before
    // any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Command::List = cli.command {
        return list();
    }
    let cameras = camctl_usb::list_cameras()?;
    let info = cameras
        .get(cli.camera)
        .with_context(|| format!("no camera {} (found {})", cli.camera, cameras.len()))?;
    let cam = info.open()?;
    match cli.command {
        Command::List => unreachable!("handled above"),
        Command::Controls => {
            controls(&cam);
            Ok(())
        }
        Command::Formats => {
            formats(&cam);
            Ok(())
        }
        Command::Get { control } => {
            let c = control_named(&control)?;
            println!("{}", values::format(c, &cam.get(c)?));
            Ok(())
        }
        Command::Set { control, value } => {
            let c = control_named(&control)?;
            cam.set(c, values::parse(c, &value)?)?;
            // The write succeeded; a read-back that fails (relative controls
            // often cannot be read) is worth a note, not a failure.
            match cam.get(c) {
                Ok(v) => println!("{} = {}", c.key(), values::format(c, &v)),
                Err(e) => eprintln!("{}: set (read-back unavailable: {e:#})", c.key()),
            }
            Ok(())
        }
        Command::Point { pan, tilt } => {
            let sign = camctl_core::quirks::pan_sign(info.vendor_id);
            cam.set(
                Control::PanTiltAbsolute,
                Value::pan_tilt_from_degrees(pan * sign, tilt)?,
            )?;
            Ok(())
        }
        Command::Home => {
            let t = cam.topology();
            let (aims, zooms) = (
                t.supports(Control::PanTiltAbsolute),
                t.supports(Control::ZoomAbsolute),
            );
            if !aims && !zooms {
                bail!("this camera can neither aim nor zoom");
            }
            if aims {
                cam.set(Control::PanTiltAbsolute, Value::PanTilt { pan: 0, tilt: 0 })?;
            }
            if zooms {
                let r = cam.range(Control::ZoomAbsolute)?;
                if let Some(wide) = r.min.or(r.default) {
                    cam.set(Control::ZoomAbsolute, wide)?;
                }
            }
            Ok(())
        }
        Command::Sleep => {
            if !cam.topology().supports(Control::PanTiltAbsolute) {
                bail!("this camera cannot aim, so it cannot point down to sleep");
            }
            let range = cam.range(Control::PanTiltAbsolute)?;
            let (Some(lo), Some(hi)) = (
                range.min.and_then(|v| v.pan_tilt_degrees()),
                range.max.and_then(|v| v.pan_tilt_degrees()),
            ) else {
                bail!("the camera does not report how far it can pan and tilt");
            };
            let (Some(pan), Some(tilt)) = (
                Limits::from_range(lo.0, hi.0),
                Limits::from_range(lo.1, hi.1),
            ) else {
                bail!("the camera reports an unusable pan and tilt range");
            };
            let quirk = camctl_core::quirks::sleep_tilt(info.vendor_id, info.product_id);
            let (pan, tilt) = sleep_aim(pan, tilt, quirk);
            cam.set(
                Control::PanTiltAbsolute,
                Value::pan_tilt_from_degrees(pan, tilt)?,
            )?;
            Ok(())
        }
        Command::Wake => {
            cam.set(Control::PanTiltAbsolute, Value::PanTilt { pan: 0, tilt: 0 })?;
            Ok(())
        }
    }
}

fn control_named(key: &str) -> Result<Control> {
    match Control::from_key(key) {
        Some(c) => Ok(c),
        None => bail!("unknown control {key}; run `camctl controls` for the list"),
    }
}

fn list() -> Result<()> {
    let cameras = camctl_usb::list_cameras()?;
    if cameras.is_empty() {
        println!("no UVC cameras found");
    }
    for (i, c) in cameras.iter().enumerate() {
        println!(
            "{i}: {} ({:04x}:{:04x}, location 0x{:08x})",
            c.name(),
            c.vendor_id,
            c.product_id,
            c.location_id
        );
    }
    Ok(())
}

fn controls<T: Transport>(cam: &Camera<T>) {
    for c in cam.topology().supported_controls() {
        let current = cam.get(c).map_or_else(
            |e| format!("({:#})", anyhow::Error::from(e)),
            |v| values::format(c, &v),
        );
        let range = cam
            .range(c)
            .map(|r| describe_range(c, &r))
            .unwrap_or_default();
        println!("{:<30} {current}{range}", c.key());
    }
}

/// "[min..max step s, default d]", leaving out what the device does not
/// report and anything meaningless for the control's shape.
fn describe_range(c: Control, r: &ControlRange) -> String {
    if matches!(c.spec().shape, Shape::Bool) {
        return r
            .default
            .map(|d| format!("  [default {}]", values::format(c, &d)))
            .unwrap_or_default();
    }
    if c == Control::AeMode {
        return r
            .step
            .map(|s| format!("  [modes {}]", values::format(c, &s)))
            .unwrap_or_default();
    }
    let f = |v: Option<Value>| v.map(|v| values::format(c, &v));
    let mut parts = Vec::new();
    if let (Some(min), Some(max)) = (f(r.min), f(r.max)) {
        parts.push(format!("{min}..{max}"));
    }
    if let Some(step) = f(r.step).filter(|_| matches!(r.step, Some(Value::Int(_)))) {
        parts.push(format!("step {step}"));
    }
    if let Some(default) = f(r.default) {
        parts.push(format!("default {default}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  [{}]", parts.join(", "))
    }
}

fn formats<T: Transport>(cam: &Camera<T>) {
    for f in &cam.topology().formats {
        let name = match f.kind {
            FormatKind::Mjpeg => "MJPEG".to_owned(),
            FormatKind::Uncompressed(cc) | FormatKind::FrameBased(cc) => {
                String::from_utf8_lossy(&cc).into_owned()
            }
        };
        for fr in &f.frames {
            let fps: Vec<String> = fr.fps().iter().map(|v| format!("{v}")).collect();
            println!(
                "{name:<6} {}x{} @ {} fps",
                fr.width,
                fr.height,
                fps.join(", ")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_values_parse_as_values_not_flags() {
        assert!(Cli::try_parse_from(["camctl", "set", "pan-tilt", "-20,5"]).is_ok());
        assert!(Cli::try_parse_from(["camctl", "set", "brightness", "-5"]).is_ok());
        assert!(Cli::try_parse_from(["camctl", "point", "-20", "-5"]).is_ok());
    }

    #[test]
    fn the_command_line_definition_is_valid() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }

    #[test]
    fn sleep_and_wake_are_commands() {
        let cli = Cli::try_parse_from(["camctl", "sleep"]).unwrap();
        assert!(matches!(cli.command, Command::Sleep));
        let cli = Cli::try_parse_from(["camctl", "-c", "1", "wake"]).unwrap();
        assert!(matches!(cli.command, Command::Wake));
    }

    #[test]
    fn flags_still_work() {
        let cli = Cli::try_parse_from(["camctl", "-c", "1", "controls"]).unwrap();
        assert_eq!(cli.camera, 1);
    }
}
