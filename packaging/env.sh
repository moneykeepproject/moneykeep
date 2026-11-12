# shellcheck shell=bash
# Shared setup for the packaging scripts (docs/specs/macos-release.md). Source it:
#   . "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
#
# Sets:
#   ROOT              the workspace root
#   VERSION           [workspace.package] version from the root Cargo.toml (the version
#                     the release tag must carry: v<VERSION>)
#   DIST              where release files go (default: $ROOT/dist, as cargo xtask installer)
#   CARGO_TARGET_DIR  cargo's target folder (default: $ROOT/target)
# Functions: warn, sha256.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

# The version lives in one place: `[workspace.package] version` in the root Cargo.toml.
workspace_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]"); next }
    in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$ROOT/Cargo.toml"
  return
}

VERSION="$(workspace_version)"
if [[ -z "$VERSION" ]]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

DIST="${DIST:-$ROOT/dist}"
export DIST
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# A GitHub Actions warning (plain stderr outside Actions).
warn() {
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then echo "::warning::$*"; else echo "warning: $*" >&2; fi
  return
}

# The SHA-256 of a file (just the hash).
sha256() {
  local file="$1"
  if command -v sha256sum >/dev/null; then sha256sum "$file" | cut -d' ' -f1; else shasum -a 256 "$file" | cut -d' ' -f1; fi
  return
}
