#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

release_repository='junit/pre-commit-review'
release_tag='artifact-rust-analyzer-2026.07.27-pcr.3'
release_ref='refs/tags/artifact-rust-analyzer-2026.07.27-pcr.3'
pack_version='2026.07.27-pcr.3'
release_run='30563800815'
release_commit='c1ec955f447eb171554c1a7efad288dcbe51bbea'
source_lock_sha256='298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862'

tmp_dir="$(mktemp -d)"
cleanup() {
  if [ -n "${tmp_dir:-}" ] && [ -d "$tmp_dir" ]; then
    rm -rf -- "$tmp_dir"
  fi
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'provider real-server test failed: %s\n' "$1" >&2
  exit 1
}

baseline_runner=''
baseline_runner_class=''
baseline_evidence_output=''
baseline_runner_sha256=''
baseline_values=0
while [ "$#" -gt 0 ]; do
  [ "$#" -ge 2 ] || fail 'every hosted baseline option requires one value'
  option="$1"
  value="$2"
  [ -n "$value" ] || fail 'hosted baseline option values cannot be empty'
  case "$option" in
    --baseline-runner)
      [ -z "$baseline_runner" ] || fail 'hosted baseline runner is duplicated'
      baseline_runner="$value"
      ;;
    --baseline-runner-class)
      [ -z "$baseline_runner_class" ] || fail 'hosted baseline runner class is duplicated'
      baseline_runner_class="$value"
      ;;
    --baseline-evidence-output)
      [ -z "$baseline_evidence_output" ] || fail 'hosted baseline evidence output is duplicated'
      baseline_evidence_output="$value"
      ;;
    --baseline-runner-sha256)
      [ -z "$baseline_runner_sha256" ] || fail 'hosted baseline runner digest is duplicated'
      baseline_runner_sha256="$value"
      ;;
    *) fail "unknown provider real-server option: $option" ;;
  esac
  baseline_values=$((baseline_values + 1))
  shift 2
done
case "$baseline_values" in
  0|4) ;;
  *) fail 'hosted baseline inputs must be provided together' ;;
esac

require_tool() {
  command -v "$1" >/dev/null 2>&1 || fail "required tool is unavailable: $1"
}

native_path() {
  local converted
  if [ "$platform" = 'windows-amd64' ]; then
    converted="$(cygpath -aw "$1")" || fail "cannot convert path for Windows: $1"
    python3 - "$converted" <<'PY' || fail "cygpath returned a non-native absolute Windows path: $converted"
import re
import sys

value = sys.argv[1]
drive_path = re.fullmatch(r'[A-Za-z]:[\\/].*', value)
unc_path = re.fullmatch(r'(?:\\\\|//)[^\\/]+[\\/][^\\/]+(?:[\\/].*)?', value)
if drive_path is None and unc_path is None:
    raise SystemExit(1)
PY
    printf '%s\n' "$converted"
    return
  fi
  python3 - "$1" <<'PY'
import sys
from pathlib import Path

print(Path(sys.argv[1]).resolve())
PY
}

detect_platform() {
  local os_name arch_name libc
  case "$(uname -s)" in
    Darwin) os_name='darwin' ;;
    Linux) os_name='linux' ;;
    MSYS*|MINGW*|CYGWIN*) os_name='windows' ;;
    *) fail "unsupported host operating system: $(uname -s)" ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch_name='amd64' ;;
    arm64|aarch64) arch_name='arm64' ;;
    *) fail "unsupported host architecture: $(uname -m)" ;;
  esac

  case "$os_name-$arch_name" in
    darwin-amd64|darwin-arm64|windows-amd64) ;;
    linux-amd64)
      command -v getconf >/dev/null 2>&1 || fail 'linux-amd64 requires glibc 2.28 or newer'
      libc="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
      python3 - "$libc" <<'PY' || fail 'linux-amd64 requires glibc 2.28 or newer; musl and unknown libc are unsupported'
import re
import sys

match = re.fullmatch(r'glibc ([0-9]+)\.([0-9]+)', sys.argv[1].strip())
if match is None or tuple(map(int, match.groups())) < (2, 28):
    raise SystemExit(1)
PY
      ;;
    *) fail "unsupported provider platform: $os_name-$arch_name" ;;
  esac
  printf '%s-%s\n' "$os_name" "$arch_name"
}

snapshot_target() {
  python3 - "$1" "$2" <<'PY'
import hashlib
import json
import os
import stat
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
output = Path(sys.argv[2])
files = []
for path in sorted(root.rglob('*')):
    relative = path.relative_to(root).as_posix()
    mode = path.lstat().st_mode
    if stat.S_ISLNK(mode):
        raise SystemExit(f'target contains a symbolic link: {relative}')
    if stat.S_ISREG(mode):
        raw = path.read_bytes()
        files.append({
            'path': relative,
            'size': len(raw),
            'sha256': hashlib.sha256(raw).hexdigest(),
        })
value = {'files': files}
output.write_bytes(json.dumps(value, separators=(',', ':')).encode())
PY
}

require_tool gh
require_tool git
require_tool cargo
require_tool python3

case "$release_tag:$pack_version" in
  *pcr.1*|*pcr.2*|*latest*|*nightly*) fail 'release identity is not the exact pcr.3 publication' ;;
esac
[ "$release_tag" = "artifact-rust-analyzer-$pack_version" ] || fail 'release tag and pack version differ'

platform="$(detect_platform)"
case "$platform" in
  darwin-amd64|darwin-arm64|linux-amd64|windows-amd64) ;;
  *) fail "internal unsupported platform mapping: $platform" ;;
esac
if [ "$platform" = 'windows-amd64' ]; then
  require_tool cygpath
fi

run_json="$tmp_dir/workflow-run.json"
gh run view "$release_run" --repo "$release_repository" \
  --json status,conclusion,headSha,event,headBranch,url >"$run_json"
python3 - "$run_json" "$release_run" "$release_tag" "$release_commit" <<'PY'
import json
import sys
from pathlib import Path

value = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
expected = {
    'status': 'completed',
    'conclusion': 'success',
    'headSha': sys.argv[4],
    'event': 'push',
    'headBranch': sys.argv[3],
    'url': f'https://github.com/junit/pre-commit-review/actions/runs/{sys.argv[2]}',
}
if value != expected:
    raise SystemExit(f'workflow run identity differs: {value!r}')
PY

release_owned='yes'
if [ -n "${PCR_PROVIDER_RELEASE_ROOT:-}" ]; then
  case "$PCR_PROVIDER_RELEASE_ROOT" in
    /*|[A-Za-z]:[\\/]*) ;;
    *) fail 'PCR_PROVIDER_RELEASE_ROOT must be absolute' ;;
  esac
  [ -d "$PCR_PROVIDER_RELEASE_ROOT" ] || fail 'PCR_PROVIDER_RELEASE_ROOT is not a directory'
  release_root="$(CDPATH='' cd -- "$PCR_PROVIDER_RELEASE_ROOT" && pwd -P)"
  release_owned='no'
else
  release_root="$tmp_dir/release"
  mkdir -p "$release_root"
  gh release download "$release_tag" --repo "$release_repository" --dir "$release_root"
fi

GITHUB_REF="$release_ref" GITHUB_SHA="$release_commit" \
  bash "$repo_root/scripts/verify_provider_release.sh" \
  --signed-release-root "$release_root" >/dev/null

harness_root="$tmp_dir/harness"
cache_root="$tmp_dir/cache"
target_root="$tmp_dir/target"
sentinel_root="$tmp_dir/fallback-sentinel"
mkdir -p "$harness_root" "$cache_root" "$target_root" \
  "$sentinel_root/bin" "$sentinel_root/home" \
  "$sentinel_root/cargo-home" "$sentinel_root/rustup-home"

manifest_path="$harness_root/candidate-manifest.json"
pack_path="$release_root/pre-commit-review-rust-analyzer-$pack_version-$platform.tar.gz"
[ -f "$pack_path" ] || fail "current-platform pack is absent: $platform"

python3 - \
  "$repo_root" "$release_root" "$platform" "$manifest_path" "$pack_path" \
  "$release_repository" "$release_tag" "$release_ref" "$pack_version" \
  "$release_commit" "$source_lock_sha256" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

(
    repo_root_raw, release_root_raw, platform, manifest_raw, pack_raw,
    repository, release_tag, release_ref, pack_version, release_commit,
    expected_source_lock_sha256,
) = sys.argv[1:]
repo_root = Path(repo_root_raw).resolve()
release_root = Path(release_root_raw).resolve()
manifest_path = Path(manifest_raw)
pack_path = Path(pack_raw).resolve()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def read_canonical(path):
    raw = path.read_bytes()
    value = json.loads(raw.decode('utf-8'))
    if json.dumps(value, separators=(',', ':')).encode() != raw:
        raise SystemExit(f'noncanonical JSON: {path.name}')
    return value

def read_json(path):
    return json.loads(path.read_text(encoding='utf-8'))

source_lock_path = repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json'
revocations_path = repo_root / 'third_party_artifacts/revocations.json'
baseline_path = repo_root / 'tests/fixtures/provider-release/reviewed-baseline.json'
release_path = release_root / f'rust-analyzer-{platform}.release.json'
metadata_path = release_root / f'rust-analyzer-{platform}.metadata.json'
pack_manifest_path = release_root / f'rust-analyzer-{platform}.pack-manifest.json'
sbom_path = release_root / f'rust-analyzer-{platform}.sbom.cdx.json'

source_lock = read_canonical(source_lock_path)
revocations = read_canonical(revocations_path)
baseline = read_canonical(baseline_path)
release = read_canonical(release_path)
metadata = read_json(metadata_path)
pack_manifest = read_canonical(pack_manifest_path)
sbom = read_canonical(sbom_path)

source_digest = digest(source_lock_path)
if source_digest != expected_source_lock_sha256:
    raise SystemExit('source lock is not the reviewed pcr.3 byte sequence')
if source_lock.get('artifact_id') != 'rust-analyzer' or source_lock.get('tool_version') != '2026-07-27':
    raise SystemExit('source lock identity differs')
try:
    source_asset = next(item for item in source_lock['assets'] if item['platform_id'] == platform)
except (KeyError, StopIteration, TypeError):
    raise SystemExit('source lock has no current-platform record')

if release.get('repository') != repository or release.get('ref') != release_ref:
    raise SystemExit('signed release repository/ref differs')
if release.get('commit') != release_commit or release.get('composition', {}).get('pack_builder_commit') != release_commit:
    raise SystemExit('signed release commit differs')
subjects = {item['role']: item for item in release.get('subjects', [])}
if set(subjects) != {'pack', 'manifest', 'sbom'}:
    raise SystemExit('signed release subject inventory differs')

expected_pack_name = f'pre-commit-review-rust-analyzer-{pack_version}-{platform}.tar.gz'
if pack_path.name != expected_pack_name or subjects['pack']['path'] != expected_pack_name:
    raise SystemExit('signed pack name differs')
pack_digest = digest(pack_path)
if pack_digest != subjects['pack']['sha256'] or pack_digest != metadata.get('pack_sha256'):
    raise SystemExit('pack digest differs from signed metadata')
if digest(pack_manifest_path) != subjects['manifest']['sha256']:
    raise SystemExit('pack manifest digest differs from signed metadata')
if digest(sbom_path) != subjects['sbom']['sha256']:
    raise SystemExit('SBOM digest differs from signed metadata')

expected_metadata = {
    'artifact_id': 'rust-analyzer',
    'pack_version': pack_version,
    'platform_id': platform,
    'project_asset_name': expected_pack_name,
    'pack_sha256': pack_digest,
    'pack_manifest_sha256': subjects['manifest']['sha256'],
    'sbom_sha256': subjects['sbom']['sha256'],
    'source_lock_sha256': source_digest,
    'executable_sha256': source_asset['executable_sha256'],
    'upstream_archive_sha256': source_asset['archive_sha256'],
}
if metadata != expected_metadata:
    raise SystemExit('released current-platform metadata differs from reviewed inputs')

if (
    pack_manifest.get('artifact_id') != 'rust-analyzer'
    or pack_manifest.get('pack_version') != pack_version
    or pack_manifest.get('platform_id') != platform
    or pack_manifest.get('target_triple') != source_asset['target_triple']
    or pack_manifest.get('source_lock_sha256') != source_digest
    or pack_manifest.get('project_asset_name') != expected_pack_name
):
    raise SystemExit('pack manifest identity differs')
files = pack_manifest.get('files', [])
executable_files = [item for item in files if item.get('role') == 'executable']
license_files = [item for item in files if item.get('role') == 'license']
sbom_files = [item for item in files if item.get('role') == 'sbom']
if len(executable_files) != 1 or len(license_files) != 2 or len(sbom_files) != 1:
    raise SystemExit('pack manifest file roles differ')
executable = executable_files[0]
if executable['sha256'] != source_asset['executable_sha256'] or executable['size'] != source_asset['executable_size']:
    raise SystemExit('pack executable differs from source lock')
if sbom_files[0]['sha256'] != metadata['sbom_sha256']:
    raise SystemExit('pack SBOM binding differs')

components = sbom.get('components', [])
if len(components) != 1 or components[0].get('name') != 'rust-analyzer':
    raise SystemExit('SBOM component identity differs')
sbom_component = components[0].get('purl')
if sbom_component != 'pkg:github/rust-lang/rust-analyzer@2026-07-27':
    raise SystemExit('SBOM package identity differs')

if (
    baseline.get('artifact_id') != 'rust-analyzer'
    or baseline.get('pack_version') != pack_version
    or baseline.get('source_lock_sha256') != source_digest
):
    raise SystemExit('reviewed candidate baseline identity differs')
if revocations != {'schema_version': 1, 'kind': 'third_party_artifact_revocations', 'entries': []}:
    raise SystemExit('candidate revocation index differs')

record = {
    'artifact_id': 'rust-analyzer',
    'artifact_role': 'repository-context-provider',
    'tool_version': source_lock['tool_version'],
    'upstream_repository': source_lock['upstream_repository'],
    'upstream_tag': source_lock['upstream_tag'],
    'upstream_commit': source_lock['upstream_commit'],
    'source_lock_sha256': source_digest,
    'platform_id': platform,
    'target_triple': source_asset['target_triple'],
    'state': 'active',
    'pack_version': pack_version,
    'project_release_tag': release_tag,
    'project_asset_name': expected_pack_name,
    'expected_compressed_size': pack_path.stat().st_size,
    'max_compressed_size': 32 * 1024 * 1024,
    'pack_sha256': metadata['pack_sha256'],
    'pack_manifest_sha256': metadata['pack_manifest_sha256'],
    'sbom_sha256': metadata['sbom_sha256'],
    'pack_format': 'normalized-tar-gzip-v1',
    'executable': {
        'path': executable['path'],
        'size': executable['size'],
        'sha256': executable['sha256'],
    },
    'version_probe': 'rust-analyzer-version-v1',
    'capability_probe': 'rust-analyzer-stdio-v1',
    'expected_version': source_asset['expected_version_output'],
    'license_component': 'rust-analyzer',
    'license_files': [
        {'path': item['path'], 'size': item['size'], 'sha256': item['sha256']}
        for item in license_files
    ],
    'sbom_component': sbom_component,
    'default_configuration_sha256': None,
    'quality_baseline_sha256': digest(baseline_path),
    'revoked_reason': None,
    'replacement_pack_version': None,
}
manifest = {
    'schema_version': 1,
    'kind': 'third_party_artifacts',
    'release_repository': repository,
    'revocation_index_sha256': digest(revocations_path),
    'packs': [record],
}
manifest_path.write_bytes(json.dumps(manifest, separators=(',', ':')).encode())
PY

manifest_native="$(native_path "$manifest_path")"
pack_native="$(native_path "$pack_path")"
cache_native="$(native_path "$cache_root")"
target_native="$(native_path "$target_root")"

manager_messages="$harness_root/manager-build.jsonl"
cargo +1.95.0 build --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --locked --bin collect-diff-context-cli --message-format=json >"$manager_messages"
manager="$(python3 - "$manager_messages" <<'PY'
import json
import sys
from pathlib import Path

executables = []
for line in Path(sys.argv[1]).read_text(encoding='utf-8').splitlines():
    value = json.loads(line)
    target = value.get('target', {})
    if value.get('reason') == 'compiler-artifact' and target.get('name') == 'collect-diff-context-cli':
        if value.get('executable'):
            executables.append(value['executable'])
if len(executables) != 1:
    raise SystemExit(f'expected one artifact manager executable, found {executables!r}')
print(executables[0])
PY
)"
[ -x "$manager" ] || fail 'artifact manager binary is unavailable after build'

PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR="$cache_native" \
PRE_COMMIT_REVIEW_FETCH_PROGRESS='never' \
  "$manager" artifacts verify \
  --manifest "$manifest_native" \
  --artifact-id rust-analyzer \
  --platform-id "$platform" \
  --pack "$pack_native" >"$harness_root/verify-report.json"

PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR="$cache_native" \
PRE_COMMIT_REVIEW_FETCH_PROGRESS='never' \
  "$manager" artifacts provision \
  --manifest "$manifest_native" \
  --artifact-id rust-analyzer \
  --platform-id "$platform" \
  --pack "$pack_native" \
  --target-root "$target_native" >"$harness_root/provision-report.json"

python3 - "$target_native" "$platform" "$pack_version" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
platform = sys.argv[2]
pack_version = sys.argv[3]
registry_path = root / 'runtime/providers/provider-registry.json'
profile_path = root / 'runtime/providers/rust-analyzer.profile.json'
receipt_path = root / 'runtime/artifact-receipts/rust-analyzer.json'
for path in (registry_path, profile_path, receipt_path):
    if not path.is_file() or path.is_symlink():
        raise SystemExit(f'target-local provider file is absent or unsafe: {path}')
registry_raw = registry_path.read_bytes()
profile_raw = profile_path.read_bytes()
registry = json.loads(registry_raw)
profile = json.loads(profile_raw)
if json.dumps(registry, separators=(',', ':')).encode() != registry_raw:
    raise SystemExit('target-local registry is not canonical')
if json.dumps(profile, separators=(',', ':')).encode() != profile_raw:
    raise SystemExit('target-local profile is not canonical')
if registry.get('kind') != 'repository_context_provider_registry' or len(registry.get('entries', [])) != 1:
    raise SystemExit('target-local provider registry identity differs')
entry = registry['entries'][0]
executable = Path(entry['executable_path']).resolve()
expected_root = (root / 'runtime/third-party/rust-analyzer' / pack_version).resolve()

def same_file(first, second):
    try:
        return first.samefile(second)
    except OSError:
        return False

profile_matches = same_file(Path(entry['profile_path']), profile_path)
executable_is_contained = any(same_file(parent, expected_root) for parent in executable.parents)
if not profile_matches or not executable_is_contained:
    raise SystemExit('provider registry escapes the target-local pack')
if entry['target_triple'] != profile['target_triple'] or profile['arguments'] != []:
    raise SystemExit('provider registry/profile binding differs')
if entry['profile_sha256'] != hashlib.sha256(profile_raw).hexdigest():
    raise SystemExit('provider registry does not bind the profile bytes')
if entry['executable_sha256'] != hashlib.sha256(executable.read_bytes()).hexdigest():
    raise SystemExit('provider registry does not bind the executable bytes')
if not executable.is_file() or platform.startswith('windows-') != executable.name.endswith('.exe'):
    raise SystemExit('target-local executable identity differs')
PY

test_messages="$harness_root/provider-test-build.jsonl"
cargo +1.95.0 test --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --locked --features test-fixture --test repository_context_provider_real \
  --no-run --message-format=json >"$test_messages"
test_executable="$(python3 - "$test_messages" <<'PY'
import json
import sys
from pathlib import Path

executables = []
for line in Path(sys.argv[1]).read_text(encoding='utf-8').splitlines():
    value = json.loads(line)
    target = value.get('target', {})
    if value.get('reason') == 'compiler-artifact' and target.get('name') == 'repository_context_provider_real':
        if value.get('executable'):
            executables.append(value['executable'])
if len(executables) != 1:
    raise SystemExit(f'expected one real-provider test executable, found {executables!r}')
print(executables[0])
PY
)"
[ -x "$test_executable" ] || fail 'real-provider test executable is unavailable'

snapshot_target "$target_native" "$harness_root/target-before.json"
PCR_REAL_PROVIDER_TARGET_ROOT="$target_native" \
  cargo +1.95.0 test \
  --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --locked --features test-fixture --test repository_context_provider_real -- --nocapture
snapshot_target "$target_native" "$harness_root/target-after.json"
cmp "$harness_root/target-before.json" "$harness_root/target-after.json" >/dev/null || \
  fail 'provider execution changed target-local authorization bytes'

fallback_marker="$sentinel_root/fallback-invoked"
fallback_marker_native="$(native_path "$fallback_marker")"
real_path="$PATH"
printf '%s' 'invalid global provider registry' >"$sentinel_root/home/provider-registry.json"
printf '%s' 'invalid global provider registry' \
  >"$sentinel_root/home/.pre-commit-review-provider-registry.json"
if [ "$platform" = 'windows-amd64' ]; then
  require_tool rustc
  sentinel_source="$sentinel_root/provider-tool-sentinel.rs"
  sentinel_executable="$sentinel_root/provider-tool-sentinel.exe"
  cat >"$sentinel_source" <<'RS'
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let marker = env::var_os("PCR_PROVIDER_FALLBACK_MARKER")
        .expect("PCR_PROVIDER_FALLBACK_MARKER must identify the sentinel marker");
    let executable = env::current_exe().expect("sentinel executable path must be available");
    let name = executable
        .file_name()
        .expect("sentinel executable name must be available");
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
        .expect("sentinel marker must be writable");
    writeln!(output, "{}", name.to_string_lossy()).expect("sentinel marker write must succeed");
    std::process::exit(97);
}
RS
  rustc +1.95.0 "$sentinel_source" -o "$sentinel_executable"
  for name in cargo rustc rustup; do
    cp -- "$sentinel_executable" "$sentinel_root/bin/$name.exe"
  done
else
  bash_executable="$(command -v bash)"
  python3 - "$sentinel_root/bin" "$bash_executable" <<'PY'
import os
import sys
from pathlib import Path

root = Path(sys.argv[1])
shell = sys.argv[2]
for name in ('cargo', 'rustc', 'rustup'):
    script = root / name
    script.write_text(
        f'#!{shell}\n'
        'set -euo pipefail\n'
        ': "${PCR_PROVIDER_FALLBACK_MARKER:?}"\n'
        f"printf '%s\\n' '{name}' >>\"$PCR_PROVIDER_FALLBACK_MARKER\"\n"
        'exit 97\n',
        encoding='utf-8',
    )
    os.chmod(script, 0o700)
PY
fi

PATH="$sentinel_root/bin:$real_path" \
HOME="$sentinel_root/home" \
CARGO_HOME="$sentinel_root/cargo-home" \
RUSTUP_HOME="$sentinel_root/rustup-home" \
PCR_PROVIDER_FALLBACK_MARKER="$fallback_marker_native" \
PCR_REAL_PROVIDER_TARGET_ROOT="$target_native" \
  "$test_executable" real_multi_crate_report_contains_the_cross_crate_call_edge \
  --exact --nocapture
[ ! -e "$fallback_marker" ] || fail 'provider reached a PATH/rustup/Cargo fallback'
snapshot_target "$target_native" "$harness_root/target-after-sentinel.json"
cmp "$harness_root/target-before.json" "$harness_root/target-after-sentinel.json" >/dev/null || \
  fail 'sentinel execution changed target-local authorization bytes'

python3 - "$repo_root" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
runtime = (root / 'collect-diff-context-cli/src/trusted_runtime.rs').read_text(encoding='utf-8')
session = (root / 'collect-diff-context-cli/src/repository_context_provider/session.rs').read_text(encoding='utf-8')
if '.env_clear()' not in runtime or '.env("PATH", path)' not in runtime:
    raise SystemExit('trusted runtime no longer clears and replaces PATH')
if 'runtime.empty_path().as_os_str()' not in session or '.env("RA_LOG", "off")' not in session:
    raise SystemExit('provider session no longer binds the empty PATH and deterministic logging')
for forbidden in (r'Command::new\("cargo"\)', r'Command::new\("rustup"\)'):
    if re.search(forbidden, session):
        raise SystemExit('provider session contains a Cargo/rustup fallback')
PY

(
  unset PCR_REAL_PROVIDER_TARGET_ROOT
  "$test_executable" normalized_real_single_crate_reports_are_byte_identical \
    --exact --nocapture
)
cargo +1.95.0 test --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --locked --test provider_install

if [ "$baseline_values" -eq 4 ]; then
  [ -f "$baseline_runner" ] && [ ! -L "$baseline_runner" ] || \
    fail 'hosted baseline runner is not a regular file'
  case "$baseline_evidence_output" in
    /*|[A-Za-z]:[\\/]*) ;;
    *) fail 'hosted baseline evidence output must be absolute' ;;
  esac
  [ ! -e "$baseline_evidence_output" ] || \
    fail 'hosted baseline evidence output already exists'
  baseline_output_parent="$(dirname -- "$baseline_evidence_output")"
  [ -d "$baseline_output_parent" ] && [ ! -L "$baseline_output_parent" ] || \
    fail 'hosted baseline evidence parent is not a regular directory'
  python3 - "$baseline_runner_sha256" <<'PY' || \
    fail 'hosted baseline runner digest is invalid'
import re
import sys

if re.fullmatch(r'[0-9a-f]{64}', sys.argv[1]) is None:
    raise SystemExit(1)
PY

  baseline_contract="$harness_root/provider-baseline-contract.json"
  baseline_measurement="$harness_root/provider-baseline-measurement.json"
  baseline_runner_native="$(native_path "$baseline_runner")"
  baseline_contract_native="$(native_path "$baseline_contract")"
  source_lock_native="$(native_path "$repo_root/third_party_artifacts/sources/rust-analyzer-2026-07-27.json")"
  fixture_native="$(native_path "$repo_root/collect-diff-context-cli/tests/fixtures/repository_context_provider/real/single_crate")"
  env -u PCR_PROVIDER_BASELINE_EXPECTED_RUNNER_SHA256 \
    "$baseline_runner_native" contract \
    --target-root "$target_native" \
    --source-lock "$source_lock_native" \
    --fixture-root "$fixture_native" \
    --runner-class "$baseline_runner_class" \
    --output "$baseline_contract_native"

  python3 - "$baseline_contract" <<'PY' || \
    fail 'runner contract cannot declare its trusted digest'
import json
import sys
from pathlib import Path

contract = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
trusted = 'PCR_PROVIDER_BASELINE_EXPECTED_RUNNER_SHA256'.casefold()
if any(name.casefold() == trusted for name in contract.get('environment', {})):
    raise SystemExit(1)
PY

  PCR_PROVIDER_BASELINE_EXPECTED_RUNNER_SHA256="$baseline_runner_sha256" \
    python3 "$repo_root/scripts/measure_provider_baseline.py" \
    --runner "$baseline_contract" \
    --samples 20 >"$baseline_measurement"

  python3 - \
    "$baseline_measurement" \
    "$baseline_runner_sha256" \
    "$platform" \
    "$baseline_runner_class" <<'PY' || \
    fail 'hosted baseline measurement differs from its reviewed inputs'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
raw = path.read_bytes()
measurement = json.loads(raw)
if json.dumps(measurement, separators=(',', ':'), sort_keys=True).encode() != raw:
    raise SystemExit(1)
if measurement.get('kind') == 'provider_baseline_local_evidence':
    raise SystemExit(1)
if measurement.get('runner_sha256') != sys.argv[2]:
    raise SystemExit(1)
if measurement.get('platform_id') != sys.argv[3]:
    raise SystemExit(1)
if measurement.get('runner_class') != sys.argv[4]:
    raise SystemExit(1)
if measurement.get('provisioning_included') is not False:
    raise SystemExit(1)
if len(measurement.get('samples_ms', [])) != 20:
    raise SystemExit(1)
if not isinstance(measurement.get('p95_ms'), int):
    raise SystemExit(1)
if not isinstance(measurement.get('peak_process_tree_rss_bytes'), int):
    raise SystemExit(1)
PY
  snapshot_target "$target_native" "$harness_root/target-after-baseline.json"
  cmp "$harness_root/target-before.json" "$harness_root/target-after-baseline.json" >/dev/null || \
    fail 'baseline measurement changed target-local authorization bytes'
  mv -- "$baseline_measurement" "$baseline_evidence_output"
fi

rm -rf -- "$target_root" "$cache_root" "$harness_root" "$sentinel_root"
for removed in "$target_root" "$cache_root" "$harness_root" "$sentinel_root"; do
  [ ! -e "$removed" ] || fail "temporary path was not removed: $removed"
done

if [ "$release_owned" = 'yes' ]; then
  [ "$release_root" = "$tmp_dir/release" ] || fail 'downloaded release root escaped the harness'
fi

cleanup
trap - EXIT HUP INT TERM
[ ! -e "$tmp_dir" ] || fail 'temporary provider harness root was not removed'
printf 'provider real-server test passed for %s\n' "$platform"
