#!/usr/bin/env bash
# bundle.sh: build camctl.app and sign it.
#
# macOS grants camera access per signed app bundle, so the app must run
# from a bundle with NSCameraUsageDescription. Signing with a stable
# identity (an Apple Development certificate) keeps that grant across
# rebuilds; an ad-hoc signature would ask again after every build.
#
# usage: scripts/bundle.sh [release|debug]
#   CAMCTL_JOBS            cargo parallelism (default 3; unset on CI means
#                          all cores)
#   CAMCTL_BUNDLE_DIR      where to put camctl.app (default dist)
#   CAMCTL_SIGN_IDENTITY   codesign identity (default "Apple Development")
#                          "-"             ad-hoc (CI builds without secrets)
#                          "Developer ID…" hardened runtime with the camera
#                                          entitlement, for notarization
set -euo pipefail

profile="${1:-release}"
if [[ -n "${CI:-}" && -z "${CAMCTL_JOBS:-}" ]]; then
  jobs_args=()
else
  jobs_args=(-j "${CAMCTL_JOBS:-3}")
fi
identity="${CAMCTL_SIGN_IDENTITY:-Apple Development}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

build_args=(build ${jobs_args[@]+"${jobs_args[@]}"} --locked -p camctl-app --message-format=json-render-diagnostics)
case "$profile" in
  release) build_args+=(--release) ;;
  debug) ;;
  *) echo "bundle.sh: profile must be release or debug" >&2; exit 2 ;;
esac

# Take the binary's path from cargo's own report, so CARGO_TARGET_DIR,
# build.target-dir and --target all lead to the file just built.
artifacts="$(cargo "${build_args[@]}")"
bin="$(sed -n 's/.*"executable":"\([^"]*\/camctl-app\)".*/\1/p' <<<"$artifacts" | tail -n 1)"
if [[ -z "$bin" ]]; then
  echo "bundle.sh: cargo reported no camctl-app executable" >&2
  exit 1
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="${CAMCTL_BUNDLE_DIR:-dist}/camctl.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/camctl"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>us.ainslies.camctl</string>
  <key>CFBundleName</key><string>camctl</string>
  <key>CFBundleDisplayName</key><string>camctl</string>
  <key>CFBundleExecutable</key><string>camctl</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key><string>camctl shows a live preview from your camera while you adjust it.</string>
</dict>
</plist>
PLIST

sign_args=(--force --sign "$identity")
case "$identity" in
  "Developer ID"*)
    # Notarization needs the hardened runtime and a secure timestamp; the
    # hardened runtime needs camera access declared.
    sign_args+=(--options runtime --timestamp --entitlements scripts/camctl.entitlements)
    ;;
  *)
    sign_args+=(--timestamp=none)
    ;;
esac
codesign "${sign_args[@]}" "$app"
codesign --verify --strict "$app"
echo "$app"
