#!/usr/bin/env bash
# Build multi-platform release binaries for collect-diff-context-cli (Industrial Grade)
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
REPO_ROOT="$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd -P)"
CLI_DIR="${REPO_ROOT}/collect-diff-context-cli"
BIN_DIR="${REPO_ROOT}/scripts/bin"
PACK_DIR="${REPO_ROOT}/dist"
CORE_PACK_VERSION="${CORE_PACK_VERSION:-0.1.0}"
GITLEAKS_PACK_VERSION="${GITLEAKS_PACK_VERSION:-8.30.1-pcr.1}"

mkdir -p "${BIN_DIR}"

smoke_host_repository_context() {
  local os_name arch_name suffix='' repository_binary provider_binary
  case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
    darwin) os_name='darwin' ;;
    linux) os_name='linux' ;;
    msys*|mingw*|cygwin*) os_name='windows'; suffix='.exe' ;;
    *)
      echo "Skipping repository-context smoke test on unsupported host OS"
      return 0
      ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch_name='arm64' ;;
    x86_64|amd64) arch_name='amd64' ;;
    *)
      echo "Skipping repository-context smoke test on unsupported host architecture"
      return 0
      ;;
  esac

  repository_binary="${BIN_DIR}/repository_context-${os_name}-${arch_name}${suffix}"
  if [ ! -x "${repository_binary}" ]; then
    echo "Skipping repository-context smoke test; no host-compatible binary was built"
    return 0
  fi

  echo "Smoke-testing host repository-context binary..."
  "${repository_binary}" collect --help >/dev/null
  "${repository_binary}" index --help >/dev/null

  provider_binary="${BIN_DIR}/repository_context_provider-${os_name}-${arch_name}${suffix}"
  if [ ! -x "${provider_binary}" ]; then
    echo "Skipping repository-context provider smoke test; no host-compatible binary was built"
    return 0
  fi
  echo "Smoke-testing host repository-context provider binary..."
  "${provider_binary}" --help >/dev/null
}

echo "======================================================"
echo " Building Multi-Platform Industrial Release Binaries "
echo "======================================================"

# 1. macOS ARM64 & AMD64 (Native Cargo)
if [ "$(uname -s)" = "Darwin" ]; then
  echo "[1/4] Building macOS arm64 (aarch64-apple-darwin)..."
  (cd "${CLI_DIR}" && cargo +1.95.0 build --release --locked --target aarch64-apple-darwin --bins >/dev/null)
  cp "${CLI_DIR}/target/aarch64-apple-darwin/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-darwin-arm64"
  cp "${CLI_DIR}/target/aarch64-apple-darwin/release/static-analysis-cli" "${BIN_DIR}/static_analysis-darwin-arm64"
  cp "${CLI_DIR}/target/aarch64-apple-darwin/release/repository-context-cli" "${BIN_DIR}/repository_context-darwin-arm64"
  cp "${CLI_DIR}/target/aarch64-apple-darwin/release/repository-context-provider-cli" "${BIN_DIR}/repository_context_provider-darwin-arm64"

  echo "[2/4] Building macOS amd64 (x86_64-apple-darwin)..."
  (cd "${CLI_DIR}" && cargo +1.95.0 build --release --locked --target x86_64-apple-darwin --bins >/dev/null)
  cp "${CLI_DIR}/target/x86_64-apple-darwin/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-darwin-amd64"
  cp "${CLI_DIR}/target/x86_64-apple-darwin/release/static-analysis-cli" "${BIN_DIR}/static_analysis-darwin-amd64"
  cp "${CLI_DIR}/target/x86_64-apple-darwin/release/repository-context-cli" "${BIN_DIR}/repository_context-darwin-amd64"
  cp "${CLI_DIR}/target/x86_64-apple-darwin/release/repository-context-provider-cli" "${BIN_DIR}/repository_context_provider-darwin-amd64"
else
  echo "[1/4 & 2/4] Skipping macOS targets (not on macOS host)"
fi

# 3. Linux AMD64 (Docker MUSL for 100% Static Linking)
echo "[3/4] Building Linux amd64 (x86_64-unknown-linux-musl static binary)..."
if command -v cross >/dev/null 2>&1; then
  echo "      -> Using cross CLI"
  (cd "${CLI_DIR}" && cross +1.95.0 build --release --locked --target x86_64-unknown-linux-musl --bins >/dev/null)
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/static-analysis-cli" "${BIN_DIR}/static_analysis-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/repository-context-cli" "${BIN_DIR}/repository_context-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/repository-context-provider-cli" "${BIN_DIR}/repository_context_provider-linux-amd64"
else
  echo "      -> Using Docker musl container"
  docker run --rm --platform linux/amd64 \
    -v "${REPO_ROOT}:/volume" \
    -w /volume/collect-diff-context-cli \
    rust:latest sh -c "rustup toolchain install 1.95.0 >/dev/null && rustup target add --toolchain 1.95.0 x86_64-unknown-linux-musl >/dev/null && apt-get update -qq && apt-get install -y --no-install-recommends musl-tools >/dev/null && cargo +1.95.0 build --release --locked --target x86_64-unknown-linux-musl --bins >/dev/null"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/collect-diff-context-cli" "${BIN_DIR}/collect_diff_context-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/static-analysis-cli" "${BIN_DIR}/static_analysis-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/repository-context-cli" "${BIN_DIR}/repository_context-linux-amd64"
  cp "${CLI_DIR}/target/x86_64-unknown-linux-musl/release/repository-context-provider-cli" "${BIN_DIR}/repository_context_provider-linux-amd64"
fi

# 4. Windows AMD64 (MSVC; available on a Windows release runner)
echo "[4/4] Building Windows amd64 (x86_64-pc-windows-msvc)..."
case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  msys*|mingw*|cygwin*)
    (cd "${CLI_DIR}" && cargo +1.95.0 build --release --locked --target x86_64-pc-windows-msvc --bins >/dev/null)
    cp "${CLI_DIR}/target/x86_64-pc-windows-msvc/release/collect-diff-context-cli.exe" "${BIN_DIR}/collect_diff_context-windows-amd64.exe"
    cp "${CLI_DIR}/target/x86_64-pc-windows-msvc/release/static-analysis-cli.exe" "${BIN_DIR}/static_analysis-windows-amd64.exe"
    cp "${CLI_DIR}/target/x86_64-pc-windows-msvc/release/repository-context-cli.exe" "${BIN_DIR}/repository_context-windows-amd64.exe"
    cp "${CLI_DIR}/target/x86_64-pc-windows-msvc/release/repository-context-provider-cli.exe" "${BIN_DIR}/repository_context_provider-windows-amd64.exe"
    ;;
  *)
    echo "Skipping Windows MSVC target; build it on the Windows release runner"
    ;;
esac

smoke_host_repository_context

if [ -x "${SCRIPT_DIR}/build_artifact_pack.sh" ]; then
  "${SCRIPT_DIR}/build_artifact_pack.sh" --help >/dev/null
fi

echo "Fetching pinned Gitleaks release binaries..."
"${SCRIPT_DIR}/fetch_gitleaks.sh" --all --dest "${BIN_DIR}"

echo "Building normalized core and Gitleaks packs..."
mkdir -p "${PACK_DIR}"
platform_manifest="${PACK_DIR}/manifest.json"
cp "${REPO_ROOT}/third_party_artifacts/manifest.json" "${platform_manifest}"
for platform in darwin-amd64 darwin-arm64 linux-amd64 windows-amd64; do
  suffix=''
  if [ "${platform}" = 'windows-amd64' ]; then
    suffix='.exe'
  fi
  if [ ! -x "${BIN_DIR}/collect_diff_context-${platform}${suffix}" ] \
    || [ ! -x "${BIN_DIR}/static_analysis-${platform}${suffix}" ] \
    || [ ! -x "${BIN_DIR}/repository_context-${platform}${suffix}" ] \
    || [ ! -x "${BIN_DIR}/repository_context_provider-${platform}${suffix}" ]; then
    echo "Skipping ${platform} packs; platform project binaries are unavailable"
    continue
  fi
  gitleaks_pack="${PACK_DIR}/pre-commit-review-gitleaks-${GITLEAKS_PACK_VERSION}-${platform}.tar.gz"
  gitleaks_record="${PACK_DIR}/gitleaks-${platform}.record.json"
  "${SCRIPT_DIR}/build_artifact_pack.sh" \
    --kind gitleaks \
    --platform-id "${platform}" \
    --pack-version "${GITLEAKS_PACK_VERSION}" \
    --source-root "${REPO_ROOT}" \
    --manifest "${platform_manifest}" \
    --source-lock "${REPO_ROOT}/third_party_artifacts/sources/gitleaks-8.30.1.json" \
    --binary "${BIN_DIR}/gitleaks-${platform}${suffix}" \
    --output "${gitleaks_pack}" \
    --record-output "${gitleaks_record}" \
    --manifest-output "${platform_manifest}" >/dev/null
  "${SCRIPT_DIR}/build_artifact_pack.sh" \
    --kind core \
    --platform-id "${platform}" \
    --pack-version "${CORE_PACK_VERSION}" \
    --source-root "${REPO_ROOT}" \
    --manifest "${platform_manifest}" \
    --revocations "${REPO_ROOT}/third_party_artifacts/revocations.json" \
    --output "${PACK_DIR}/pre-commit-review-core-${CORE_PACK_VERSION}-${platform}.tar.gz" \
    --record-output "${PACK_DIR}/core-${platform}.record.json" >/dev/null
done

echo "======================================================"
echo " All platform binaries successfully built!"
echo " Binaries updated in scripts/bin/ :"
ls -lh "${BIN_DIR}"
echo " Release packs written to dist/ :"
ls -lh "${PACK_DIR}"/*.tar.gz "${PACK_DIR}"/*.record.json
echo "======================================================"
