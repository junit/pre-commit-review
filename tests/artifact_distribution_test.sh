#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'artifact distribution test failed: %s\n' "$*" >&2
  exit 1
}

fixture_root="$tmp_dir/payload"
mkdir -p \
  "$fixture_root/agents" \
  "$fixture_root/references/security" \
  "$fixture_root/docs" \
  "$fixture_root/THIRD_PARTY_LICENSES" \
  "$fixture_root/collect-diff-context-cli/schemas" \
  "$fixture_root/scripts/bin" \
  "$fixture_root/cache/downloads" \
  "$fixture_root/runtime/artifact-receipts"
printf '%s\n' 'fixture skill' > "$fixture_root/SKILL.md"
printf '%s\n' 'fixture project license' > "$fixture_root/LICENSE"
printf '%s\n' '#!/usr/bin/env bash' > "$fixture_root/install.sh"
printf '%s\n' 'fixture agent' > "$fixture_root/agents/reviewer.md"
printf '%s\n' 'title = "fixture"' > "$fixture_root/references/security/gitleaks.toml"
printf '%s\n' 'fixture docs' > "$fixture_root/docs/distribution.md"
printf '%s\n' 'fixture Gitleaks MIT license' > "$fixture_root/THIRD_PARTY_LICENSES/gitleaks-LICENSE"
printf '%s\n' 'fixture dependency license' > "$fixture_root/THIRD_PARTY_LICENSES/dependency-LICENSE"
printf '%s\n' '{"type":"object"}' > "$fixture_root/collect-diff-context-cli/schemas/review.json"
printf '%s\n' '#!/usr/bin/env bash' > "$fixture_root/scripts/collect_diff_context.sh"
chmod +x "$fixture_root/install.sh" "$fixture_root/scripts/collect_diff_context.sh"
printf '%s\n' 'must not ship' > "$fixture_root/cache/downloads/upstream-url-override"
printf '%s\n' 'must not ship' > "$fixture_root/runtime/artifact-receipts/gitleaks.json"

platforms=(darwin-amd64 darwin-arm64 linux-amd64 windows-amd64)
prefixes=(collect_diff_context static_analysis repository_context repository_context_provider)
for platform in "${platforms[@]}"; do
  suffix=''
  [ "$platform" != 'windows-amd64' ] || suffix='.exe'
  for prefix in "${prefixes[@]}"; do
    printf 'fixture %s %s\n' "$prefix" "$platform" \
      > "$fixture_root/scripts/bin/${prefix}-${platform}${suffix}"
    chmod +x "$fixture_root/scripts/bin/${prefix}-${platform}${suffix}"
  done
done

manifest="$repo_root/third_party_artifacts/manifest.json"
revocations="$repo_root/third_party_artifacts/revocations.json"
source_lock="$repo_root/third_party_artifacts/sources/gitleaks-8.30.1.json"
updated_manifest="$tmp_dir/manifest.json"
cp "$manifest" "$updated_manifest"

for platform in "${platforms[@]}"; do
  suffix=''
  [ "$platform" != 'windows-amd64' ] || suffix='.exe'
  fake_binary="$tmp_dir/gitleaks-${platform}${suffix}"
  printf 'fixture gitleaks %s\n' "$platform" > "$fake_binary"
  chmod +x "$fake_binary"
  platform_source_lock="$tmp_dir/source-lock-${platform}.json"
  python3 - "$source_lock" "$platform_source_lock" "$platform" "$fake_binary" <<'PY'
import hashlib
import json
from pathlib import Path
import sys

source, destination, platform, binary = sys.argv[1:]
lock = json.loads(Path(source).read_text(encoding='utf-8'))
payload = Path(binary).read_bytes()
for asset in lock['assets']:
    if asset['platform_id'] == platform:
        asset['executable_size'] = len(payload)
        asset['executable_sha256'] = hashlib.sha256(payload).hexdigest()
        break
else:
    raise SystemExit(f'missing source-lock platform: {platform}')
Path(destination).write_text(
    json.dumps(lock, separators=(',', ':'), ensure_ascii=True), encoding='utf-8'
)
PY
  pack="$tmp_dir/pre-commit-review-gitleaks-8.30.1-pcr.1-${platform}.tar.gz"
  record="$tmp_dir/gitleaks-${platform}.record.json"
  "$repo_root/scripts/build_artifact_pack.sh" \
    --kind gitleaks --platform-id "$platform" --pack-version 8.30.1-pcr.1 \
    --source-root "$fixture_root" --manifest "$updated_manifest" \
    --source-lock "$platform_source_lock" --binary "$fake_binary" \
    --output "$pack" --record-output "$record" \
    --manifest-output "$updated_manifest" >/dev/null

  core="$tmp_dir/pre-commit-review-core-0.1.0-pcr.1-${platform}.tar.gz"
  "$repo_root/scripts/build_artifact_pack.sh" \
    --kind core --platform-id "$platform" --pack-version 0.1.0-pcr.1 \
    --source-root "$fixture_root" --manifest "$updated_manifest" \
    --revocations "$revocations" --output "$core" >/dev/null

  python3 - "$pack" "$record" "$core" "$platform" "$suffix" \
    "$updated_manifest" "$revocations" "$platform_source_lock" <<'PY'
import hashlib
import json
from pathlib import Path
import sys
import tarfile

pack_path, record_path, core_path, platform, suffix, manifest_path, revocations_path, source_lock_path = sys.argv[1:]

def sha256(data):
    return hashlib.sha256(data).hexdigest()

def check_archive_metadata(path, members):
    header = Path(path).read_bytes()[:10]
    if header != bytes([0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255]):
        raise SystemExit(f'{path}: non-canonical gzip header: {header!r}')
    names = [member.name for member in members]
    if names != sorted(names):
        raise SystemExit(f'{path}: members are not path sorted')
    for member in members:
        executable = member.name in {'install.sh', 'scripts/collect_diff_context.sh'} \
            or member.name.startswith(('bin/', 'scripts/bin/'))
        expected_mode = 0o755 if member.isdir() or executable else 0o644
        if member.mode != expected_mode or member.uid != 0 or member.gid != 0 or member.mtime != 0:
            raise SystemExit(f'{path}: non-canonical metadata for {member.name}')
        if member.uname or member.gname or member.issym() or member.islnk():
            raise SystemExit(f'{path}: forbidden metadata or link for {member.name}')

with tarfile.open(pack_path, 'r:gz') as archive:
    members = archive.getmembers()
    check_archive_metadata(pack_path, members)
    expected = [
        'bin', f'bin/gitleaks{suffix}', 'licenses',
        'licenses/GITLEAKS-LICENSE', 'pack-manifest.json', 'sbom.cdx.json',
    ]
    names = [member.name for member in members]
    if names != expected:
        raise SystemExit(f'{platform}: unexpected Gitleaks members: {names!r}')
    files = {member.name: archive.extractfile(member).read() for member in members if member.isfile()}

record_bytes = Path(record_path).read_bytes()
record = json.loads(record_bytes)
if json.dumps(record, separators=(',', ':'), ensure_ascii=True).encode() != record_bytes:
    raise SystemExit(f'{platform}: record is not compact canonical JSON')
required_record_fields = {
    'artifact_id', 'artifact_role', 'tool_version', 'upstream_repository',
    'upstream_tag', 'upstream_commit', 'source_lock_sha256', 'platform_id',
    'target_triple', 'state', 'pack_version', 'project_release_tag',
    'project_asset_name', 'expected_compressed_size', 'max_compressed_size',
    'pack_sha256', 'pack_manifest_sha256', 'sbom_sha256', 'pack_format',
    'executable', 'version_probe', 'capability_probe', 'expected_version',
    'license_component', 'license_files', 'sbom_component',
    'default_configuration_sha256', 'quality_baseline_sha256',
    'revoked_reason', 'replacement_pack_version',
}
if set(record) != required_record_fields:
    raise SystemExit(f'{platform}: incomplete ArtifactPackRecord fields: {sorted(set(record) ^ required_record_fields)}')
if record['platform_id'] != platform or record['project_asset_name'] != Path(pack_path).name:
    raise SystemExit(f'{platform}: record identity is not bound to the release asset')
if record['pack_sha256'] != sha256(Path(pack_path).read_bytes()):
    raise SystemExit(f'{platform}: record does not bind the outer pack digest')
if record['pack_manifest_sha256'] != sha256(files['pack-manifest.json']):
    raise SystemExit(f'{platform}: record does not bind the internal manifest digest')
if record['source_lock_sha256'] != sha256(Path(source_lock_path).read_bytes()):
    raise SystemExit(f'{platform}: record does not bind the source lock digest')
if record['license_files'][0]['sha256'] != sha256(files['licenses/GITLEAKS-LICENSE']):
    raise SystemExit(f'{platform}: record does not bind the copied license')
updated_manifest = json.loads(Path(manifest_path).read_bytes())
matching = [item for item in updated_manifest['packs']
            if item['artifact_id'] == record['artifact_id']
            and item['platform_id'] == record['platform_id']]
if matching != [record] or record['state'] != 'active':
    raise SystemExit(f'{platform}: generated manifest does not contain its active pack record')
sbom = json.loads(files['sbom.cdx.json'])
component = sbom['components'][0]
properties = {item['name']: item['value'] for item in component['properties']}
if component.get('supplier', {}).get('name') != 'Gitleaks':
    raise SystemExit(f'{platform}: SBOM is missing supplier evidence')
if properties.get('pre-commit-review:evidence-scope') != 'component-evidence':
    raise SystemExit(f'{platform}: SBOM overstates component evidence')
if properties.get('pre-commit-review:transitive-closure') != 'unknown':
    raise SystemExit(f'{platform}: SBOM overstates transitive closure')
if not component.get('licenses') or not component.get('externalReferences'):
    raise SystemExit(f'{platform}: SBOM is missing license or source evidence')
if sbom['dependencies'][0]['dependsOn'] != [component['bom-ref']]:
    raise SystemExit(f'{platform}: SBOM is missing the contains relationship')

with tarfile.open(core_path, 'r:gz') as archive:
    members = archive.getmembers()
    check_archive_metadata(core_path, members)
    names = [member.name for member in members]
    regular = {member.name: archive.extractfile(member).read() for member in members if member.isfile()}

expected_binaries = {f'scripts/bin/{prefix}-{platform}{suffix}' for prefix in (
    'collect_diff_context', 'static_analysis', 'repository_context', 'repository_context_provider')}
observed_binaries = {name for name in regular if name.startswith('scripts/bin/')}
if observed_binaries != expected_binaries:
    raise SystemExit(f'{platform}: core binaries are not platform-isolated: {sorted(observed_binaries)}')
required = {
    'SKILL.md', 'LICENSE', 'install.sh', 'agents/reviewer.md',
    'references/security/gitleaks.toml', 'docs/distribution.md',
    'collect-diff-context-cli/schemas/review.json',
    'THIRD_PARTY_LICENSES/dependency-LICENSE',
    'runtime/distribution/manifest.json', 'runtime/distribution/revocations.json',
    'runtime/distribution/core-pack-manifest.json',
    'runtime/distribution/core-sbom.cdx.json',
} | expected_binaries
if missing := required - set(regular):
    raise SystemExit(f'{platform}: core pack is missing required files: {sorted(missing)}')
for name in names:
    if (name.startswith(('bin/gitleaks', 'scripts/bin/gitleaks-'))
            or name.startswith(('bin/rust-analyzer', 'scripts/bin/rust-analyzer'))
            or '/target/' in f'/{name}/'
            or '/cache/' in f'/{name}/' or 'upstream-url-override' in name
            or 'artifact-receipts' in name):
        raise SystemExit(f'{platform}: forbidden core member: {name}')

inventory_path = 'runtime/distribution/core-pack-manifest.json'
inventory_bytes = regular[inventory_path]
inventory = json.loads(inventory_bytes)
if json.dumps(inventory, separators=(',', ':'), ensure_ascii=True).encode() != inventory_bytes:
    raise SystemExit(f'{platform}: core inventory is not compact canonical JSON')
bindings = {item['path']: item for item in inventory['members']}
expected_inventory = set(regular) - {inventory_path}
if set(bindings) != expected_inventory:
    raise SystemExit(f'{platform}: inventory must bind every regular member except itself')
if inventory_path in bindings:
    raise SystemExit(f'{platform}: core inventory contains an impossible self-reference')
for path, data in regular.items():
    if path == inventory_path:
        continue
    expected_mode = 0o755 if path in {'install.sh', 'scripts/collect_diff_context.sh'} \
        or path.startswith('scripts/bin/') else 0o644
    binding = bindings[path]
    if binding != {'path': path, 'mode': expected_mode, 'size': len(data), 'sha256': sha256(data)}:
        raise SystemExit(f'{platform}: incorrect inventory binding for {path}')
if inventory['distribution_manifest_sha256'] != sha256(Path(manifest_path).read_bytes()):
    raise SystemExit(f'{platform}: inventory does not bind the distribution manifest')
if inventory['revocation_index_sha256'] != sha256(Path(revocations_path).read_bytes()):
    raise SystemExit(f'{platform}: inventory does not bind the revocation index')
PY
done

python3 - "$updated_manifest" <<'PY'
import json
from pathlib import Path
import sys

manifest = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
records = manifest['packs']
if len(records) != 4 or [record['platform_id'] for record in records] != [
    'darwin-amd64', 'darwin-arm64', 'linux-amd64', 'windows-amd64'
]:
    raise SystemExit('matrix did not produce one canonical four-platform manifest')
PY

original="$tmp_dir/pre-commit-review-gitleaks-8.30.1-pcr.1-darwin-arm64.tar.gz"
rebuilt="$tmp_dir/gitleaks-darwin-arm64-rebuilt.tar.gz"
"$repo_root/scripts/build_artifact_pack.sh" \
  --kind gitleaks --platform-id darwin-arm64 --pack-version 8.30.1-pcr.1 \
  --source-root "$fixture_root" --manifest "$updated_manifest" \
  --source-lock "$tmp_dir/source-lock-darwin-arm64.json" \
  --binary "$tmp_dir/gitleaks-darwin-arm64" --output "$rebuilt" >/dev/null
cmp "$original" "$rebuilt" || fail 'identical inputs did not produce identical Gitleaks bytes'

if "$repo_root/scripts/build_artifact_pack.sh" \
  --kind gitleaks --platform-id linux-amd64 --pack-version 8.30.1-pcr.1 \
  --source-root "$fixture_root" --manifest "$manifest" --source-lock "$source_lock" \
  --binary "$tmp_dir/gitleaks-linux-amd64" --output "$tmp_dir/override.tar.gz" \
  --upstream-url https://example.invalid >/dev/null 2>&1; then
  fail 'builder accepted a non-reviewed upstream URL override'
fi

if grep -Eq 'python3|tarfile|gzip\.GzipFile' "$repo_root/scripts/build_artifact_pack.sh"; then
  fail 'builder still contains a Python tar/gzip writer'
fi

grep -Fq 'Build normalized Gitleaks pack' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not build per-platform Gitleaks packs'
grep -Fq 'Build platform core pack' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not build per-platform core packs'
grep -Fq 'pre-commit-review-gitleaks-' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not publish the Gitleaks asset grammar'
grep -Fq 'pre-commit-review-core-' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not publish the core asset grammar'
grep -Fq 'sha256sum' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not publish external archive sidecars'
grep -Fq 'actions/attest-build-provenance@' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not attest release archive subjects'
grep -Fq 'artifact-pack-release.yml' "$repo_root/.github/workflows/artifact-pack-release.yml" \
  || fail 'provider pack workflow does not bind its own workflow identity'
grep -Fq 'verify_release_artifacts.sh --fixture' "$repo_root/.github/workflows/artifact-pack-release.yml" \
  || fail 'provider pack workflow does not run the independent verifier'
if grep -Eq 'uses: [^@]+@(v[0-9]+|master|stable|main)$' \
  "$repo_root/.github/workflows/release.yml" "$repo_root/.github/workflows/artifact-pack-release.yml"; then
  fail 'release trust workflows use a moving action ref'
fi
grep -Fq 'tag_name: artifact-gitleaks-8.30.1-pcr.1' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not publish Gitleaks at the record-bound release tag'
if grep -Fq 'pre-commit-review-runtime.tar.gz' "$repo_root/.github/workflows/release.yml"; then
  fail 'release workflow still publishes the legacy all-platform runtime archive'
fi
grep -Fq 'pre-commit-review-gitleaks-' "$repo_root/scripts/build_all_binaries.sh" \
  || fail 'local multi-platform builder does not create Gitleaks packs'
grep -Fq 'pre-commit-review-core-' "$repo_root/scripts/build_all_binaries.sh" \
  || fail 'local multi-platform builder does not create core packs'
grep -Fq "copy_core_distribution \"\$staging_dir\"" "$repo_root/install.sh" \
  || fail 'installer does not stage immutable core distribution metadata before provisioning'
grep -Fq "cp \"\$source_dir/install.sh\" \"\$staging_dir/\"" "$repo_root/install.sh" \
  || fail 'installer does not preserve the core-bound installer member'
grep -Fq "cp -R \"\$source_dir/docs\" \"\$staging_dir/\"" "$repo_root/install.sh" \
  || fail 'installer does not preserve every core-bound documentation member'

python3 - "$repo_root/install.sh" <<'PY'
from pathlib import Path
import sys

installer = Path(sys.argv[1]).read_text(encoding='utf-8')
copy = installer.index('copy_core_distribution "$staging_dir"')
provider = installer.index("'repository-context-provider-cli' 'Repository context provider'", copy)
gitleaks = installer.index('provision_gitleaks "$staging_dir"', provider)
if not copy < provider < gitleaks:
    raise SystemExit('installer does not finalize core inventory before provider/Gitleaks provisioning')
PY

release_fixture="$repo_root/tests/fixtures/release"
"$repo_root/scripts/verify_release_artifacts.sh" --fixture "$release_fixture" >/dev/null \
  || fail 'release trust fixture did not verify'

expect_release_rejection() {
  local fixture_path="$1"
  local expected_code="$2"
  if "$repo_root/scripts/verify_release_artifacts.sh" --fixture "$fixture_path" \
    >"$tmp_dir/release-stdout" 2>"$tmp_dir/release-stderr"; then
    fail "release verifier accepted fixture expected to fail: $expected_code"
  fi
  grep -Fq "$expected_code" "$tmp_dir/release-stderr" \
    || fail "release verifier did not report $expected_code"
}

sidecar_fixture="$tmp_dir/release-sidecar"
cp -R "$release_fixture" "$sidecar_fixture"
printf '%064d\n' 0 > "$sidecar_fixture/pre-commit-review-core-0.1.0-linux-amd64.tar.gz.sha256"
expect_release_rejection "$sidecar_fixture" 'sidecar-digest'

subject_fixture="$tmp_dir/release-subject"
cp -R "$release_fixture" "$subject_fixture"
python3 - "$subject_fixture/pre-commit-review-core-0.1.0-linux-amd64.tar.gz.attestation.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
attestation = json.loads(path.read_text(encoding='utf-8'))
attestation['subject'][0]['digest']['sha256'] = '0' * 64
path.write_text(json.dumps(attestation, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$subject_fixture" 'attestation-subject'

signer_fixture="$tmp_dir/release-signer"
cp -R "$release_fixture" "$signer_fixture"
python3 - "$signer_fixture/pre-commit-review-gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz.attestation.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
attestation = json.loads(path.read_text(encoding='utf-8'))
attestation['signer']['workflow'] = '.github/workflows/release.yml'
path.write_text(json.dumps(attestation, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$signer_fixture" 'attestation-signer'

predicate_fixture="$tmp_dir/release-predicate"
cp -R "$release_fixture" "$predicate_fixture"
python3 - "$predicate_fixture/pre-commit-review-core-0.1.0-linux-amd64.tar.gz.attestation.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
attestation = json.loads(path.read_text(encoding='utf-8'))
attestation['predicateType'] = 'https://slsa.dev/provenance/v1'
path.write_text(json.dumps(attestation, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$predicate_fixture" 'attestation-predicate'

composition_fixture="$tmp_dir/release-composition"
cp -R "$release_fixture" "$composition_fixture"
python3 - "$composition_fixture/pre-commit-review-gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz.attestation.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
attestation = json.loads(path.read_text(encoding='utf-8'))
del attestation['predicate']['composition']['source_lock_sha256']
path.write_text(json.dumps(attestation, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$composition_fixture" 'attestation-composition'

immutable_fixture="$tmp_dir/release-immutable"
cp -R "$release_fixture" "$immutable_fixture"
python3 - "$immutable_fixture/release.json" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
release = json.loads(path.read_text(encoding='utf-8'))
release['immutable'] = False
path.write_text(json.dumps(release, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$immutable_fixture" 'immutable-release-unavailable'

revocation_fixture="$tmp_dir/release-revocation-limit"
cp -R "$release_fixture" "$revocation_fixture"
python3 - "$revocation_fixture/revocations.json" "$revocation_fixture/release.json" <<'PY'
import hashlib
import json
from pathlib import Path
import sys

revocations_path, release_path = map(Path, sys.argv[1:])
entries = [
    {
        'pack_sha256': f'{index:064x}',
        'artifact_id': 'gitleaks',
        'platform_id': 'linux-amd64',
        'pack_version': '8.30.1-pcr.1',
        'reason': 'fixture revocation',
        'replacement_pack_version': None,
    }
    for index in range(16_385)
]
revocations = {
    'schema_version': 1,
    'kind': 'third_party_artifact_revocations',
    'entries': entries,
}
raw = json.dumps(revocations, separators=(',', ':')).encode('utf-8')
revocations_path.write_bytes(raw)
release = json.loads(release_path.read_text(encoding='utf-8'))
release['revocation_index']['sha256'] = hashlib.sha256(raw).hexdigest()
release_path.write_text(json.dumps(release, separators=(',', ':')), encoding='utf-8')
PY
expect_release_rejection "$revocation_fixture" 'revocation-entry-limit'

tracked_packs="$(git -C "$repo_root" ls-files third_party_artifacts/packs)"
[ "$tracked_packs" = 'third_party_artifacts/packs/.gitkeep' ] \
  || fail "generated pack archives must remain release outputs: $tracked_packs"

printf 'artifact distribution tests passed\n'
