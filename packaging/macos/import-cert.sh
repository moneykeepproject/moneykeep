#!/usr/bin/env bash
# GitHub's Mac only (release.yml's macos job with `macos: github`, for when the project's Mac
# is away; docs/specs/macos-release.md W2, W4): put the Developer ID certificate from the
# repository's secrets into a temporary keychain, and the notarization key into a private file.
#
# Inputs (env, from the mac-release environment's secrets; all optional):
#   APPLE_CERTIFICATE            the Developer ID Application certificate and key, a base64 .p12
#   APPLE_CERTIFICATE_PASSWORD   its password (read by openssl from the environment)
#   APPLE_API_KEY                an App Store Connect API key for notarytool, a base64 .p8
#   APPLE_API_KEY_ID, APPLE_API_ISSUER   that key's ID and issuer ID
# Outputs: MACOS_SIGN_IDENTITY, MACOS_KEYCHAIN and, with the API key, MACOS_NOTARY_KEY,
# MACOS_NOTARY_KEY_ID and MACOS_NOTARY_ISSUER, appended to $GITHUB_ENV (or printed).
#
# No secret goes on a command line (security-review-1 L9): openssl reads the .p12 password from
# the environment and the key is imported unencrypted into the temporary keychain; notarytool
# reads the API key from a file. The temporary keychain's own password is made here at random,
# is no project secret, and dies with the keychain (release.yml deletes it after the job).
#
# No certificate: a warning and exit 0, so package.sh signs ad hoc.
set -euo pipefail
umask 077
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

if [[ -z "${APPLE_CERTIFICATE:-}" ]]; then
  warn "APPLE_CERTIFICATE is not set: ad hoc only. The macOS build is signed ad hoc and not notarized"
  exit 0
fi

# A private folder of our own, so a predictable name in a shared temp folder can't be taken over
# to capture the decoded key (CWE-377). The runner discards RUNNER_TEMP when the job ends.
WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/mk-signing.XXXXXXXX")"
KEYCHAIN="$WORK/mk-signing-temp.keychain-db"
P12="$WORK/cert.p12"
PEM="$WORK/cert.pem"
trap 'rm -f "$P12" "$PEM"' EXIT
KC_PASS="$(openssl rand -hex 24)"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode >"$P12"
export MK_P12_PASSWORD="${APPLE_CERTIFICATE_PASSWORD:-}"
openssl pkcs12 -in "$P12" -passin env:MK_P12_PASSWORD -nodes -out "$PEM"
unset MK_P12_PASSWORD
rm -f "$P12"

security create-keychain -p "$KC_PASS" "$KEYCHAIN"
security set-keychain-settings -lut 21600 "$KEYCHAIN"
security unlock-keychain -p "$KC_PASS" "$KEYCHAIN"
security import "$PEM" -k "$KEYCHAIN" -T /usr/bin/codesign -T /usr/bin/security
rm -f "$PEM"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KEYCHAIN" >/dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')

IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" | awk '/Developer ID Application/ { print $2; exit }')"
if [[ -z "$IDENTITY" ]]; then
  security find-identity -v -p codesigning "$KEYCHAIN" >&2
  echo "error: no 'Developer ID Application' identity in APPLE_CERTIFICATE" >&2
  exit 1
fi
out="MACOS_SIGN_IDENTITY=$IDENTITY
MACOS_KEYCHAIN=$KEYCHAIN"

if [[ -n "${APPLE_API_KEY:-}" ]] && [[ -n "${APPLE_API_KEY_ID:-}" ]] && [[ -n "${APPLE_API_ISSUER:-}" ]]; then
  KEY="$WORK/notary-key.p8"
  printf '%s' "$APPLE_API_KEY" | base64 --decode >"$KEY"
  out="$out
MACOS_NOTARY_KEY=$KEY
MACOS_NOTARY_KEY_ID=$APPLE_API_KEY_ID
MACOS_NOTARY_ISSUER=$APPLE_API_ISSUER"
else
  warn "APPLE_API_KEY, APPLE_API_KEY_ID or APPLE_API_ISSUER is not set: the macOS build is signed but not notarized"
fi

echo "Imported signing identity $IDENTITY"
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf '%s\n' "$out" >>"$GITHUB_ENV"
else
  printf '%s\n' "$out"
fi
