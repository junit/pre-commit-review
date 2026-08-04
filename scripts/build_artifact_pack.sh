#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
kind=''
platform=''
pack_version=''
source_root="$repo_root"
source_lock=''
manifest="$repo_root/third_party_artifacts/manifest.json"
revocations="$repo_root/third_party_artifacts/revocations.json"
output=''
binary=''
record_output=''
manifest_output=''

usage() {
  cat <<'EOF'
Usage: scripts/build_artifact_pack.sh --kind gitleaks|core --platform-id ID \
  --pack-version VERSION --output /absolute/pack.tar.gz [options]

Options:
  --source-root PATH    Payload root (default: repository root)
  --manifest PATH       Reviewed distribution manifest
  --revocations PATH    Reviewed revocation index (core only)
  --source-lock PATH    Checked-in Gitleaks source lock
  --binary PATH         Explicit Gitleaks executable
  --record-output PATH  Write canonical generated record metadata
  --manifest-output PATH
                        Write a canonical manifest containing the active record
EOF
}

absolute() {
  case "$1" in
    /*|[A-Za-z]:[\\/]*) return 0 ;;
    *) printf 'path must be absolute: %s\n' "$1" >&2; exit 2 ;;
  esac
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --kind) shift; [ "$#" -gt 0 ] || exit 2; kind="$1" ;;
    --platform-id) shift; [ "$#" -gt 0 ] || exit 2; platform="$1" ;;
    --pack-version) shift; [ "$#" -gt 0 ] || exit 2; pack_version="$1" ;;
    --source-root) shift; [ "$#" -gt 0 ] || exit 2; source_root="$1" ;;
    --source-lock) shift; [ "$#" -gt 0 ] || exit 2; source_lock="$1" ;;
    --manifest) shift; [ "$#" -gt 0 ] || exit 2; manifest="$1" ;;
    --revocations) shift; [ "$#" -gt 0 ] || exit 2; revocations="$1" ;;
    --output) shift; [ "$#" -gt 0 ] || exit 2; output="$1" ;;
    --binary) shift; [ "$#" -gt 0 ] || exit 2; binary="$1" ;;
    --record-output) shift; [ "$#" -gt 0 ] || exit 2; record_output="$1" ;;
    --manifest-output) shift; [ "$#" -gt 0 ] || exit 2; manifest_output="$1" ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$kind" in
  gitleaks|core) ;;
  *) printf 'unsupported pack kind: %s\n' "$kind" >&2; exit 2 ;;
esac
[ -n "$platform" ] && [ -n "$pack_version" ] && [ -n "$output" ] \
  || { usage >&2; exit 2; }
absolute "$source_root"
absolute "$manifest"
absolute "$output"
[ -z "$record_output" ] || absolute "$record_output"
[ -z "$manifest_output" ] || absolute "$manifest_output"

writer_args=(
  "$kind"
  --platform-id "$platform"
  --pack-version "$pack_version"
  --source-root "$source_root"
  --manifest "$manifest"
  --output "$output"
)
if [ -n "$record_output" ]; then
  writer_args+=(--record-output "$record_output")
fi
if [ -n "$manifest_output" ]; then
  [ "$kind" = 'gitleaks' ] || {
    printf '%s\n' '--manifest-output is only valid for Gitleaks packs' >&2
    exit 2
  }
  writer_args+=(--manifest-output "$manifest_output")
fi
if [ "$kind" = 'gitleaks' ]; then
  if [ -z "$source_lock" ]; then
    printf 'Gitleaks pack requires --source-lock\n' >&2
    exit 2
  fi
  if [ -z "$binary" ]; then
    suffix=''
    [ "$platform" != 'windows-amd64' ] || suffix='.exe'
    binary="$source_root/scripts/bin/gitleaks-${platform}${suffix}"
  fi
  absolute "$source_lock"
  absolute "$binary"
  writer_args+=(--source-lock "$source_lock" --binary "$binary")
else
  absolute "$revocations"
  writer_args+=(--revocations "$revocations")
fi

if [ -n "${PRE_COMMIT_REVIEW_PACK_WRITER:-}" ]; then
  absolute "$PRE_COMMIT_REVIEW_PACK_WRITER"
  exec "$PRE_COMMIT_REVIEW_PACK_WRITER" "${writer_args[@]}"
fi

exec cargo +1.95.0 run --quiet --locked \
  --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --bin artifact-pack-writer -- "${writer_args[@]}"
