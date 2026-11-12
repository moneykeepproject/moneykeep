#!/usr/bin/env bash
# Build, sign and (with a Developer ID) notarize MoneyKeep for macOS
# (docs/specs/macos-release.md, Part P):
#
#   $DIST/moneykeep-<version>-macos-<arch>.dmg   MoneyKeep.app and an Applications link
#   $DIST/SHA256SUMS.txt                         that disk image's checksum
#
# MoneyKeep.app/Contents/MacOS holds MoneyKeep (the desktop app), moneykeep-server (the capture
# server, with the phone app built in) and keep-mcp (the MCP server), as the Windows setup does.
# `cargo xtask installer` runs this script on a Mac.
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#                                   [--no-wasm-opt] [--dry-run]
#   --dry-run      print the planned steps and names and stop: builds, signs and runs nothing
#                  (works on any system with bash; test MAC-3)
#   --skip-build   reuse the release programs already in $CARGO_TARGET_DIR/<target>/release
#   --no-wasm-opt  passed to cargo xtask phone-app (developers; the phone program is larger)
#
# Phone app (env): MK_PHONE_APP, a folder made by `cargo xtask phone-app --out DIR` (release.yml
# builds it once on Linux); without it this script runs that command (Trunk and wasm-opt needed).
# Signing (env):
#   MACOS_SIGN_IDENTITY   codesign identity (name or SHA-1). Default "-": ad hoc (until the owner
#                         enrols, decision O1; macOS warns on first open). In CI,
#                         runner-keychain.sh (the project's Mac) or import-cert.sh (GitHub's) sets it.
#   MACOS_KEYCHAIN        the keychain holding it (optional)
# Notarization (env; with a real identity, one of):
#   MACOS_NOTARY_PROFILE  a `notarytool store-credentials` profile in MACOS_KEYCHAIN (the project's
#                         Mac: mk-notary in mk-signing.keychain-db), or
#   MACOS_NOTARY_KEY, MACOS_NOTARY_KEY_ID, MACOS_NOTARY_ISSUER   an App Store Connect API key file
# No password or Apple ID is ever put on a command line (security-review-1 L9).
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

# ---- MoneyKeep's values (P2, P3, P5) ------------------------------------------------------------
APP_ID=moneykeep                    # the release file names' app id
APP_NAME=MoneyKeep                  # bundle, main program, icon and disk image volume
BUNDLE_ID=com.moneykeep.moneykeep
MAIN_PKG=moneykeep-app              # cargo package of the main program (renamed to $APP_NAME)
# Programs beside it in Contents/MacOS, under their own names (the app finds the capture server
# and keep-mcp next to itself). The CLI is not in the bundle: on a case-insensitive disk its name
# `moneykeep` would collide with `MoneyKeep`.
EXTRAS=(moneykeep-server keep-mcp)
DOCS=(NOTICE LICENSE-MIT LICENSE-APACHE ATTRIBUTION.md)
LICENCES=THIRD-PARTY-LICENSES.txt

ARCH=universal
SKIP_BUILD=0
DRY_RUN=0
NO_WASM_OPT=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --no-wasm-opt) NO_WASM_OPT=(--no-wasm-opt); shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h | --help) sed -n '2,31p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in (P1: macOS 11.0).
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
SHORT_VERSION="${VERSION%%-*}"
BUILD_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/$APP_NAME.app"
DMG_NAME="$APP_ID-$VERSION-macos-$ARCH.dmg"
DMG="$DIST/$DMG_NAME"
PROGRAMS=("$MAIN_PKG" "${EXTRAS[@]}")

NOTARIZE=0
NOTARY_AUTH=()
NOTARY_WHAT="no (ad hoc: Apple notarizes only Developer ID signatures)"
if [[ "$IDENTITY" = "-" ]]; then
  SIGN_WHAT="ad hoc (no MACOS_SIGN_IDENTITY; macOS warns on first open; not notarized)"
else
  SIGN_WHAT="identity $IDENTITY${MACOS_KEYCHAIN:+ (keychain $MACOS_KEYCHAIN)}"
  if [[ -n "${MACOS_NOTARY_PROFILE:-}" ]]; then
    NOTARIZE=1
    NOTARY_AUTH=(--keychain-profile "$MACOS_NOTARY_PROFILE")
    [[ -n "${MACOS_KEYCHAIN:-}" ]] && NOTARY_AUTH+=(--keychain "$MACOS_KEYCHAIN")
    NOTARY_WHAT="xcrun notarytool submit --keychain-profile $MACOS_NOTARY_PROFILE --wait, then xcrun stapler staple"
  elif [[ -n "${MACOS_NOTARY_KEY:-}" ]] && [[ -n "${MACOS_NOTARY_KEY_ID:-}" ]] && [[ -n "${MACOS_NOTARY_ISSUER:-}" ]]; then
    NOTARIZE=1
    NOTARY_AUTH=(--key "$MACOS_NOTARY_KEY" --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER")
    NOTARY_WHAT="xcrun notarytool submit --key <file> --key-id $MACOS_NOTARY_KEY_ID --wait, then xcrun stapler staple"
  else
    NOTARY_WHAT="no (no MACOS_NOTARY_PROFILE or API key: signed but not notarized)"
  fi
fi

# The plan, printed before a real run and alone with --dry-run (MAC-3).
plan() {
  local i=0 p
  step() { i=$((i + 1)); printf '  %2d. %s\n' "$i" "$*"; }
  echo "==> $APP_NAME $VERSION for macOS ($ARCH)$([[ "$DRY_RUN" = 1 ]] && echo ', dry run: nothing is built, signed or run')"
  if [[ -n "${MK_PHONE_APP:-}" ]]; then
    step "phone app: prebuilt in $MK_PHONE_APP (cargo xtask phone-app)"
  else
    step "phone app: cargo xtask phone-app --out $WORK/phone${NO_WASM_OPT[*]:+ ${NO_WASM_OPT[*]}}"
  fi
  if [[ "$SKIP_BUILD" = 1 ]]; then
    step "build: skipped (--skip-build), using $CARGO_TARGET_DIR/<target>/release"
  else
    step "build: cargo build --release --locked$(printf ' -p %s' "${PROGRAMS[@]}")$(printf ' --target %s' "${TARGETS[@]}") (MK_CAPTURE_DIST: the phone app; MACOSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET)"
  fi
  step "check: moneykeep-server version has the phone app and the capture version of cargo xtask release --dry-run"
  for p in "${PROGRAMS[@]}"; do
    step "lipo -create $p from$(printf ' %s' "${TARGETS[@]}")"
  done
  step "icon: $APP_NAME.icns from the calculator drawing (cargo test -p mk-ui --test app_ico app_icns_has_every_size, MK_ICNS_OUT)"
  step "bundle $APP_NAME.app, identifier $BUNDLE_ID, version $VERSION ($SHORT_VERSION), minimum macOS $MACOSX_DEPLOYMENT_TARGET:"
  echo "        $APP_NAME.app/Contents/Info.plist"
  echo "        $APP_NAME.app/Contents/PkgInfo"
  echo "        $APP_NAME.app/Contents/MacOS/$APP_NAME"
  for p in "${EXTRAS[@]}"; do echo "        $APP_NAME.app/Contents/MacOS/$p"; done
  echo "        $APP_NAME.app/Contents/Resources/$APP_NAME.icns"
  echo "        $APP_NAME.app/Contents/Resources/$LICENCES"
  for p in "${DOCS[@]}"; do echo "        $APP_NAME.app/Contents/Resources/$p"; done
  step "sign: $SIGN_WHAT; hardened runtime, entitlements.plist; inside out:$(printf ' %s,' "${EXTRAS[@]}") $APP_NAME, then $APP_NAME.app"
  step "notarize $APP_NAME.app: $NOTARY_WHAT"
  step "disk image: $DMG_NAME (volume $APP_NAME: $APP_NAME.app and an Applications link), signed like the app; notarize: $NOTARY_WHAT"
  step "checksums: $DIST/SHA256SUMS.txt with $DMG_NAME"
  return
}

plan
if [[ "$DRY_RUN" = 1 ]]; then
  exit 0
fi
[[ "$IDENTITY" = "-" ]] && warn "macOS: ad hoc signing (no MACOS_SIGN_IDENTITY); $DMG_NAME is not notarized"
[[ "$IDENTITY" != "-" ]] && [[ "$NOTARIZE" = 0 ]] && warn "macOS: $NOTARY_WHAT"
if [[ "$(uname -s)" != Darwin ]]; then
  echo "error: package.sh builds the Mac app with Apple's tools: run it on a Mac (--dry-run works anywhere)" >&2
  exit 1
fi

xtask() { (cd "$ROOT" && env -u CARGO_BUILD_TARGET cargo run --quiet --locked -p xtask -- "$@"); }

rm -rf "$WORK"
mkdir -p "$WORK/bin" "$DIST"

# ---- phone app ----------------------------------------------------------------------------------
PHONE="${MK_PHONE_APP:-}"
if [[ -z "$PHONE" ]]; then
  PHONE="$WORK/phone"
  xtask phone-app --out "$PHONE" ${NO_WASM_OPT[@]+"${NO_WASM_OPT[@]}"}
fi
if [[ ! -f "$PHONE/app/index.html" ]]; then
  echo "error: $PHONE/app has no index.html (make it with cargo xtask phone-app --out $PHONE)" >&2
  exit 1
fi
PHONE="$(cd "$PHONE" && pwd)"

# ---- build --------------------------------------------------------------------------------------
if [[ "$SKIP_BUILD" = 0 ]]; then
  args=()
  for p in "${PROGRAMS[@]}"; do args+=(-p "$p"); done
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  (cd "$ROOT" && MK_CAPTURE_DIST="$PHONE/app" cargo build --release --locked "${args[@]}")
fi

# The server must carry the phone app and this checkout's capture version. `release --dry-run`
# builds nothing; it writes version.json and THIRD-PARTY-LICENSES.txt (this Mac's platform; both
# slices of a universal build use the same crates).
xtask release --dry-run --out "$WORK/plan" >/dev/null
PLANNED="$WORK/plan/MoneyKeep-$VERSION"
want="$(plutil -extract capture_version raw -o - "$PLANNED/version.json")"
host_target="aarch64-apple-darwin"
[[ "$(uname -m)" = x86_64 ]] && host_target="x86_64-apple-darwin"
server="$CARGO_TARGET_DIR/$host_target/release/moneykeep-server"
[[ -x "$server" ]] || server="$CARGO_TARGET_DIR/${TARGETS[0]}/release/moneykeep-server"
"$server" version >"$WORK/server-version.json"
got="$(plutil -extract capture_version raw -o - "$WORK/server-version.json")"
has_app="$(plutil -extract has_app raw -o - "$WORK/server-version.json")"
if [[ "$got" != "$want" ]] || [[ "$has_app" != true ]]; then
  echo "error: the built server reports capture version $got (phone app: $has_app); this checkout is $want with the phone app" >&2
  exit 1
fi
echo "==> moneykeep-server: capture version $got, phone app built in"

for p in "${PROGRAMS[@]}"; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$p"); done
  lipo -create -output "$WORK/bin/$p" "${inputs[@]}"
  lipo -info "$WORK/bin/$p"
done

echo "==> icon"
(cd "$ROOT" && MK_ICNS_OUT="$WORK/$APP_NAME.icns" cargo test --quiet --locked -p mk-ui --test app_ico app_icns_has_every_size)
[[ -s "$WORK/$APP_NAME.icns" ]] || { echo "error: making $APP_NAME.icns failed" >&2; exit 1; }

# ---- signing helpers ----------------------------------------------------------------------------
sign() {
  # Hardened runtime and a secure timestamp for a real identity; ad hoc can't be timestamped.
  local ts=(--timestamp)
  [[ "$IDENTITY" = "-" ]] && ts=(--timestamp=none)
  local kc=()
  [[ -n "${MACOS_KEYCHAIN:-}" ]] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" "${NOTARY_AUTH[@]}" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [[ "$status" != "Accepted" ]]; then
    if [[ -n "$id" ]]; then
      xcrun notarytool log "$id" "${NOTARY_AUTH[@]}" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# `stapler staple` and `validate` fetch the ticket from Apple, and a dropped connection fails them
# although notarization worked. Both are idempotent, so retry with a pause.
stapler_retry() {
  local action="$1" target="$2" try
  for try in 1 2 3 4; do
    xcrun stapler "$action" "$target" && return 0
    [[ "$try" = 4 ]] && break
    echo "stapler $action failed (try $try of 4); retrying in $((try * 20)) s" >&2
    sleep $((try * 20))
  done
  echo "error: stapler $action $(basename "$target") failed 4 times" >&2
  exit 1
}

# ---- MoneyKeep.app ------------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$WORK/bin/$MAIN_PKG" "$APP/Contents/MacOS/$APP_NAME"
for p in "${EXTRAS[@]}"; do cp "$WORK/bin/$p" "$APP/Contents/MacOS/$p"; done
cp "$WORK/$APP_NAME.icns" "$APP/Contents/Resources/$APP_NAME.icns"
cp "$PLANNED/$LICENCES" "$APP/Contents/Resources/$LICENCES"
for d in "${DOCS[@]}"; do cp "$ROOT/$d" "$APP/Contents/Resources/"; done
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" -e "s/@BUILD_SHA@/$BUILD_SHA/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Extended attributes picked up during the build fail `codesign --verify --strict`.
xattr -cr "$APP"

# Inside out: the programs beside the main one, the main program, then the bundle (P5).
for p in "${EXTRAS[@]}"; do
  sign --options runtime --entitlements "$HERE/entitlements.plist" --identifier "$BUNDLE_ID.$p" "$APP/Contents/MacOS/$p"
done
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/$APP_NAME"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

if [[ "$NOTARIZE" = 1 ]]; then
  ditto -c -k --keepParent "$APP" "$WORK/notarize.zip"
  notarize "$WORK/notarize.zip"
  stapler_retry staple "$APP"
  stapler_retry validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- disk image (P3) ----------------------------------------------------------------------------
# Finder's plain window: the app and the Applications link (packaging/macos/dmg/README.md).
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/$APP_NAME.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# `hdiutil create` attaches a device while it copies, which now and then fails on busy machines
# ("Resource busy"): a few tries with growing pauses.
for attempt in 1 2 3 4 5; do
  if hdiutil create -srcfolder "$STAGE" -volname "$APP_NAME" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"; then
    break
  fi
  [[ "$attempt" = 5 ]] && { echo "error: hdiutil create failed 5 times" >&2; exit 1; }
  warn "hdiutil create failed (attempt $attempt of 5); retrying in $((attempt * 10)) s"
  sleep $((attempt * 10))
done
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
if [[ "$NOTARIZE" = 1 ]]; then
  notarize "$DMG"
  stapler_retry staple "$DMG"
  stapler_retry validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

printf '%s  %s\n' "$(sha256 "$DMG")" "$DMG_NAME" >"$DIST/SHA256SUMS.txt"
echo "==> done (check the disk image as shipped with packaging/macos/verify.sh --arch $ARCH)"
ls -lh "$DMG"
cat "$DIST/SHA256SUMS.txt"
