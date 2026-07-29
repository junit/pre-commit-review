#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
REPO_ROOT="$(CDPATH='' cd -- "${SCRIPT_DIR}/.." && pwd -P)"
kind='gitleaks'
platform=''
pack_version=''
source_root="$REPO_ROOT"
source_lock=''
manifest=''
output=''
binary=''
record_output=''

usage() {
  cat <<'EOF'
Usage: scripts/build_artifact_pack.sh --kind gitleaks|core --platform-id ID \
  --pack-version VERSION --output /absolute/pack.tar.gz [options]

Options:
  --source-root PATH    Payload root (default: repository root)
  --manifest PATH       Reviewed distribution manifest (optional seed check)
  --source-lock PATH    Checked-in Gitleaks source lock
  --binary PATH         Explicit Gitleaks executable
  --record-output PATH  Write generated pack metadata
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
    --output) shift; [ "$#" -gt 0 ] || exit 2; output="$1" ;;
    --binary) shift; [ "$#" -gt 0 ] || exit 2; binary="$1" ;;
    --record-output) shift; [ "$#" -gt 0 ] || exit 2; record_output="$1" ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$kind" in gitleaks|core) ;; *) printf 'unsupported pack kind: %s\n' "$kind" >&2; exit 2 ;; esac
[ -n "$platform" ] && [ -n "$pack_version" ] && [ -n "$output" ] || { usage >&2; exit 2; }
absolute "$source_root"; absolute "$output"
[ -n "$source_lock" ] && absolute "$source_lock"
[ -n "$manifest" ] && absolute "$manifest"
[ -n "$binary" ] && absolute "$binary"
[ -n "$record_output" ] && absolute "$record_output"

export PCR_PACK_KIND="$kind" PCR_PACK_PLATFORM="$platform" PCR_PACK_VERSION="$pack_version"
export PCR_PACK_SOURCE_ROOT="$source_root" PCR_PACK_SOURCE_LOCK="$source_lock"
export PCR_PACK_MANIFEST="$manifest"
export PCR_PACK_OUTPUT="$output" PCR_PACK_BINARY="$binary" PCR_PACK_RECORD_OUTPUT="$record_output"

python3 - <<'PY'
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile


def canonical(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=True)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def fail(message):
    raise SystemExit(message)


def read_canonical(path):
    data = Path(path).read_bytes()
    value = json.loads(data)
    if canonical(value).encode() != data:
        fail(f'non-canonical JSON input: {path}')
    return value, data


def target(platform):
    return {
        'darwin-arm64': 'aarch64-apple-darwin',
        'darwin-amd64': 'x86_64-apple-darwin',
        'linux-amd64': 'x86_64-unknown-linux-musl',
        'windows-amd64': 'x86_64-pc-windows-msvc',
    }.get(platform) or fail(f'unsupported platform: {platform}')


def add(files, archive_path, source, mode=None):
    source = Path(source)
    if not source.is_file():
        fail(f'missing pack input: {source}')
    files[archive_path] = (source.read_bytes(), mode or (0o755 if archive_path.startswith('bin/') or archive_path.startswith('scripts/bin/') else 0o644))


def add_tree(files, root, prefix):
    root = Path(root)
    for source in sorted(root.rglob('*')):
        if source.is_file():
            add(files, prefix + source.relative_to(root).as_posix(), source)


def build_archive(files):
    tar_buffer = io.BytesIO()
    with tarfile.open(fileobj=tar_buffer, mode='w', format=tarfile.USTAR_FORMAT) as tar:
        directories = set()
        for path in files:
            parts = path.split('/')[:-1]
            for index in range(1, len(parts) + 1):
                directories.add('/'.join(parts[:index]) + '/')
        for path in sorted(directories | set(files)):
            info = tarfile.TarInfo(path)
            info.uid = info.gid = 0
            info.uname = info.gname = ''
            info.mtime = 0
            if path.endswith('/'):
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                tar.addfile(info)
            else:
                data, mode = files[path]
                info.mode = mode
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode='wb', compresslevel=9, mtime=0) as stream:
        stream.write(tar_buffer.getvalue())
    return compressed.getvalue()


kind = os.environ['PCR_PACK_KIND']
platform = os.environ['PCR_PACK_PLATFORM']
version = os.environ['PCR_PACK_VERSION']
root = Path(os.environ['PCR_PACK_SOURCE_ROOT'])
output = Path(os.environ['PCR_PACK_OUTPUT'])
files = {}
manifest_path = os.environ.get('PCR_PACK_MANIFEST')
if manifest_path:
    manifest, _ = read_canonical(manifest_path)
    selected = [item for item in manifest.get('packs', []) if item.get('artifact_id') == 'gitleaks' and item.get('platform_id') == platform and item.get('state') == 'active']
    if selected and selected[0].get('pack_version') != version:
        fail('manifest active pack version does not match --pack-version')

if kind == 'gitleaks':
    lock_path = os.environ.get('PCR_PACK_SOURCE_LOCK')
    if not lock_path:
        matches = sorted((root / 'third_party_artifacts' / 'sources').glob('gitleaks-*.json'))
        if len(matches) != 1:
            fail('Gitleaks pack requires one --source-lock')
        lock_path = str(matches[0])
    lock, lock_bytes = read_canonical(lock_path)
    assets = [item for item in lock['assets'] if item['platform_id'] == platform]
    if len(assets) != 1:
        fail(f'source lock has no unique asset for {platform}')
    asset = assets[0]
    suffix = '.exe' if platform == 'windows-amd64' else ''
    executable = os.environ.get('PCR_PACK_BINARY') or str(root / 'scripts' / 'bin' / f'gitleaks-{platform}{suffix}')
    license_path = root / 'THIRD_PARTY_LICENSES' / 'gitleaks-LICENSE'
    executable_bytes = Path(executable).read_bytes() if Path(executable).is_file() else fail(f'missing pack input: {executable}')
    license_bytes = license_path.read_bytes() if license_path.is_file() else fail(f'missing pack input: {license_path}')
    executable_sha = digest(executable_bytes)
    project_asset = f'gitleaks-{version}-{platform}.tar.gz'
    sbom_component = f'pkg:github/gitleaks/gitleaks@{lock["tool_version"]}'
    sbom = {
        'bomFormat': 'CycloneDX', 'specVersion': '1.5', 'version': 1,
        'metadata': {'component': {'type': 'application', 'bom-ref': f'urn:pre-commit-review:pack:gitleaks:{version}:{platform}', 'name': 'pre-commit-review-gitleaks-pack', 'version': version}},
        'components': [{'type': 'application', 'bom-ref': sbom_component, 'name': 'gitleaks', 'version': lock['tool_version'], 'purl': sbom_component,
                        'hashes': [{'alg': 'SHA-256', 'content': executable_sha}], 'licenses': [{'license': {'id': 'MIT'}}],
                        'externalReferences': [{'type': 'distribution', 'url': asset['url'], 'hashes': [{'alg': 'SHA-256', 'content': asset['archive_sha256']}]}],
                        'properties': [{'name': 'pre-commit-review:artifact-id', 'value': 'gitleaks'}, {'name': 'pre-commit-review:pack-version', 'value': version}, {'name': 'pre-commit-review:platform-id', 'value': platform}, {'name': 'pre-commit-review:evidence-scope', 'value': 'component-evidence'}, {'name': 'pre-commit-review:transitive-closure', 'value': 'unknown'}]}],
        'dependencies': [{'ref': f'urn:pre-commit-review:pack:gitleaks:{version}:{platform}', 'dependsOn': [sbom_component]}],
    }
    files['bin/gitleaks' + suffix] = (executable_bytes, 0o755)
    files['licenses/GITLEAKS-LICENSE'] = (license_bytes, 0o644)
    files['sbom.cdx.json'] = (canonical(sbom).encode(), 0o644)
    pack_manifest = {'schema_version': 1, 'kind': 'third_party_artifact_pack', 'artifact_id': 'gitleaks', 'tool_version': lock['tool_version'], 'pack_version': version, 'platform_id': platform, 'target_triple': asset['target_triple'], 'upstream_asset_name': asset['archive_name'], 'upstream_asset_sha256': asset['archive_sha256'], 'source_lock_sha256': digest(lock_bytes), 'project_asset_name': project_asset, 'files': []}
    for path, (data, _) in sorted(files.items()):
        role = 'executable' if path.startswith('bin/') else 'license' if path.startswith('licenses/') else 'sbom'
        pack_manifest['files'].append({'path': path, 'size': len(data), 'sha256': digest(data), 'role': role})
    files['pack-manifest.json'] = (canonical(pack_manifest).encode(), 0o644)
    metadata = {'artifact_id': 'gitleaks', 'artifact_role': 'sanitizer', 'tool_version': lock['tool_version'], 'platform_id': platform, 'target_triple': asset['target_triple'], 'pack_version': version, 'project_asset_name': project_asset, 'pack_manifest_sha256': digest(files['pack-manifest.json'][0]), 'sbom_sha256': digest(files['sbom.cdx.json'][0]), 'executable_sha256': executable_sha}
else:
    add(files, 'runtime/distribution/manifest.json', root / 'third_party_artifacts' / 'manifest.json')
    add(files, 'runtime/distribution/revocations.json', root / 'third_party_artifacts' / 'revocations.json')
    for name in ('SKILL.md', 'LICENSE', 'install.sh'):
        add(files, name, root / name)
    add_tree(files, root / 'agents', 'agents/')
    add_tree(files, root / 'references', 'references/')
    add_tree(files, root / 'collect-diff-context-cli' / 'schemas', 'collect-diff-context-cli/schemas/')
    add_tree(files, root / 'docs', 'docs/')
    add_tree(files, root / 'THIRD_PARTY_LICENSES', 'THIRD_PARTY_LICENSES/')
    for source in sorted((root / 'scripts').rglob('*')):
        relative = source.relative_to(root / 'scripts').as_posix()
        if source.is_file() and not relative.startswith('bin/'):
            add(files, 'scripts/' + relative, source)
    suffix = '.exe' if platform == 'windows-amd64' else ''
    collector = f'collect_diff_context-{platform}{suffix}'
    add(files, 'scripts/bin/' + collector, root / 'scripts' / 'bin' / collector)
    for prefix in ('static_analysis', 'repository_context', 'repository_context_provider'):
        candidate = root / 'scripts' / 'bin' / f'{prefix}-{platform}{suffix}'
        if candidate.is_file():
            add(files, 'scripts/bin/' + candidate.name, candidate)
    distribution = files['runtime/distribution/manifest.json'][0]
    revocations = files['runtime/distribution/revocations.json'][0]
    core_manifest = {'schema_version': 1, 'kind': 'pre_commit_review_core_pack', 'core_version': version, 'platform_id': platform, 'target_triple': target(platform), 'distribution_manifest_sha256': digest(distribution), 'revocation_index_sha256': digest(revocations), 'members': []}
    for path, (data, _) in sorted(files.items()):
        core_manifest['members'].append({'path': path, 'size': len(data), 'sha256': digest(data)})
    files['core-pack-manifest.json'] = (canonical(core_manifest).encode(), 0o644)
    files['core-sbom.cdx.json'] = (canonical({'bomFormat': 'CycloneDX', 'specVersion': '1.5', 'version': 1, 'components': []}).encode(), 0o644)
    metadata = {'kind': 'core', 'core_version': version, 'platform_id': platform, 'core_manifest_sha256': digest(files['core-pack-manifest.json'][0])}

pack = build_archive(files)
output.parent.mkdir(parents=True, exist_ok=True)
temporary = output.with_name(output.name + '.tmp')
temporary.write_bytes(pack)
os.replace(temporary, output)
metadata.update({'pack_sha256': digest(pack), 'pack_size': len(pack)})
record_output = os.environ.get('PCR_PACK_RECORD_OUTPUT')
if record_output:
    record = Path(record_output)
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(canonical(metadata), encoding='utf-8')
print(canonical(metadata))
PY
