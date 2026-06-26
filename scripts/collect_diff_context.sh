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

# Fallback binary if precompiled not found
CARGO_RELEASE_BIN="${SCRIPT_DIR}/../collect-diff-context-cli/target/release/collect-diff-context-cli"

get_rust_binary() {
  if [ -f "$BINARY_PATH" ]; then
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
  exec "$LEGACY_SCRIPT" "$@"
}

# Run Rust implementation
run_rust_only() {
  local bin
  bin="$(get_rust_binary)"
  if [ $? -ne 0 ] || [ -z "$bin" ]; then
    echo "Warning: Rust binary not found and cargo build failed. Falling back to legacy shell script..." >&2
    run_legacy "$@"
  fi
  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  exec "$bin" "$@"
}

# Run Rust implementation with fallback to legacy on failure
run_rust_with_fallback() {
  local bin
  bin="$(get_rust_binary)"
  if [ $? -ne 0 ] || [ -z "$bin" ]; then
    echo "Warning: Rust binary not found. Falling back to legacy shell script..." >&2
    run_legacy "$@"
  fi

  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  
  # Run Rust binary and capture output and exit code
  # Note: we use a temp file to avoid pipe issues or memory limits for stdout
  local rust_out
  rust_out=$(mktemp)
  
  # Disable set -e temporarily to handle exit status
  set +e
  "$bin" "$@" > "$rust_out"
  local rust_exit=$?
  set -e

  if [ $rust_exit -eq 0 ]; then
    cat "$rust_out"
    rm -f "$rust_out"
    exit 0
  else
    rm -f "$rust_out"
    if [ -f "$LEGACY_SCRIPT" ]; then
      echo "Warning: Rust helper failed (exit code ${rust_exit}). Falling back to legacy Shell helper..." >&2
      exec "$LEGACY_SCRIPT" "$@"
    else
      echo "Error: Rust helper failed (exit code ${rust_exit}) and no legacy helper is available." >&2
      exit "$rust_exit"
    fi
  fi
}

# Run in shadow comparison mode
run_shadow() {
  local bin
  bin="$(get_rust_binary)"
  if [ $? -ne 0 ] || [ -z "$bin" ]; then
    echo "Warning: Rust binary not found for shadow mode. Running legacy shell script only..." >&2
    run_legacy "$@"
  fi

  if [ ! -f "$LEGACY_SCRIPT" ]; then
    echo "Warning: Legacy script not found for shadow mode. Running Rust only..." >&2
    export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
    exec "$bin" "$@"
  fi

  local legacy_out
  local rust_out
  legacy_out=$(mktemp)
  rust_out=$(mktemp)

  set +e
  export PRE_COMMIT_REVIEW_HELPER_PATH="$WRAPPER_SCRIPT"
  "$LEGACY_SCRIPT" "$@" > "$legacy_out" 2> /dev/null
  local legacy_exit=$?

  "$bin" "$@" > "$rust_out" 2> /dev/null
  local rust_exit=$?
  set -e

  # Compare outputs
  if ! diff -u "$legacy_out" "$rust_out" > /dev/null 2>&1; then
    echo "Warning: [Shadow Mode] Output mismatch detected between legacy shell and Rust implementation!" >&2
    # Optional: log the diff
    local diff_log="/tmp/collect_diff_context_shadow_diff.log"
    echo "=== DIFF MISMATCH ON $(date) ===" >> "$diff_log"
    echo "Args: $@" >> "$diff_log"
    diff -u "$legacy_out" "$rust_out" >> "$diff_log" 2>&1 || true
    echo "================================" >> "$diff_log"
  fi

  if [ $legacy_exit -ne $rust_exit ]; then
    echo "Warning: [Shadow Mode] Exit code mismatch! Legacy exit code: $legacy_exit, Rust exit code: $rust_exit" >&2
  fi

  # Return legacy output to guarantee safety during rollout
  cat "$legacy_out"
  rm -f "$legacy_out" "$rust_out"
  exit "$legacy_exit"
}

# Execute based on configured mode
if [ "$IMPL" = "legacy" ] || [ "$IMPL" = "shell" ]; then
  run_legacy "$@"
elif [ "$IMPL" = "shadow" ] || [ "$SHADOW_MODE" = "1" ]; then
  run_shadow "$@"
elif [ "${PRE_COMMIT_REVIEW_DISABLE_FALLBACK:-0}" = "1" ]; then
  run_rust_only "$@"
else
  run_rust_with_fallback "$@"
fi
