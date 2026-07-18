#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
# shellcheck source=scripts/lib/gitleaks_integrity.sh
source "$SCRIPT_DIR/lib/gitleaks_integrity.sh"

VERSION_FILE="$SCRIPT_DIR/gitleaks.version"
BINARY_MANIFEST="$SCRIPT_DIR/gitleaks-binaries.sha256"
CONFIG="${PRE_COMMIT_REVIEW_GITLEAKS_CONFIG:-$SCRIPT_DIR/../references/security/gitleaks.toml}"

fail() {
  local reason="$1"
  printf '%s\n' 'Gitleaks doctor: UNAVAILABLE'
  printf 'reason: %s\n' "$reason"
  printf '%s\n' 'redaction_available: no'
  printf '%s\n' 'review_output_allowed: yes'
  exit 1
}

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) os_name='darwin' ;;
  linux) os_name='linux' ;;
  msys*|mingw*|cygwin*) os_name='windows' ;;
  *) fail 'unsupported-operating-system' ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch_name='arm64' ;;
  x86_64|amd64) arch_name='amd64' ;;
  *) fail 'unsupported-architecture' ;;
esac

binary_name="gitleaks-${os_name}-${arch_name}"
if [ "$os_name" = 'windows' ]; then
  binary_name="${binary_name}.exe"
fi

source_kind='bundled'
executable="$SCRIPT_DIR/bin/$binary_name"
if [ -n "${PRE_COMMIT_REVIEW_GITLEAKS_BIN:-}" ]; then
  source_kind='explicit'
  executable="$PRE_COMMIT_REVIEW_GITLEAKS_BIN"
  gitleaks_path_is_absolute "$executable" || fail 'explicit-path-not-absolute'
fi

[ -f "$CONFIG" ] || fail 'trusted-config-unavailable'
[ -x "$executable" ] || fail 'scanner-unavailable'

integrity='explicit-user-trust'
if [ "$source_kind" = 'bundled' ]; then
  gitleaks_hash_matches "$executable" "$BINARY_MANIFEST" "$binary_name" \
    || fail 'binary-integrity-mismatch'
  integrity='sha256-verified'
fi

gitleaks_version_matches "$executable" "$VERSION_FILE" \
  || fail 'version-mismatch'

gitleaks_smoke_scan "$executable" "$CONFIG" \
  || fail 'capability-smoke-test-failed'

printf '%s\n' 'Gitleaks doctor: OK'
printf 'source: %s\n' "$source_kind"
printf 'binary: %s\n' "$executable"
printf 'version: %s\n' "$(tr -d '[:space:]' < "$VERSION_FILE")"
printf 'integrity: %s\n' "$integrity"
printf '%s\n' 'capability: stdin-json-coordinates-ready'
printf '%s\n' 'redaction_available: yes'
printf '%s\n' 'review_output_allowed: yes'
