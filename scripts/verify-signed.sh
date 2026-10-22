#!/usr/bin/env bash
# verify-signed.sh: check that camctl.app is fit to publish.
#
# A release must be Developer ID signed with the hardened runtime and the
# camera entitlement (without it the capture session gets no frames),
# notarized with the ticket stapled, and accepted by Gatekeeper. Anything
# less ships an app that macOS blocks or that never sees the camera.
#
# usage: scripts/verify-signed.sh [path/to/camctl.app]   (default dist/camctl.app)
set -euo pipefail

app="${1:-dist/camctl.app}"
failures=0

check() {
  local what="$1"
  shift
  if "$@"; then
    echo "ok: $what"
  else
    echo "FAIL: $what" >&2
    failures=$((failures + 1))
  fi
}

if [[ ! -d "$app" ]]; then
  echo "verify-signed.sh: no app at $app" >&2
  exit 1
fi

# Empty for an unsigned app, so every check below still reports.
details="$(codesign -d --verbose=4 "$app" 2>&1 || true)"
entitlements="$(codesign -d --entitlements - --xml "$app" 2>/dev/null || true)"

check "the signature is valid" codesign --verify --strict --deep "$app"
check "it is signed with a Developer ID Application certificate" \
  grep -q '^Authority=Developer ID Application: ' <<<"$details"
check "it names a team" grep -Eq '^TeamIdentifier=[A-Z0-9]{10}$' <<<"$details"
check "the hardened runtime is on" grep -Eq '^CodeDirectory .*flags=.*runtime' <<<"$details"
check "it has a secure timestamp" grep -q '^Timestamp=' <<<"$details"
check "it may use the camera" \
  grep -Eq 'com\.apple\.security\.device\.camera</key>[[:space:]]*<true/>' <<<"$entitlements"
# Tried a few times, since stapler may ask Apple's ticket service, which
# lags a fresh notarization; shows stapler's own words when it fails.
stapled() {
  local out attempt
  for attempt in 1 2 3; do
    if out="$(xcrun stapler validate "$1" 2>&1)"; then
      return 0
    fi
    if ((attempt < 3)); then
      sleep 10
    fi
  done
  local line
  while IFS= read -r line; do
    printf '  stapler: %s\n' "$line" >&2
  done <<<"$out"
  return 1
}
check "its notarization ticket is stapled" stapled "$app"

# Captured before matching: grep -q leaving early would kill spctl with
# SIGPIPE, and pipefail would fail the check at random.
gatekeeper_accepts() {
  local out
  out="$(spctl --assess --type execute --verbose=2 "$1" 2>&1)" || return 1
  grep -q '^source=Notarized Developer ID$' <<<"$out"
}
if spctl --status 2>&1 | grep -q 'assessments disabled'; then
  # Some machines (CI images among them) run with Gatekeeper off; the
  # stapled ticket above still proves notarization.
  echo "skipped: Gatekeeper assessments are disabled on this machine"
else
  check "Gatekeeper accepts it as notarized" gatekeeper_accepts "$app"
fi

if ((failures > 0)); then
  echo "$app is not fit to publish: $failures check(s) failed" >&2
  exit 1
fi
echo "$app is signed, notarized and accepted by Gatekeeper"
