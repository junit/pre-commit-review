#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

platform_id() {
  local os_name arch_name
  case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
    darwin) os_name='darwin' ;;
    linux) os_name='linux' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch_name='arm64' ;;
    x86_64|amd64) arch_name='amd64' ;;
    *) return 1 ;;
  esac
  printf '%s-%s\n' "$os_name" "$arch_name"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

source_root="$tmp_dir/source"
mkdir -p "$source_root/collect-diff-context-cli" "$source_root/runtime/distribution"
cp "$repo_root/install.sh" "$repo_root/SKILL.md" "$repo_root/LICENSE" "$source_root/"
cp -R "$repo_root/agents" "$repo_root/references" "$repo_root/scripts" \
  "$repo_root/docs" "$repo_root/THIRD_PARTY_LICENSES" "$source_root/"
cp -R "$repo_root/collect-diff-context-cli/schemas" "$source_root/collect-diff-context-cli/"
for file in manifest.json revocations.json core-pack-manifest.json core-sbom.cdx.json; do
  printf '%s' '{}' >"$source_root/runtime/distribution/$file"
done

platform="$(platform_id)"
manager_name="collect_diff_context-${platform}"
case "$platform" in
  windows-*) manager_name="${manager_name}.exe" ;;
esac
manager="$source_root/scripts/bin/$manager_name"

cat >"$manager" <<'FAKE_MANAGER'
#!/usr/bin/env bash
set -euo pipefail

artifact_id=''
platform_id=''
target_root=''
cache_only='no'
operation="${1:-} ${2:-}"
shift 2 || true
while [ "$#" -gt 0 ]; do
  case "$1" in
    --artifact-id) artifact_id="$2"; shift 2 ;;
    --platform-id) platform_id="$2"; shift 2 ;;
    --target-root) target_root="$2"; shift 2 ;;
    --manifest) shift 2 ;;
    --no-download) cache_only='yes'; shift ;;
    *) shift ;;
  esac
done

if [ "$operation" = 'artifacts doctor' ]; then
  printf 'doctor:%s\n' "$target_root" >>"${FAKE_MANAGER_LOG:?}"
  printf '{"operation":"doctor","status":"completed"}'
  exit 0
fi

if [ "$artifact_id" != 'rust-analyzer' ]; then
  printf 'other:%s:%s\n' "$artifact_id" "$cache_only" >>"${FAKE_MANAGER_LOG:?}"
  printf '{"operation":"provision","status":"completed"}'
  exit 0
fi

printf 'provider:%s:%s\n' "$platform_id" "$cache_only" >>"${FAKE_MANAGER_LOG:?}"
case "${FAKE_PROVIDER_MODE:-success}" in
  success) ;;
  missing|corrupt|revoked|version-failure|probe-failure|wrong-platform|cache-miss)
    printf '{"operation":"provision","status":"failed","errors":[{"code":"fixture-%s"}]}' \
      "${FAKE_PROVIDER_MODE}" >&2
    exit 1
    ;;
  *) exit 2 ;;
esac

pack_version='2026.07.27-pcr.1'
pack_root="$target_root/runtime/third-party/rust-analyzer/$pack_version"
executable_name='rust-analyzer'
case "$platform_id" in
  windows-*) executable_name='rust-analyzer.exe' ;;
esac
mkdir -p "$pack_root/bin" "$pack_root/licenses" \
  "$target_root/runtime/artifact-receipts"
printf 'fixture rust-analyzer\n' >"$pack_root/bin/$executable_name"
chmod +x "$pack_root/bin/$executable_name"
printf '{}' >"$pack_root/pack-manifest.json"
printf '{}' >"$pack_root/sbom.cdx.json"
printf 'Apache fixture\n' >"$pack_root/licenses/LICENSE-APACHE"
printf 'MIT fixture\n' >"$pack_root/licenses/LICENSE-MIT"
printf '{}' >"$target_root/runtime/artifact-receipts/rust-analyzer.json"
printf '{"operation":"provision","status":"completed"}'
FAKE_MANAGER
chmod +x "$manager"

manager_log="$tmp_dir/manager.log"
: >"$manager_log"

run_install() {
  FAKE_MANAGER_LOG="$manager_log" FAKE_PROVIDER_MODE="${FAKE_PROVIDER_MODE:-success}" \
    "$source_root/install.sh" codex --copy --dir "$1" "${@:2}"
}

default_skills="$tmp_dir/default-skills"
run_install "$default_skills" >/dev/null
default_target="$default_skills/pre-commit-review"
[ ! -e "$default_target/runtime/third-party/rust-analyzer" ]
if grep -Fq 'provider:' "$manager_log"; then
  printf '%s\n' 'provider installer test failed: default install invoked rust-analyzer provisioning' >&2
  exit 1
fi

explicit_skills="$tmp_dir/explicit-skills"
run_install "$explicit_skills" --with-rust-analyzer >/dev/null
explicit_target="$explicit_skills/pre-commit-review"
pack_root="$explicit_target/runtime/third-party/rust-analyzer/2026.07.27-pcr.1"
provider_executable='rust-analyzer'
case "$platform" in
  windows-*) provider_executable='rust-analyzer.exe' ;;
esac
[ -x "$pack_root/bin/$provider_executable" ]
[ -f "$pack_root/pack-manifest.json" ]
[ -f "$pack_root/sbom.cdx.json" ]
[ -f "$pack_root/licenses/LICENSE-APACHE" ]
[ -f "$pack_root/licenses/LICENSE-MIT" ]
[ -f "$explicit_target/runtime/artifact-receipts/rust-analyzer.json" ]
[ -f "$explicit_target/runtime/distribution/manifest.json" ]
[ -f "$explicit_target/runtime/distribution/revocations.json" ]
[ -f "$explicit_target/runtime/distribution/core-pack-manifest.json" ]
grep -Fq "provider:${platform}:no" "$manager_log"

cache_skills="$tmp_dir/cache-skills"
run_install "$cache_skills" --with-rust-analyzer --no-download >/dev/null
grep -Fq "provider:${platform}:yes" "$manager_log"

link_parent="$tmp_dir/link-skills"
if run_install "$link_parent" --link --with-rust-analyzer \
  >"$tmp_dir/link.out" 2>"$tmp_dir/link.err"; then
  printf '%s\n' 'provider installer test failed: link mode accepted rust-analyzer' >&2
  exit 1
fi
[ ! -e "$link_parent" ]
grep -Fq -- '--with-rust-analyzer cannot be combined with --link' "$tmp_dir/link.err"

for mode in missing corrupt revoked version-failure probe-failure wrong-platform cache-miss; do
  failure_skills="$tmp_dir/failure-$mode"
  failure_target="$failure_skills/pre-commit-review"
  mkdir -p "$failure_target"
  cp "$source_root/SKILL.md" "$failure_target/SKILL.md"
  printf 'preserve-%s\n' "$mode" >"$failure_target/existing-target.bin"
  before="$(sha256_file "$failure_target/existing-target.bin")"
  if [ "$mode" = 'cache-miss' ]; then
    install_status=0
    FAKE_PROVIDER_MODE="$mode" run_install "$failure_skills" \
      --with-rust-analyzer --no-download \
      >"$tmp_dir/$mode.out" 2>"$tmp_dir/$mode.err" || install_status=$?
  else
    install_status=0
    FAKE_PROVIDER_MODE="$mode" run_install "$failure_skills" \
      --with-rust-analyzer \
      >"$tmp_dir/$mode.out" 2>"$tmp_dir/$mode.err" || install_status=$?
  fi
  if [ "$install_status" -eq 0 ]; then
    printf 'provider installer test failed: %s failure was accepted\n' "$mode" >&2
    exit 1
  fi
  after="$(sha256_file "$failure_target/existing-target.bin")"
  [ "$after" = "$before" ]
done

target_resolver="$explicit_target/scripts/lib/collect_diff_context_cli.sh"
cat >"$target_resolver" <<'TARGET_RESOLVER'
#!/usr/bin/env bash
resolve_packaged_collect_diff_context_cli() {
  printf '%s/bin/target-doctor\n' "$1"
}
TARGET_RESOLVER
target_manager="$explicit_target/scripts/bin/target-doctor"
cat >"$target_manager" <<'TARGET_MANAGER'
#!/usr/bin/env bash
set -euo pipefail
[ "${1:-} ${2:-}" = 'artifacts doctor' ]
[ "${3:-}" = '--target-root' ]
printf 'target-doctor:%s\n' "${4:?}" >>"${FAKE_MANAGER_LOG:?}"
printf '{"operation":"doctor","status":"completed"}'
TARGET_MANAGER
chmod +x "$target_manager"
doctor_status=0
FAKE_MANAGER_LOG="$manager_log" "$source_root/install.sh" \
  --doctor-target "$explicit_target" >"$tmp_dir/doctor.out" 2>"$tmp_dir/doctor.err" \
  || doctor_status=$?
if [ "$doctor_status" -ne 0 ]; then
  cat "$tmp_dir/doctor.err" >&2
  exit 1
fi
canonical_explicit_target="$(CDPATH='' cd -- "$explicit_target" && pwd -P)"
if ! grep -Fq "target-doctor:${canonical_explicit_target}" "$manager_log"; then
  printf '%s\n' 'provider installer test failed: doctor did not use the target collector' >&2
  cat "$manager_log" >&2
  exit 1
fi

printf '%s\n' 'rust-analyzer installer tests passed'
