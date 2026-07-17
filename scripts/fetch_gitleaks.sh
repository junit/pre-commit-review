#!/usr/bin/env bash
# Fetch pinned upstream Gitleaks release binaries for release/install staging.
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
# shellcheck source=scripts/lib/gitleaks_integrity.sh
source "${SCRIPT_DIR}/lib/gitleaks_integrity.sh"
VERSION="$(tr -d '[:space:]' < "${SCRIPT_DIR}/gitleaks.version")"
CHECKSUMS_FILE="${SCRIPT_DIR}/gitleaks-assets.sha256"
BINARY_CHECKSUMS_FILE="${SCRIPT_DIR}/gitleaks-binaries.sha256"
DEST_DIR="${SCRIPT_DIR}/bin"
MODE='current'
REQUESTED_PLATFORM=''
PROGRESS_MODE="${PRE_COMMIT_REVIEW_FETCH_PROGRESS:-auto}"

usage() {
  cat <<'EOF'
Usage: scripts/fetch_gitleaks.sh [--all | --platform OS-ARCH] [--dest DIR]

Fetch pinned official Gitleaks assets after verifying archive and extracted-binary SHA256 values.

Platforms: darwin-arm64, darwin-amd64, linux-amd64, windows-amd64

Environment:
  PRE_COMMIT_REVIEW_FETCH_PROGRESS=auto|always|never
    Show download progress on a terminal (default), always, or never.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --all)
      MODE='all'
      shift
      ;;
    --platform)
      [ "$#" -ge 2 ] || { printf 'missing value for --platform\n' >&2; exit 2; }
      MODE='platform'
      REQUESTED_PLATFORM="$2"
      shift 2
      ;;
    --dest)
      [ "$#" -ge 2 ] || { printf 'missing value for --dest\n' >&2; exit 2; }
      DEST_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$PROGRESS_MODE" in
  auto|always|never) ;;
  *)
    printf 'invalid PRE_COMMIT_REVIEW_FETCH_PROGRESS value: %s (expected auto, always, or never)\n' \
      "$PROGRESS_MODE" >&2
    exit 2
    ;;
esac

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) CURRENT_OS='darwin' ;;
  linux) CURRENT_OS='linux' ;;
  msys*|mingw*|cygwin*) CURRENT_OS='windows' ;;
  *) printf 'unsupported operating system\n' >&2; exit 1 ;;
esac

case "$(uname -m)" in
  arm64|aarch64) CURRENT_ARCH='arm64' ;;
  x86_64|amd64) CURRENT_ARCH='amd64' ;;
  *) printf 'unsupported architecture\n' >&2; exit 1 ;;
esac

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

download() {
  local url="$1"
  local output="$2"
  if command -v curl >/dev/null 2>&1; then
    local -a curl_args=(-fL --show-error --retry 5 --retry-all-errors --connect-timeout 15)
    if [ "$PROGRESS_MODE" = 'always' ] || { [ "$PROGRESS_MODE" = 'auto' ] && [ -t 2 ]; }; then
      curl_args+=(--progress-bar)
    else
      curl_args+=(--silent)
    fi
    curl "${curl_args[@]}" "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    case "$PROGRESS_MODE" in
      always) wget --progress=bar:force:noscroll -O "$output" "$url" ;;
      auto)
        if [ -t 2 ]; then
          wget -O "$output" "$url"
        else
          wget -qO "$output" "$url"
        fi
        ;;
      never) wget -qO "$output" "$url" ;;
    esac
  else
    printf 'curl or wget is required to fetch Gitleaks\n' >&2
    return 1
  fi
}

fetch_platform() {
  local platform="$1"
  local asset
  local output_name
  local archive_kind
  case "$platform" in
    darwin-arm64)
      asset="gitleaks_${VERSION}_darwin_arm64.tar.gz"
      output_name='gitleaks-darwin-arm64'
      archive_kind='tar'
      ;;
    darwin-amd64)
      asset="gitleaks_${VERSION}_darwin_x64.tar.gz"
      output_name='gitleaks-darwin-amd64'
      archive_kind='tar'
      ;;
    linux-amd64)
      asset="gitleaks_${VERSION}_linux_x64.tar.gz"
      output_name='gitleaks-linux-amd64'
      archive_kind='tar'
      ;;
    windows-amd64)
      asset="gitleaks_${VERSION}_windows_x64.zip"
      output_name='gitleaks-windows-amd64.exe'
      archive_kind='zip'
      ;;
    *)
      printf 'unsupported Gitleaks platform: %s\n' "$platform" >&2
      return 1
      ;;
  esac

  local expected
  expected="$(awk -v asset="$asset" '$2 == asset {print $1}' "$CHECKSUMS_FILE")"
  [ -n "$expected" ] || { printf 'missing pinned checksum for %s\n' "$asset" >&2; return 1; }

  local archive="${TMP_DIR}/${asset}"
  local url="${PRE_COMMIT_REVIEW_GITLEAKS_BASE_URL:-https://github.com/gitleaks/gitleaks/releases/download/v${VERSION}}/${asset}"
  printf 'Downloading %s\n' "$asset"
  download "$url" "$archive"

  printf 'Verifying SHA256 for %s\n' "$asset"
  local actual
  actual="$(gitleaks_sha256_file "$archive")" || {
    printf 'sha256sum or shasum is required to verify Gitleaks\n' >&2
    return 1
  }
  [ "$actual" = "$expected" ] || {
    printf 'checksum mismatch for %s\nexpected: %s\nactual:   %s\n' "$asset" "$expected" "$actual" >&2
    return 1
  }

  local extract_dir="${TMP_DIR}/extract-${platform}"
  local extracted_binary
  printf 'Extracting %s\n' "$asset"
  mkdir -p "$extract_dir" "$DEST_DIR"
  if [ "$archive_kind" = 'tar' ]; then
    tar -xzf "$archive" -C "$extract_dir" gitleaks
    extracted_binary="$extract_dir/gitleaks"
  else
    command -v unzip >/dev/null 2>&1 || { printf 'unzip is required for Windows assets\n' >&2; return 1; }
    unzip -q "$archive" gitleaks.exe -d "$extract_dir"
    extracted_binary="$extract_dir/gitleaks.exe"
  fi

  printf 'Verifying binary SHA256 for %s\n' "$output_name"
  gitleaks_hash_matches "$extracted_binary" "$BINARY_CHECKSUMS_FILE" "$output_name" || {
    printf 'binary checksum mismatch for %s\n' "$output_name" >&2
    return 1
  }

  cp "$extracted_binary" "$DEST_DIR/$output_name"
  chmod +x "$DEST_DIR/$output_name"
  printf 'Installed %s\n' "$DEST_DIR/$output_name"
}

if [ "$MODE" = 'all' ]; then
  for platform in darwin-arm64 darwin-amd64 linux-amd64 windows-amd64; do
    fetch_platform "$platform"
  done
elif [ "$MODE" = 'platform' ]; then
  fetch_platform "$REQUESTED_PLATFORM"
else
  fetch_platform "${CURRENT_OS}-${CURRENT_ARCH}"
fi
