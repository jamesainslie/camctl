# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the version is 0.x, a breaking change bumps the minor version and
anything else bumps the patch; see [docs/RELEASING.md](docs/RELEASING.md).

## [Unreleased]

### Added

- Choosing the camera. A **Camera** submenu in the menu bar menu lists
  the connected cameras, checks the one the preview shows, and switches
  the preview and its controls to another. The choice is remembered
  across launches; when the chosen camera is not connected, camctl falls
  back to the first USB camera.

## [0.1.1] - 2026-10-18

### Added

- Camera sleep. A **Sleep** button in the preview's bar, and **Sleep
  Camera** in the menu bar menu, turn the lens down and stop the preview.
  Zoom is left alone. **Wake** aims the camera back and resumes. On a
  Tiny 2 Lite this enters the camera's own sleep mode through the
  standard pan-tilt control.
- `camctl sleep` and `camctl wake` CLI commands: lens down with pan
  centred, and pan and tilt back to centre.
- Per-camera quirk: the Tiny 2 Lite's sleep tilt of -80 degrees, since it
  ignores tilts below about -84.
- Homebrew: a `camctl` cask (the app) and a `camctl-cli` formula in
  yaklabco/homebrew-tap, written by `cargo xtask homebrew` and pushed by
  the release workflow when `HOMEBREW_TAP_TOKEN` is set.

## [0.1.0] - 2026-10-17

### Added

- Menu bar app (`camctl.app`) with a custom template icon whose lens fills
  while the camera streams. Left click shows or hides the preview; right
  click opens a menu.
- Floating, always-on-top live preview window that draws camera frames
  zero-copy through GPUI's surface element, with a dark interface built on
  gpui-component.
- Smooth pan, tilt and zoom:
  - Hold arrow keys (Shift for fine moves, `+` and `-` to zoom).
  - Drag on the video like a joystick, or hold the buttons of an on-video
    pad.
  - Click a point to aim the camera at it, scroll to zoom, and press `H`
    to go home.
- Streaming only while the preview can be seen and the user is present.
  Hiding, covering or minimizing the window, the screen saver, and the
  lock screen all stop the camera and turn its light off.
- A stream liveness watchdog that restarts or rebuilds a stalled capture
  session and says plainly when the camera sends no video.
- Stream format policy that prefers raw YUY2 for small previews (no
  decoding), uses the smallest MJPEG mode that covers larger windows, caps
  previews at 30 fps, and never changes the format of a camera another app
  is using.
- Position sync every 2 seconds while idle, so the camera's own tracking
  does not throw off aiming.
- An event log of state changes at `~/Library/Logs/camctl/camctl.log`.
- `camctl` CLI:
  - `list`, `controls`, `formats`, `get`, `set`, `point` and `home`,
    covering every standard UVC 1.1 camera terminal and processing unit
    control the camera advertises.
  - `point` takes degrees right and up from centre on every camera.
- `camctl-uvc` library: UVC descriptor parsing, the control catalogue,
  checked wire encoding, and a transport-agnostic `Camera` API, tested
  against a real OBSBOT Tiny 2 Lite descriptor.
- Per-camera quirks: the OBSBOT pan direction and the Tiny 2 Lite's field
  of view for click-to-aim.
- `cargo xtask` task runner, CI on GitHub Actions, and changelog-driven
  releases that publish the signed app and the CLI.

[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/jamesainslie/camctl/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0
