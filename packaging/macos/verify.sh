#!/usr/bin/env bash
# Check MoneyKeep's macOS disk image exactly as users download it (docs/specs/macos-release.md
# P1-P3, P5; MAC-4 is the manual part on a second Mac):
#
#   $DIST/moneykeep-<version>-macos-<arch>.dmg   signed (notarized and stapled with a Developer ID);
#                                                 MoneyKeep.app inside, every program in it signed
#                                                 with the hardened runtime and carrying every arch
#
# Usage: packaging/macos/verify.sh [--arch universal|aarch64|x86_64]
#
# With a real identity and notarization settings in the environment (the MACOS_SIGN_IDENTITY and
# MACOS_NOTARY_PROFILE, or MACOS_NOTARY_KEY..., that package.sh used) every check is strict and a
# failure exits non-zero. Ad hoc (no Developer ID yet, decision O1), only the signatures'
# integrity is checked and the rest are warnings. Nothing here reads or prints a secret.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

APP_ID=moneykeep
APP_NAME=MoneyKeep
BUNDLE_ID=com.moneykeep.moneykeep
EXTRAS=(moneykeep-server keep-mcp)

ARCH=universal
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    -h | --help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) WANT_ARCHS="x86_64 arm64" ;;
  aarch64) WANT_ARCHS="arm64" ;;
  x86_64) WANT_ARCHS="x86_64" ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

DMG="$DIST/$APP_ID-$VERSION-macos-$ARCH.dmg"
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
STRICT=0
if [[ "$IDENTITY" != "-" ]]; then
  if [[ -n "${MACOS_NOTARY_PROFILE:-}" ]] || [[ -n "${MACOS_NOTARY_KEY:-}" ]]; then
    STRICT=1
  fi
fi

FAILED=0
fail() {
  if [[ "$STRICT" = 1 ]]; then
    echo "error: $*" >&2
    FAILED=1
  else
    warn "$* (expected for an ad hoc build)"
  fi
}

# spctl's online ticket lookup can lag a few seconds behind notarytool's "Accepted".
assess() {
  local out attempt tries=1
  [[ "$STRICT" = 1 ]] && tries=5
  for attempt in $(seq "$tries"); do
    if out="$(spctl "$@" 2>&1)"; then
      printf '%s\n' "$out"
      return 0
    fi
    [[ "$attempt" = "$tries" ]] || sleep 10
  done
  printf '%s\n' "$out"
  return 1
}

stapler_validate() {
  local try
  for try in 1 2 3 4; do
    xcrun stapler validate "$1" && return 0
    [[ "$try" = 4 ]] && break
    echo "stapler validate failed (try $try of 4); retrying in $((try * 20)) s" >&2
    sleep $((try * 20))
  done
  return 1
}

if [[ ! -f "$DMG" ]]; then
  echo "error: missing $DMG" >&2
  exit 1
fi

WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/$APP_ID-verify.XXXXXX")"
MNT="$WORK/mnt"
cleanup() {
  hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- the disk image -----------------------------------------------------------------------------
echo "==> $(basename "$DMG")"
codesign --verify --strict --verbose=2 "$DMG"
if [[ "$STRICT" = 1 ]]; then
  stapler_validate "$DMG" || fail "no notarization ticket stapled to the disk image"
  assess --assess --type open --context context:primary-signature -vv "$DMG" \
    || fail "Gatekeeper rejects the disk image"
fi
if [[ -f "$DIST/SHA256SUMS.txt" ]] && ! grep -qx "$(sha256 "$DMG")  $(basename "$DMG")" "$DIST/SHA256SUMS.txt"; then
  echo "error: SHA256SUMS.txt has no matching line for $(basename "$DMG")" >&2
  exit 1
fi

# ---- the app inside -----------------------------------------------------------------------------
mkdir -p "$MNT"
hdiutil attach "$DMG" -readonly -nobrowse -noautoopen -mountpoint "$MNT" -quiet
APP="$MNT/$APP_NAME.app"
if [[ ! -L "$MNT/Applications" ]]; then
  echo "error: the disk image has no Applications link" >&2
  exit 1
fi
id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
if [[ "$id" != "$BUNDLE_ID" ]]; then
  echo "error: bundle identifier is $id, not $BUNDLE_ID" >&2
  exit 1
fi
echo "==> $APP_NAME.app ($id)"

for exe in "$APP_NAME" "${EXTRAS[@]}"; do
  path="$APP/Contents/MacOS/$exe"
  if [[ ! -x "$path" ]]; then
    echo "error: $APP_NAME.app has no Contents/MacOS/$exe" >&2
    exit 1
  fi
  archs="$(lipo -archs "$path")"
  for a in $WANT_ARCHS; do
    case " $archs " in
      *" $a "*) ;;
      *) echo "error: $exe lacks $a (has: $archs)" >&2; exit 1 ;;
    esac
  done
  details="$(codesign -d --verbose=2 "$path" 2>&1)"
  printf '%s\n' "$details" | grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*runtime' \
    || fail "$exe is not signed with the hardened runtime"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' \
    || fail "$exe is not signed with a Developer ID Application identity"
  printf '%s\n' "$details" | grep -q '^Timestamp=' \
    || fail "$exe's signature has no secure timestamp"
  echo "    $exe: $archs"
done
for f in "$APP_NAME.icns" THIRD-PARTY-LICENSES.txt NOTICE; do
  [[ -s "$APP/Contents/Resources/$f" ]] || { echo "error: Contents/Resources/$f is missing" >&2; exit 1; }
done

# Integrity: must hold even ad hoc, in the image and on a copy installed from it.
codesign --verify --strict --deep --verbose=2 "$APP"
ditto "$APP" "$WORK/copy/$APP_NAME.app"
codesign --verify --strict --deep --verbose=2 "$WORK/copy/$APP_NAME.app"
codesign -d --verbose=2 "$APP" 2>&1 | grep -E '^(Identifier|Format|CodeDirectory|Authority|Timestamp|TeamIdentifier)[= ]' || true
if [[ "$STRICT" = 1 ]]; then
  stapler_validate "$APP" || fail "no notarization ticket stapled to $APP_NAME.app"
  if gk="$(assess --assess --type execute -vv "$APP")"; then
    printf '%s\n' "$gk"
    printf '%s\n' "$gk" | grep -q 'source=Notarized Developer ID' \
      || fail "Gatekeeper accepts $APP_NAME.app but not as notarized"
  else
    printf '%s\n' "$gk"
    fail "Gatekeeper rejects $APP_NAME.app (not notarized?)"
  fi
fi

if [[ "$FAILED" != 0 ]]; then
  echo "error: the macOS disk image failed verification" >&2
  exit 1
fi
if [[ "$STRICT" = 1 ]]; then
  result="signed (Developer ID, hardened runtime), notarized; disk image and app stapled"
else
  result="signatures intact; signed ad hoc, not notarized (no Developer ID yet)"
fi
echo "==> verified: $result"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  echo "- macOS \`$(basename "$DMG")\`: $result" >>"$GITHUB_STEP_SUMMARY"
fi
