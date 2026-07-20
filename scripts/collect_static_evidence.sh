#!/usr/bin/env bash
# Normalize explicit SARIF/JSON results and sanitize the machine-readable output.
set -uo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
PYTHON_COLLECTOR="$SCRIPT_DIR/collect_static_evidence.py"
SECRET_SCAN_MODE="${PRE_COMMIT_REVIEW_SECRET_SCAN:-auto}"

tmp_output="$(mktemp)"
tmp_error="$(mktemp)"
tmp_sanitized="$(mktemp)"
tmp_report="$(mktemp)"
trap 'rm -f "$tmp_output" "$tmp_error" "$tmp_sanitized" "$tmp_report"' EXIT

if ! command -v python3 >/dev/null 2>&1; then
  printf '%s\n' 'collect_static_evidence: python3 is required for optional static-result ingestion' >&2
  exit 2
fi

collector_exit=0
python3 "$PYTHON_COLLECTOR" "$@" >"$tmp_output" 2>"$tmp_error" || collector_exit=$?
if [ "$collector_exit" -ne 0 ]; then
  cat "$tmp_error" >&2
  exit "$collector_exit"
fi

if [ "$SECRET_SCAN_MODE" = 'off' ]; then
  cat "$tmp_output"
  printf '%s\n' '# Pre-Commit Review Static Evidence Secret Scan' >&2
  printf '%s\n' 'status: disabled' 'redaction_applied: no' 'review_continued: yes' >&2
  exit 0
fi

os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch_name="$(uname -m)"
case "$os_name" in
  darwin) os_name='darwin' ;;
  msys*|mingw*|cygwin*) os_name='windows' ;;
  *) os_name='linux' ;;
esac
case "$arch_name" in
  x86_64|amd64) arch_name='amd64' ;;
  arm64|aarch64) arch_name='arm64' ;;
  *) arch_name='amd64' ;;
esac
binary_name="collect_diff_context-${os_name}-${arch_name}"
[ "$os_name" = 'windows' ] && binary_name="${binary_name}.exe"

sanitizer_bin=''
if [ -n "${PRE_COMMIT_REVIEW_SANITIZER_BIN:-}" ] && [ -x "$PRE_COMMIT_REVIEW_SANITIZER_BIN" ]; then
  sanitizer_bin="$PRE_COMMIT_REVIEW_SANITIZER_BIN"
elif [ -x "$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli" ]; then
  sanitizer_bin="$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli"
elif [ -x "$SCRIPT_DIR/bin/$binary_name" ]; then
  sanitizer_bin="$SCRIPT_DIR/bin/$binary_name"
fi

if [ -z "$sanitizer_bin" ]; then
  cat "$tmp_output"
  printf '%s\n' '# Pre-Commit Review Static Evidence Secret Scan' >&2
  printf '%s\n' 'status: unavailable' 'reason: sanitizer-unavailable' \
    'redaction_applied: no' 'review_continued: yes' >&2
  exit 0
fi

sanitize_exit=0
PRE_COMMIT_REVIEW_SANITIZE_REPORT="$tmp_report" \
PRE_COMMIT_REVIEW_SANITIZE_STREAM='static-evidence-stdout' \
  "$sanitizer_bin" --sanitize-stdin <"$tmp_output" >"$tmp_sanitized" 2>>"$tmp_error" \
  || sanitize_exit=$?

if [ "$sanitize_exit" -eq 0 ] \
  && grep -Fq 'protocol: pcr-sanitizer-v1' "$tmp_report" \
  && grep -Eq '^status: (clean|redacted)$' "$tmp_report"; then
  cat "$tmp_sanitized"
  cat "$tmp_report" >&2
  [ -s "$tmp_error" ] && cat "$tmp_error" >&2
  exit 0
fi

cat "$tmp_output"
if grep -Fq 'protocol: pcr-sanitizer-v1' "$tmp_report"; then
  cat "$tmp_report" >&2
else
  printf '%s\n' '# Pre-Commit Review Static Evidence Secret Scan' >&2
  printf '%s\n' 'status: unavailable' 'reason: optional-scanner-unavailable-or-failed' \
    'redaction_applied: no' 'review_continued: yes' >&2
fi
[ -s "$tmp_error" ] && cat "$tmp_error" >&2
exit 0
