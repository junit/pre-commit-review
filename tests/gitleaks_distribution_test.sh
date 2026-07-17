#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
version_file="$repo_root/scripts/gitleaks.version"
checksums_file="$repo_root/scripts/gitleaks-assets.sha256"
binary_checksums_file="$repo_root/scripts/gitleaks-binaries.sha256"
fetch_script="$repo_root/scripts/fetch_gitleaks.sh"

fail() {
  printf 'gitleaks distribution test failed: %s\n' "$*" >&2
  exit 1
}

version="$(tr -d '[:space:]' < "$version_file")"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || fail 'pinned version must be a semantic version'

expected_assets=(
  "gitleaks_${version}_darwin_arm64.tar.gz"
  "gitleaks_${version}_darwin_x64.tar.gz"
  "gitleaks_${version}_linux_x64.tar.gz"
  "gitleaks_${version}_windows_x64.zip"
)
expected_binaries=(
  'gitleaks-darwin-arm64'
  'gitleaks-darwin-amd64'
  'gitleaks-linux-amd64'
  'gitleaks-windows-amd64.exe'
)
expected_collectors=(
  'collect_diff_context-darwin-arm64'
  'collect_diff_context-darwin-amd64'
  'collect_diff_context-linux-amd64'
  'collect_diff_context-windows-amd64.exe'
)

[ "$(wc -l < "$checksums_file" | tr -d '[:space:]')" = "${#expected_assets[@]}" ] \
  || fail 'checksum manifest must contain exactly four platform assets'

for asset in "${expected_assets[@]}"; do
  line="$(awk -v asset="$asset" '$2 == asset {print}' "$checksums_file")"
  [ -n "$line" ] || fail "missing checksum for $asset"
  hash="${line%% *}"
  [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || fail "invalid SHA256 for $asset"
done

[ "$(wc -l < "$binary_checksums_file" | tr -d '[:space:]')" = "${#expected_binaries[@]}" ] \
  || fail 'binary checksum manifest must contain exactly four platform executables'
for binary in "${expected_binaries[@]}"; do
  line="$(awk -v binary="$binary" '$2 == binary {print}' "$binary_checksums_file")"
  [ -n "$line" ] || fail "missing binary checksum for $binary"
  hash="${line%% *}"
  [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || fail "invalid binary SHA256 for $binary"
done

for collector in "${expected_collectors[@]}"; do
  collector_path="$repo_root/scripts/bin/$collector"
  [ -x "$collector_path" ] || fail "missing executable helper artifact: $collector"
  if ! grep -aFq -- '--sanitize-stdin' "$collector_path" \
    && ! grep -aFq 'pcr-sanitizer-v1' "$collector_path"; then
    fail "helper artifact does not contain a sanitizer capability marker: $collector"
  fi
done

grep -Fq 'https://github.com/gitleaks/gitleaks/releases/download/' "$fetch_script" \
  || fail 'fetch script must use official Gitleaks releases by default'
grep -Fq 'PRE_COMMIT_REVIEW_FETCH_PROGRESS=auto|always|never' "$fetch_script" \
  || fail 'fetch script help must document download progress controls'
grep -Fq -- '--progress-bar' "$fetch_script" \
  || fail 'curl downloads must expose interactive progress'
# shellcheck disable=SC2016 # Assert the literal checksum comparison in the script.
grep -Fq '[ "$actual" = "$expected" ]' "$fetch_script" \
  || fail 'fetch script must verify each pinned checksum before extraction'
# shellcheck disable=SC2016 # Assert literal variable use in the fetch script.
grep -Fq 'gitleaks_hash_matches "$extracted_binary"' "$fetch_script" \
  || fail 'fetch script must verify the extracted executable before installation'
[ -f "$repo_root/references/security/gitleaks.toml" ] \
  || fail 'trusted scanner configuration is missing'
[ -f "$repo_root/THIRD_PARTY_LICENSES/gitleaks-LICENSE" ] \
  || fail 'bundled scanner license is missing'
[ -x "$repo_root/scripts/check_gitleaks.sh" ] \
  || fail 'Gitleaks doctor is missing or not executable'
[ -f "$repo_root/scripts/lib/gitleaks_integrity.sh" ] \
  || fail 'shared Gitleaks integrity helpers are missing'
if grep -Fq 'command -v gitleaks' \
  "$repo_root/install.sh" "$repo_root/scripts/collect_diff_context.sh"; then
  fail 'installer and runtime wrapper must not discover Gitleaks implicitly through PATH'
fi
if grep -Fq 'PathBuf::from("gitleaks")' \
  "$repo_root/collect-diff-context-cli/src/secret_scan.rs"; then
  fail 'Rust scanner discovery must not fall back implicitly to PATH'
fi
grep -Fq 'review_continued: yes' "$repo_root/scripts/collect_diff_context.sh" \
  || fail 'runtime wrapper must report that review continues when redaction is unavailable'
if grep -Eq 'diff_release_allowed: no|secret_scan: blocked' \
  "$repo_root/scripts/collect_diff_context.sh"; then
  fail 'optional secret scanning must not withhold review output'
fi
grep -Fq -- '--sanitize-stdin' "$repo_root/scripts/collect_diff_context.sh" \
  || fail 'runtime wrapper must sanitize captured streams through the Rust helper'
grep -Fq 'dist/pre-commit-review/scripts/check_gitleaks.sh' \
  "$repo_root/.github/workflows/release.yml" \
  || fail 'release package must run the Gitleaks doctor before archiving'

invalid_progress_output="$(mktemp)"
trap 'rm -f "$invalid_progress_output"' EXIT
if PRE_COMMIT_REVIEW_FETCH_PROGRESS=invalid \
  "$fetch_script" --platform unsupported >"$invalid_progress_output" 2>&1; then
  fail 'fetch script must reject an invalid progress mode'
fi
grep -Fq 'invalid PRE_COMMIT_REVIEW_FETCH_PROGRESS value' "$invalid_progress_output" \
  || fail 'invalid progress mode must have an actionable error'

printf 'gitleaks distribution tests passed\n'
