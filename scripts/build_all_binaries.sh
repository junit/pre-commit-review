#!/usr/bin/env bash
# Build multi-platform release binaries for collect-diff-context-cli (Industrial Grade)
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
REPO_ROOT="$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd -P)"
CLI_DIR="${REPO_ROOT}/collect-diff-context-cli"
BIN_DIR="${REPO_ROOT}/scripts/bin"

mkdir -p "${BIN_DIR}"

echo "======================================================"
echo " Building Multi-Platform Industrial Release Binaries "
echo "======================================================"

# 1. macOS ARM64 & AMD64 (Native Cargo)
if [ "$(uname -s)" = "Darwin" ]; then
  echo "[1/4] Building macOS arm64 (aarch64-apple-darwin)..."
  (cd "${CLI_DIR}" && cargo build --release --target aarch64-apple-darwin >/dev/null)
  cp "${CLI_DIR}/target/aarch64-apple-darwin/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-darwin-arm64"

  echo "[2/4] Building macOS amd64 (x86_64-apple-darwin)..."
  (cd "${CLI_DIR}" && cargo build --release --target x86_64-apple-darwin >/dev/null)
  cp "${CLI_DIR}/target/x86_64-apple-darwin/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-darwin-amd64"
else
  echo "[1/4 & 2/4] Skipping macOS targets (not on macOS host)"
fi

# 3. Linux AMD64 (Docker MUSL for 100% Static Linking)
echo "[3/4] Building Linux amd64 (x86_64-unknown-linux-musl static binary)..."
if command -v cross >/dev/null 2>&1; then
  echo "      -> Using cross CLI"
  (cd "${CLI_DIR}" && cross build --release --target x86_64-unknown-linux-musl >/dev/null)
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-linux-amd64"
else
  echo "      -> Using Docker musl container"
  docker run --rm --platform linux/amd64 \
    -v "${REPO_ROOT}:/volume" \
    -w /volume/collect-diff-context-cli \
    rust:latest sh -c "rustup target add x86_64-unknown-linux-musl >/dev/null && apt-get update -qq && apt-get install -y --no-install-recommends musl-tools >/dev/null && cargo build --release --target x86_64-unknown-linux-musl >/dev/null"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-linux-amd64"
fi

# 4. Windows AMD64 (Native mingw if available, else Docker)
echo "[4/4] Building Windows amd64 (x86_64-pc-windows-gnu)..."
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  echo "      -> Using native mingw-w64 toolchain"
  (cd "${CLI_DIR}" && cargo build --release --target x86_64-pc-windows-gnu >/dev/null)
  cp "${CLI_DIR}/target/x86_64-pc-windows-gnu/release/collect-diff-context-cli.exe" "${BIN_DIR}/collect_diff_context-windows-amd64.exe"
else
  echo "      -> Fallback to Docker mingw-w64 container"
  docker run --rm --platform linux/amd64 \
    -v "${REPO_ROOT}:/volume" \
    -w /volume/collect-diff-context-cli \
    rust:latest sh -c "apt-get update -qq && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 >/dev/null && rustup target add x86_64-pc-windows-gnu >/dev/null && cargo build --release --target x86_64-pc-windows-gnu >/dev/null"
  cp "${CLI_DIR}/target/x86_64-pc-windows-gnu/release/collect-diff-context-cli.exe" "${BIN_DIR}/collect_diff_context-windows-amd64.exe"
fi

echo "Fetching pinned Gitleaks release binaries..."
"${SCRIPT_DIR}/fetch_gitleaks.sh" --all --dest "${BIN_DIR}"

echo "======================================================"
echo " All platform binaries successfully built!"
echo " Binaries updated in scripts/bin/ :"
ls -lh "${BIN_DIR}"
echo "======================================================"
