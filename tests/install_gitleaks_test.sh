#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'install Gitleaks test failed: %s\n' "$*" >&2
  exit 1
}

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) os_name='darwin' ;;
  linux) os_name='linux' ;;
  *) printf 'install Gitleaks test skipped on this operating system\n'; exit 0 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch_name='arm64' ;;
  x86_64|amd64) arch_name='amd64' ;;
  *) printf 'install Gitleaks test skipped on this architecture\n'; exit 0 ;;
esac

platform="${os_name}-${arch_name}"
binary_name="gitleaks-${platform}"
collector_name="collect_diff_context-${platform}"
version="$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")"
case "$platform" in
  darwin-arm64) asset="gitleaks_${version}_darwin_arm64.tar.gz" ;;
  darwin-amd64) asset="gitleaks_${version}_darwin_x64.tar.gz" ;;
  linux-amd64) asset="gitleaks_${version}_linux_x64.tar.gz" ;;
  *) printf 'install Gitleaks test skipped on %s\n' "$platform"; exit 0 ;;
esac

fixture_root="$tmp_dir/source"
mkdir -p "$fixture_root"
cp "$repo_root/install.sh" "$repo_root/SKILL.md" "$repo_root/LICENSE" "$fixture_root/"
cp -R "$repo_root/agents" "$repo_root/references" "$repo_root/scripts" \
  "$repo_root/THIRD_PARTY_LICENSES" "$fixture_root/"
rm -f "$fixture_root/scripts/bin"/gitleaks-*

asset_dir="$tmp_dir/assets"
payload_dir="$tmp_dir/payload"
mkdir -p "$asset_dir" "$payload_dir"
cat > "$payload_dir/gitleaks" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$version'
  exit 0
fi
cat >/dev/null
printf '%s\n' '[]'
EOF
chmod +x "$payload_dir/gitleaks"
tar -czf "$asset_dir/$asset" -C "$payload_dir" gitleaks

if command -v sha256sum >/dev/null 2>&1; then
  asset_hash="$(sha256sum "$asset_dir/$asset" | awk '{print $1}')"
else
  asset_hash="$(shasum -a 256 "$asset_dir/$asset" | awk '{print $1}')"
fi
awk -v asset="$asset" -v hash="$asset_hash" \
  'BEGIN { OFS="  " } $2 == asset { $1=hash } { print $1, $2 }' \
  "$fixture_root/scripts/gitleaks-assets.sha256" \
  > "$tmp_dir/checksums"
mv "$tmp_dir/checksums" "$fixture_root/scripts/gitleaks-assets.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  binary_hash="$(sha256sum "$payload_dir/gitleaks" | awk '{print $1}')"
else
  binary_hash="$(shasum -a 256 "$payload_dir/gitleaks" | awk '{print $1}')"
fi
awk -v name="$binary_name" -v hash="$binary_hash" \
  'BEGIN { OFS="  " } $2 == name { $1=hash } { print $1, $2 }' \
  "$fixture_root/scripts/gitleaks-binaries.sha256" \
  > "$tmp_dir/binary-checksums"
mv "$tmp_dir/binary-checksums" "$fixture_root/scripts/gitleaks-binaries.sha256"

base_url="file://$asset_dir"

dry_output="$tmp_dir/dry-run.out"
PRE_COMMIT_REVIEW_GITLEAKS_BASE_URL="$base_url" \
  "$fixture_root/install.sh" codex --dry-run --dir "$tmp_dir/dry-skills" \
  > "$dry_output"
grep -Fq "DRY RUN fetch pinned Gitleaks for $platform" "$dry_output" \
  || fail 'dry-run did not report the current-platform download'
[ ! -e "$tmp_dir/dry-skills/pre-commit-review" ] \
  || fail 'dry-run changed the filesystem'

copy_output="$tmp_dir/copy.out"
copy_progress="$tmp_dir/copy.progress"
PRE_COMMIT_REVIEW_GITLEAKS_BASE_URL="$base_url" \
  PRE_COMMIT_REVIEW_FETCH_PROGRESS=always \
  "$fixture_root/install.sh" codex --dir "$tmp_dir/copy-skills" \
  > "$copy_output" 2> "$copy_progress"
installed_binary="$tmp_dir/copy-skills/pre-commit-review/scripts/bin/$binary_name"
installed_collector="$tmp_dir/copy-skills/pre-commit-review/scripts/bin/$collector_name"
[ -x "$installed_binary" ] || fail 'default copy install did not bundle Gitleaks'
[ -x "$installed_collector" ] || fail 'default copy install did not bundle the current helper'
[ "$("$installed_binary" version)" = "$version" ] \
  || fail 'default copy install bundled the wrong Gitleaks version'
[ ! -e "$fixture_root/scripts/bin/$binary_name" ] \
  || fail 'copy install should not modify the source tree'
grep -Fq "Gitleaks: installed pinned $binary_name" "$copy_output" \
  || fail 'copy install did not report the pinned scanner installation'
[ -s "$copy_progress" ] \
  || fail 'forced download progress did not write to stderr'
grep -Fq "Downloading $asset" "$copy_output" \
  || fail 'download stage was not reported'
grep -Fq "Verifying SHA256 for $asset" "$copy_output" \
  || fail 'checksum stage was not reported'
grep -Fq "Extracting $asset" "$copy_output" \
  || fail 'extraction stage was not reported'

installed_report="$tmp_dir/installed-sanitizer.report"
PRE_COMMIT_REVIEW_SANITIZE_REPORT="$installed_report" \
  "$installed_collector" --sanitize-stdin </dev/null >"$tmp_dir/installed-sanitizer.out"
grep -Fq 'protocol: pcr-sanitizer-v1' "$installed_report" \
  || fail 'copy-installed helper does not support the sanitizer protocol'
grep -Fq 'status: clean' "$installed_report" \
  || fail 'copy-installed helper did not complete the bundled scanner handshake'

offline_bin="$tmp_dir/offline-bin"
mkdir -p "$offline_bin"
cp "$payload_dir/gitleaks" "$offline_bin/gitleaks"

PATH="$offline_bin:$PATH" \
  "$fixture_root/install.sh" codex --no-download --dir "$tmp_dir/missing-skills" \
  > "$tmp_dir/missing.out" 2> "$tmp_dir/missing.err"
[ -d "$tmp_dir/missing-skills/pre-commit-review" ] \
  || fail '--no-download did not install the skill without Gitleaks'
[ ! -e "$tmp_dir/missing-skills/pre-commit-review/scripts/bin/$binary_name" ] \
  || fail '--no-download unexpectedly bundled a scanner'
grep -Fq 'optional scanner unavailable (--no-download); review will continue without secret redaction' \
  "$tmp_dir/missing.out" \
  || fail '--no-download did not explain the optional redaction downgrade'

PRE_COMMIT_REVIEW_GITLEAKS_BIN="$offline_bin/gitleaks" \
  "$fixture_root/install.sh" codex --no-download --dir "$tmp_dir/offline-skills" \
  > "$tmp_dir/offline.out"
[ ! -e "$tmp_dir/offline-skills/pre-commit-review/scripts/bin/$binary_name" ] \
  || fail '--no-download unexpectedly bundled a scanner'
grep -Fq 'explicitly trusted' "$tmp_dir/offline.out" \
  || fail '--no-download did not report its explicit scanner dependency'

cat > "$fixture_root/scripts/bin/$binary_name" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '0.0.0'
EOF
chmod +x "$fixture_root/scripts/bin/$binary_name"

link_output="$tmp_dir/link.out"
PRE_COMMIT_REVIEW_GITLEAKS_BASE_URL="$base_url" \
  "$fixture_root/install.sh" codex --link --dir "$tmp_dir/link-skills" \
  > "$link_output"
[ -x "$fixture_root/scripts/bin/$binary_name" ] \
  || fail 'link install did not bundle Gitleaks in the linked source tree'
[ "$("$fixture_root/scripts/bin/$binary_name" version)" = "$version" ] \
  || fail 'link install did not replace a stale bundled Gitleaks version'
[ -L "$tmp_dir/link-skills/pre-commit-review" ] \
  || fail 'link install did not create the skill symlink'
grep -Fq "Gitleaks: installed pinned $binary_name" "$link_output" \
  || fail 'link install did not report the pinned scanner installation'

doctor_output="$tmp_dir/doctor.out"
"$fixture_root/install.sh" --doctor > "$doctor_output"
grep -Fq 'Gitleaks doctor: OK' "$doctor_output" \
  || fail 'doctor did not accept a verified bundled scanner'
grep -Fq 'integrity: sha256-verified' "$doctor_output" \
  || fail 'doctor did not report bundled scanner integrity'

printf '\n# tampered\n' >> "$fixture_root/scripts/bin/$binary_name"
if "$fixture_root/install.sh" --doctor > "$tmp_dir/tampered.out" 2>&1; then
  fail 'doctor accepted a tampered bundled scanner'
fi
grep -Fq 'reason: binary-integrity-mismatch' "$tmp_dir/tampered.out" \
  || fail 'doctor did not identify bundled scanner tampering'
grep -Fq 'Gitleaks doctor: UNAVAILABLE' "$tmp_dir/tampered.out" \
  || fail 'doctor did not distinguish scanner unavailability from review availability'
grep -Fq 'redaction_available: no' "$tmp_dir/tampered.out" \
  || fail 'doctor did not report that redaction is unavailable'
grep -Fq 'review_output_allowed: yes' "$tmp_dir/tampered.out" \
  || fail 'doctor incorrectly implied that review output is blocked'

printf 'install Gitleaks tests passed\n'
