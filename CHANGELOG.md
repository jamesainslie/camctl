# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the version is 0.x, a breaking change bumps the minor version and
anything else bumps the patch; see [docs/RELEASING.md](docs/RELEASING.md).

## [Unreleased]

## [0.1.2] - 2026-10-22

### Added

- Choosing the camera. A **Camera** submenu in the menu bar menu lists
  the connected cameras, checks the one the preview shows, and switches
  the preview and its controls to another. The choice is remembered
  across launches; when the chosen camera is not connected, camctl falls
  back to the first USB camera.
- One camctl at a time. A second launch, of another copy or a fresh
  build, adds no menu bar icon: it asks the running camctl to show its
  preview and quits. The lock is
  `~/Library/Application Support/camctl/instance.lock`.
- **Reconnect Camera** in the menu bar menu closes the camera and opens it
  afresh.

### Changed

- The repository is public. `brew install --cask yaklabco/tap/camctl`
  needs no GitHub token, and the release workflow pushes to the tap
  with a deploy key scoped to it.
- Releases are Developer ID signed and notarized, so Gatekeeper opens a
  downloaded camctl and the camera permission survives updates. The
  release workflow refuses to publish without the signing and notary
  secrets, or an app that `scripts/verify-signed.sh` rejects. The cask no
  longer clears the download quarantine. Local releases can notarize with
  a keychain profile, `CAMCTL_NOTARY_PROFILE`.
- Quit stops the camera, then quits, and leaves within about 2 s even if
  the camera does not stop.
- The event log records why a request failed (for example
  `Zoom: SET_CUR failed: device disconnected`), cameras connected and
  disconnected, the Mac waking, and control reconnects.

### Fixed

- A camera that was unplugged, or that re-enumerated while the Mac slept,
  stayed lost until camctl restarted. The preview now follows the cameras
  and reopens on the chosen one, else the first USB camera, else the
  built-in camera. The shown camera gets about 4 s to come back before
  camctl falls back to another. A reopen camctl makes by itself does not
  bring the window forward or turn the camera on while the window is
  hidden, and a sleeping camera stays asleep.
- Pan, tilt and zoom stopped working when the USB control link died, after
  sleep or a re-enumeration. camctl now drops and reopens the link, with
  waits from 0.5 s doubling to 5 s and up to 8 tries, and also after every
  Mac wake. The bar says "reconnecting to the camera's controls…"
  meanwhile, and "controls lost · choose Reconnect Camera in the menu" if
  it gives up.
- Pending control writes are sent in the order of their latest request, so
  turning auto focus off and then setting a manual focus works within one
  batch.
- A camera that answers GET_RES or GET_DEF with a short or empty packet
  keeps the limits it did report.

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

[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/jamesainslie/camctl/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/jamesainslie/camctl/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0
