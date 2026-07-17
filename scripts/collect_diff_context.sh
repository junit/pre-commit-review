#!/usr/bin/env bash
# Collect Git diff context for the pre-commit-review skill.
# Supports running Rust version, legacy shell version, shadow mode, and fallback.

# We don't set -e immediately because we want to capture exit codes for fallback/shadow modes.
set -uo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
LEGACY_SCRIPT="${SCRIPT_DIR}/collect_diff_context.legacy.sh"
WRAPPER_SCRIPT="${SCRIPT_DIR}/collect_diff_context.sh"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

# Normalize OS and ARCH
case "$OS" in
  darwin)  OS_NAME="darwin" ;;
  linux)   OS_NAME="linux" ;;
  msys*|mingw*|cygwin*) OS_NAME="windows" ;;
  *)       OS_NAME="linux" ;;
esac

case "$ARCH" in
  x86_64|amd64) ARCH_NAME="amd64" ;;
  arm64|aarch64) ARCH_NAME="arm64" ;;
  *)            ARCH_NAME="amd64" ;;
esac

BINARY_NAME="collect_diff_context-${OS_NAME}-${ARCH_NAME}"
if [ "$OS_NAME" = "windows" ]; then
  BINARY_NAME="${BINARY_NAME}.exe"
fi

BINARY_PATH="${SCRIPT_DIR}/bin/${BINARY_NAME}"
SECRET_SCAN_MODE="${PRE_COMMIT_REVIEW_SECRET_SCAN:-auto}"
SANITIZER_BIN=''
SCAN_REPORT_FILES=''
SCAN_DEGRADED_REASON=''
CONTROL_PLANE_REQUEST='no'

case "$SECRET_SCAN_MODE" in
  auto|off) ;;
  *) SECRET_SCAN_MODE='auto' ;;
esac

for arg in "$@"; do
  if [ "$arg" = '--control-plane' ]; then
    CONTROL_PLANE_REQUEST='yes'
    break
  fi
done

# Fallback binary if precompiled not found
CARGO_RELEASE_BIN="${SCRIPT_DIR}/../collect-diff-context-cli/target/release/collect-diff-context-cli"

TEMP_FILES=''
register_temp_file() {
  [ -n "${1:-}" ] || return 0
  TEMP_FILES="${TEMP_FILES}${TEMP_FILES:+
}$1"
}

# shellcheck disable=SC2329 # Invoked indirectly by the EXIT trap.
cleanup_temp_files() {
  [ -n "${TEMP_FILES:-}" ] || return 0
  local temp_file
  while IFS= read -r temp_file; do
    [ -n "$temp_file" ] && rm -f "$temp_file"
  done <<EOF_CLEANUP
$TEMP_FILES
EOF_CLEANUP
}
trap cleanup_temp_files EXIT

set_scan_degraded_reason() {
  [ -n "$SCAN_DEGRADED_REASON" ] || SCAN_DEGRADED_REASON="$1"
}

append_scan_report() {
  local report_file="$1"
  SCAN_REPORT_FILES="${SCAN_REPORT_FILES}${SCAN_REPORT_FILES:+
}${report_file}"
}

ensure_sanitizer_bin() {
  if [ -n "$SANITIZER_BIN" ] && [ -x "$SANITIZER_BIN" ]; then
    return 0
  fi
  if [ -n "${PRE_COMMIT_REVIEW_SANITIZER_BIN:-}" ] \
    && [ -x "$PRE_COMMIT_REVIEW_SANITIZER_BIN" ]; then
    SANITIZER_BIN="$PRE_COMMIT_REVIEW_SANITIZER_BIN"
  elif [ -x "$CARGO_RELEASE_BIN" ]; then
    SANITIZER_BIN="$CARGO_RELEASE_BIN"
  elif [ -x "$BINARY_PATH" ]; then
    SANITIZER_BIN="$BINARY_PATH"
  else
    SANITIZER_BIN="$(get_rust_binary 2>/dev/null || true)"
  fi
  [ -n "$SANITIZER_BIN" ] && [ -x "$SANITIZER_BIN" ]
}

sanitize_file_in_place() {
  local input_file="$1"
  local stream_name="$2"
  local publish_report="$3"
  [ -s "$input_file" ] || return 0

  if [ "$SECRET_SCAN_MODE" = 'off' ]; then
    [ "$publish_report" = 'yes' ] && set_scan_degraded_reason 'disabled'
    return 0
  fi
  if ! ensure_sanitizer_bin; then
    [ "$publish_report" = 'yes' ] && set_scan_degraded_reason 'sanitizer-unavailable'
    return 0
  fi

  local sanitized_file
  local report_file
  local scanner_error
  sanitized_file="$(mktemp)"
  report_file="$(mktemp)"
  scanner_error="$(mktemp)"
  register_temp_file "$sanitized_file"
  register_temp_file "$report_file"
  register_temp_file "$scanner_error"

  local sanitize_exit=0
  PRE_COMMIT_REVIEW_SANITIZE_REPORT="$report_file" \
    PRE_COMMIT_REVIEW_SANITIZE_STREAM="$stream_name" \
    "$SANITIZER_BIN" --sanitize-stdin \
      < "$input_file" > "$sanitized_file" 2> "$scanner_error" \
    || sanitize_exit=$?

  if [ "$sanitize_exit" -eq 0 ] \
    && grep -Fq 'protocol: pcr-sanitizer-v1' "$report_file" \
    && grep -Eq '^status: (clean|redacted)$' "$report_file"; then
    mv "$sanitized_file" "$input_file"
    if [ "$publish_report" = 'yes' ] \
      && grep -Fq 'status: redacted' "$report_file"; then
      append_scan_report "$report_file"
    fi
    return 0
  fi

  if grep -Fq 'protocol: pcr-sanitizer-v1' "$report_file" \
    && grep -Eq '^status: (unavailable|redaction-failed)$' "$report_file"; then
    [ "$publish_report" = 'yes' ] && append_scan_report "$report_file"
    return 0
  fi

  [ "$publish_report" = 'yes' ] \
    && set_scan_degraded_reason 'optional-scanner-unavailable-or-failed'
  return 0
}

sanitize_captured_pair() {
  local stdout_file="$1"
  local stderr_file="$2"
  local publish_report="${3:-yes}"
  sanitize_file_in_place "$stdout_file" 'stdout' "$publish_report"
  sanitize_file_in_place "$stderr_file" 'stderr' "$publish_report"
}

emit_optional_scan_summary_body() {
  local report_file
  while IFS= read -r report_file; do
    [ -n "$report_file" ] || continue
    printf '\n'
    cat "$report_file"
  done <<EOF_REPORTS
$SCAN_REPORT_FILES
EOF_REPORTS

  if [ -n "$SCAN_DEGRADED_REASON" ]; then
    printf '\n%s\n' '# Pre-Commit Review Secret Scan'
    printf '%s\n' 'scanner: gitleaks'
    if [ "$SCAN_DEGRADED_REASON" = 'disabled' ]; then
      printf '%s\n' 'status: disabled'
    else
      printf '%s\n' 'status: unavailable'
    fi
    printf 'reason: %s\n' "$SCAN_DEGRADED_REASON"
    printf '%s\n' 'redaction_applied: no'
    printf '%s\n' 'review_continued: yes'
  fi
}

emit_optional_scan_summary() {
  if [ "$CONTROL_PLANE_REQUEST" = 'yes' ]; then
    emit_optional_scan_summary_body >&2
  else
    emit_optional_scan_summary_body
  fi
}

release_captured_output() {
  local stdout_file="$1"
  local stderr_file="$2"
  local command_exit="$3"
  sanitize_captured_pair "$stdout_file" "$stderr_file" 'yes'
  cat "$stdout_file"
  emit_optional_scan_summary
  cat "$stderr_file" >&2
  return "$command_exit"
}

get_rust_binary() {
  if [ -n "${PRE_COMMIT_REVIEW_RUST_BIN:-}" ] && [ -x "$PRE_COMMIT_REVIEW_RUST_BIN" ]; then
    echo "$PRE_COMMIT_REVIEW_RUST_BIN"
  elif [ -f "$BINARY_PATH" ]; then
    echo "$BINARY_PATH"
  elif [ -f "$CARGO_RELEASE_BIN" ]; then
    echo "$CARGO_RELEASE_BIN"
  else
    # Build it
    if command -v cargo >/dev/null 2>&1; then
      (cd "${SCRIPT_DIR}/../collect-diff-context-cli" && cargo build --release >/dev/null 2>&1)
      if [ -f "$CARGO_RELEASE_BIN" ]; then
        echo "$CARGO_RELEASE_BIN"
        return 0
      fi
    fi
    return 1
  fi
}

# Determine Implementation Mode
# PRE_COMMIT_REVIEW_HELPER_IMPL can be: "rust", "legacy", "shell", "shadow"
IMPL="${PRE_COMMIT_REVIEW_HELPER_IMPL:-rust}"
SHADOW_MODE="${PRE_COMMIT_REVIEW_SHADOW_MODE:-0}"

# Run legacy shell implementation
run_legacy() {
  if [ ! -f "$LEGACY_SCRIPT" ]; then
    echo "Error: Legacy shell script not found at $LEGACY_SCRIPT" >&2
    exit 1
  fi
  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  local legacy_out
  local legacy_err
  legacy_out=$(mktemp)
  legacy_err=$(mktemp)
  register_temp_file "$legacy_out"
  register_temp_file "$legacy_err"
  local legacy_exit=0
  "$LEGACY_SCRIPT" "$@" > "$legacy_out" 2> "$legacy_err" || legacy_exit=$?
  local release_exit=0
  release_captured_output "$legacy_out" "$legacy_err" "$legacy_exit" || release_exit=$?
  exit "$release_exit"
}

# Run Rust implementation
run_rust_only() {
  local bin
  if ! bin="$(get_rust_binary)" || [ -z "$bin" ]; then
    echo "Error: Rust binary not found and cargo build failed." >&2
    exit 1
  fi
  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  local rust_out
  local rust_err
  rust_out=$(mktemp)
  rust_err=$(mktemp)
  register_temp_file "$rust_out"
  register_temp_file "$rust_err"
  local rust_exit=0
  "$bin" "$@" > "$rust_out" 2> "$rust_err" || rust_exit=$?
  local release_exit=0
  release_captured_output "$rust_out" "$rust_err" "$rust_exit" || release_exit=$?
  exit "$release_exit"
}

# Run Rust implementation with fallback to legacy on failure
run_rust_with_fallback() {
  local bin
  if ! bin="$(get_rust_binary)" || [ -z "$bin" ]; then
    echo "Warning: Rust binary not found. Falling back to legacy shell script..." >&2
    run_legacy "$@"
  fi

  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  
  # Run Rust binary and capture output and exit code
  # Note: we use a temp file to avoid pipe issues or memory limits for stdout
  local rust_out
  local rust_err
  rust_out=$(mktemp)
  rust_err=$(mktemp)
  register_temp_file "$rust_out"
  register_temp_file "$rust_err"
  
  # Disable set -e temporarily to handle exit status
  set +e
  "$bin" "$@" > "$rust_out" 2> "$rust_err"
  local rust_exit=$?
  set -e

  if [ $rust_exit -eq 0 ]; then
    local release_exit=0
    release_captured_output "$rust_out" "$rust_err" 0 || release_exit=$?
    exit "$release_exit"
  else
    rm -f "$rust_out" "$rust_err"
    if [ -f "$LEGACY_SCRIPT" ]; then
      echo "Warning: Rust helper failed (exit code ${rust_exit}). Falling back to legacy Shell helper..." >&2
      run_legacy "$@"
    else
      echo "Error: Rust helper failed (exit code ${rust_exit}) and no legacy helper is available." >&2
      exit "$rust_exit"
    fi
  fi
}

# Run in shadow comparison mode
run_shadow() {
  local bin
  if ! bin="$(get_rust_binary)" || [ -z "$bin" ]; then
    echo "Warning: Rust binary not found for shadow mode. Running legacy shell script only..." >&2
    run_legacy "$@"
  fi

  if [ ! -f "$LEGACY_SCRIPT" ]; then
    echo "Warning: Legacy script not found for shadow mode. Running Rust only..." >&2
    run_rust_only "$@"
  fi

  local legacy_out
  local legacy_err
  local rust_out
  local rust_err
  legacy_out=$(mktemp)
  legacy_err=$(mktemp)
  rust_out=$(mktemp)
  rust_err=$(mktemp)
  register_temp_file "$legacy_out"
  register_temp_file "$legacy_err"
  register_temp_file "$rust_out"
  register_temp_file "$rust_err"

  set +e
  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  "$LEGACY_SCRIPT" "$@" > "$legacy_out" 2> "$legacy_err"
  local legacy_exit=$?

  "$bin" "$@" > "$rust_out" 2> "$rust_err"
  local rust_exit=$?
  set -e

  sanitize_captured_pair "$legacy_out" "$legacy_err" 'yes'
  sanitize_captured_pair "$rust_out" "$rust_err" 'no'

  # Compare outputs
  if ! diff -u "$legacy_out" "$rust_out" > /dev/null 2>&1; then
    echo "Warning: [Shadow Mode] Output mismatch detected between legacy shell and Rust implementation!" >&2
    if [ -n "${PRE_COMMIT_REVIEW_SHADOW_DIFF_LOG:-}" ]; then
      local diff_log="$PRE_COMMIT_REVIEW_SHADOW_DIFF_LOG"
      {
        echo "=== DIFF MISMATCH ON $(date) ==="
        printf 'Args: %s\n' "$*"
        diff -u "$legacy_out" "$rust_out" || true
        echo "================================"
      } >> "$diff_log" 2>/dev/null || {
        printf 'Warning: [Shadow Mode] Could not write diff log to %s\n' "$diff_log" >&2
      }
    else
      echo "Warning: [Shadow Mode] Diff logging is disabled by default; set PRE_COMMIT_REVIEW_SHADOW_DIFF_LOG to a local path if you need mismatch details." >&2
    fi
  fi

  if [ $legacy_exit -ne $rust_exit ]; then
    echo "Warning: [Shadow Mode] Exit code mismatch! Legacy exit code: $legacy_exit, Rust exit code: $rust_exit" >&2
  fi

  # Preserve the legacy-selected output during rollout.
  cat "$legacy_out"
  emit_optional_scan_summary
  cat "$legacy_err" >&2
  rm -f "$legacy_out" "$legacy_err" "$rust_out" "$rust_err"
  exit "$legacy_exit"
}

# Execute based on configured mode
if [ "$IMPL" = "shadow" ] || [ "$SHADOW_MODE" = "1" ]; then
  run_shadow "$@"
elif [ "$IMPL" = "legacy" ] || [ "$IMPL" = "shell" ]; then
  run_legacy "$@"
elif [ "${PRE_COMMIT_REVIEW_DISABLE_FALLBACK:-0}" = "1" ]; then
  run_rust_only "$@"
else
  run_rust_with_fallback "$@"
fi
