#!/usr/bin/env bash
set -euo pipefail

# Collect Git diff context for the pre-commit-review skill by executing
# the compiled cross-platform Rust binary.

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
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

# Development fallback: If binary is not present (e.g. during clean development),
# trigger a local cargo compilation.
if [ ! -f "$BINARY_PATH" ]; then
  CARGO_RELEASE_BIN="${SCRIPT_DIR}/../collect-diff-context-cli/target/release/collect-diff-context-cli"
  if [ -f "$CARGO_RELEASE_BIN" ]; then
    exec "$CARGO_RELEASE_BIN" "$@"
  else
    # Build it
    if command -v cargo >/dev/null 2>&1; then
      echo "Pre-compiled binary not found. Compiling from source..." >&2
      (cd "${SCRIPT_DIR}/../collect-diff-context-cli" && cargo build --release >/dev/null 2>&1)
      if [ -f "$CARGO_RELEASE_BIN" ]; then
        exec "$CARGO_RELEASE_BIN" "$@"
      fi
    fi
    echo "Error: collect_diff_context binary not found at $BINARY_PATH" >&2
    exit 1
  fi
fi

export PRE_COMMIT_REVIEW_HELPER_PATH="${SCRIPT_DIR}/collect_diff_context.sh"
exec "$BINARY_PATH" "$@"

