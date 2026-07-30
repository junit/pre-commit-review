#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

if [ "$#" -ne 2 ] || { [ "$1" != '--fixture' ] && [ "$1" != '--signed-release-root' ]; }; then
  printf 'usage: %s --fixture PATH | --signed-release-root PATH\n' "$0" >&2
  exit 2
fi

python3 - "$repo_root" "$1" "$2" <<'PY'
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

MAX_JSON_BYTES = 1024 * 1024
SHA256 = re.compile(r'^[0-9a-f]{64}$')
COMMIT = re.compile(r'^[0-9a-f]{40}$')
REPOSITORY = 'junit/pre-commit-review'
WORKFLOW = '.github/workflows/artifact-pack-release.yml'
ISSUER = 'https://token.actions.githubusercontent.com'
PREDICATE_TYPE = 'pre-commit-review.artifact-pack/v1'
SOURCE_LOCK_SHA256 = '82ee6473601fba11e01fc37f60ee48f0634bfa1f24f3d01714119cfadf84b742'
PACK_VERSION = '2026.07.27-pcr.1'
RUST_TOOLCHAIN = '1.95.0'
PLATFORMS = {'darwin-amd64', 'darwin-arm64', 'linux-amd64', 'windows-amd64'}
COMPOSITION_FIELDS = {
    'source_lock_sha256',
    'upstream_archive_sha256',
    'pack_builder_commit',
    'pack_manifest_sha256',
    'sbom_sha256',
    'generator_configuration_sha256',
}


class VerificationError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def fail(code, message):
    raise VerificationError(code, message)


def read_regular(path, limit, code):
    try:
        if path.is_symlink() or not path.is_file():
            fail(code, f'{path.name} is not a regular file')
        data = path.read_bytes()
    except OSError as exc:
        fail(code, f'could not read {path.name}: {exc}')
    if not data or len(data) > limit:
        fail(code, f'{path.name} is outside its byte limit')
    return data


def read_json(path, code):
    raw = read_regular(path, MAX_JSON_BYTES, code)
    try:
        value = json.loads(raw.decode('utf-8'))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(code, f'{path.name} is not valid JSON: {exc}')
    if not isinstance(value, dict):
        fail(code, f'{path.name} must contain an object')
    if json.dumps(value, separators=(',', ':')).encode('utf-8') != raw:
        fail(code, f'{path.name} is not compact canonical JSON')
    return value


def digest(path, limit=512 * 1024 * 1024):
    return hashlib.sha256(read_regular(path, limit, 'material-read')).hexdigest()


def require_digest(value, field):
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        fail('digest-format', f'{field} is not a lower-case SHA256 digest')
    return value


def plain_name(value, field):
    if not isinstance(value, str) or not value or len(value) > 255:
        fail('release-identity', f'{field} is not a bounded file name')
    if Path(value).name != value or value in {'.', '..'} or '\\' in value:
        fail('release-identity', f'{field} is not a plain file name')
    return value


def verify_attestation(path, subject, release, composition):
    value = read_json(path, 'attestation-json')
    if set(value) != {'predicateType', 'subject', 'signer', 'predicate'}:
        fail('attestation-contract', f'{path.name} has unexpected or missing fields')
    if value['predicateType'] != PREDICATE_TYPE:
        fail('attestation-predicate', f'{path.name} has an unexpected predicate type')
    subjects = value['subject']
    expected_subject = {'name': subject['path'], 'digest': {'sha256': subject['sha256']}}
    if subjects != [expected_subject]:
        fail('attestation-subject', f'{path.name} does not bind its exact subject')
    expected_signer = {
        'repository': REPOSITORY,
        'workflow': WORKFLOW,
        'ref': release['ref'],
        'commit': release['commit'],
        'issuer': ISSUER,
    }
    if value['signer'] != expected_signer:
        fail('attestation-signer', f'{path.name} has an unscoped signer identity')
    if value['predicate'] != {'composition': composition}:
        fail('attestation-composition', f'{path.name} omits or changes composition materials')


def verify(repo_root, fixture_root):
    release = read_json(fixture_root / 'release.json', 'release-metadata')
    required = {
        'schema_version', 'kind', 'repository', 'workflow', 'ref', 'commit', 'issuer',
        'materials', 'composition', 'subjects',
    }
    if set(release) != required:
        fail('release-metadata', 'provider release metadata fields are not strict')
    if release['schema_version'] != 1 or release['kind'] != 'pre_commit_review_provider_release':
        fail('release-metadata', 'provider release metadata identity is invalid')
    if release['repository'] != REPOSITORY or release['workflow'] != WORKFLOW:
        fail('release-signer', 'provider release names another repository or workflow')
    if release['issuer'] != ISSUER or not COMMIT.fullmatch(release.get('commit', '')):
        fail('release-signer', 'provider release signer identity is invalid')
    source_ref = release['ref']
    if not isinstance(source_ref, str) or not source_ref.startswith(('refs/heads/', 'refs/tags/')):
        fail('release-signer', 'provider release source ref is not repository-scoped')

    materials = release['materials']
    if not isinstance(materials, dict) or set(materials) != {
        'source_lock', 'upstream_archive', 'generator_configuration'
    }:
        fail('release-materials', 'provider release material inventory is incomplete')
    for name, entry in materials.items():
        if not isinstance(entry, dict) or set(entry) != {'path', 'sha256'}:
            fail('release-materials', f'{name} material fields are incomplete')
        plain_name(entry['path'], f'{name} path')
    if materials['source_lock']['path'] != 'rust-analyzer-2026-07-27.json':
        fail('release-materials', 'source lock material path is not reviewed')
    material_paths = {
        'source_lock': repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json',
        'upstream_archive': fixture_root / materials['upstream_archive']['path'],
        'generator_configuration': fixture_root / materials['generator_configuration']['path'],
    }
    observed_materials = {}
    for name, path in material_paths.items():
        entry = materials.get(name)
        expected = require_digest(entry['sha256'], f'{name} digest')
        actual = digest(path, MAX_JSON_BYTES if name != 'upstream_archive' else 512 * 1024 * 1024)
        if actual != expected:
            fail('release-materials', f'{name} material digest does not match')
        observed_materials[name] = actual

    composition = release['composition']
    if not isinstance(composition, dict) or set(composition) != COMPOSITION_FIELDS:
        fail('attestation-composition', 'provider composition fields are incomplete')
    for field, value in composition.items():
        if field == 'pack_builder_commit':
            if value != release['commit']:
                fail('attestation-composition', 'pack builder commit is not signer-bound')
        else:
            require_digest(value, field)
    if composition['source_lock_sha256'] != observed_materials['source_lock']:
        fail('attestation-composition', 'composition does not bind the source lock')
    if composition['upstream_archive_sha256'] != observed_materials['upstream_archive']:
        fail('attestation-composition', 'composition does not bind the upstream archive')
    if composition['generator_configuration_sha256'] != observed_materials['generator_configuration']:
        fail('attestation-composition', 'composition does not bind generator configuration')

    subjects = release['subjects']
    if not isinstance(subjects, list) or [item.get('role') for item in subjects] != [
        'pack', 'manifest', 'sbom'
    ]:
        fail('release-subjects', 'provider release must bind pack, manifest, and SBOM subjects')
    subject_paths = [item.get('path') for item in subjects]
    if len(set(subject_paths)) != len(subject_paths):
        fail('release-subjects', 'provider release subject paths must be unique')
    for subject in subjects:
        if not isinstance(subject, dict) or set(subject) != {'role', 'path', 'sha256', 'attestation'}:
            fail('release-subjects', 'provider release subject fields are incomplete')
        subject_path = fixture_root / plain_name(subject['path'], 'subject path')
        attestation_name = plain_name(subject['attestation'], 'attestation path')
        if attestation_name != f"{subject['path']}.attestation.json":
            fail('release-subjects', 'provider attestation name is not subject-bound')
        attestation_path = fixture_root / attestation_name
        expected = require_digest(subject['sha256'], 'subject digest')
        if digest(subject_path) != expected:
            fail('attestation-subject', f"{subject['path']} digest does not match")
        if subject['role'] == 'manifest' and expected != composition['pack_manifest_sha256']:
            fail('attestation-composition', 'composition does not bind the pack manifest subject')
        if subject['role'] == 'sbom' and expected != composition['sbom_sha256']:
            fail('attestation-composition', 'composition does not bind the SBOM subject')
        verify_attestation(attestation_path, subject, release, composition)
    print(json.dumps({'status': 'verified', 'subjects': len(subjects)}, separators=(',', ':')))


def collect_statements(value):
    statements = []
    if isinstance(value, dict):
        if {'predicateType', 'subject', 'predicate'}.issubset(value):
            statements.append(value)
        for child in value.values():
            statements.extend(collect_statements(child))
    elif isinstance(value, list):
        for child in value:
            statements.extend(collect_statements(child))
    return statements


def verify_signed_statement(release_root, release, subject, composition):
    subject_path = release_root / plain_name(subject['path'], 'subject path')
    bundle_name = plain_name(subject['attestation'], 'attestation path')
    if bundle_name != f"{subject['path']}.attestation.json":
        fail('release-subjects', 'provider attestation name is not subject-bound')
    bundle_path = release_root / bundle_name
    read_regular(bundle_path, MAX_JSON_BYTES, 'attestation-bundle')
    command = [
        'gh', 'attestation', 'verify', str(subject_path),
        '--bundle', str(bundle_path),
        '--repo', REPOSITORY,
        '--signer-workflow', f'{REPOSITORY}/{WORKFLOW}',
        '--source-ref', release['ref'],
        '--source-digest', release['commit'],
        '--cert-oidc-issuer', ISSUER,
        '--predicate-type', PREDICATE_TYPE,
        '--format', 'json',
    ]
    try:
        completed = subprocess.run(
            command, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=60, env=os.environ.copy()
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail('attestation-signature', f'could not run gh attestation verification: {exc}')
    if completed.returncode != 0:
        detail = completed.stderr[:4096].decode('utf-8', errors='replace').strip()
        fail('attestation-signature', f'{bundle_name} did not verify: {detail}')
    if not completed.stdout or len(completed.stdout) > MAX_JSON_BYTES:
        fail('attestation-statement', f'{bundle_name} verification output is outside its byte limit')
    try:
        verified = json.loads(completed.stdout.decode('utf-8'))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail('attestation-statement', f'{bundle_name} verification output is not JSON: {exc}')
    unique = {
        json.dumps(statement, separators=(',', ':'), sort_keys=True): statement
        for statement in collect_statements(verified)
    }
    if len(unique) != 1:
        fail('attestation-statement', f'{bundle_name} did not yield one verified statement')
    statement = next(iter(unique.values()))
    expected_subject = {'name': subject['path'], 'digest': {'sha256': subject['sha256']}}
    if statement.get('predicateType') != PREDICATE_TYPE:
        fail('attestation-predicate', f'{bundle_name} has an unexpected predicate type')
    if statement.get('subject') != [expected_subject]:
        fail('attestation-subject', f'{bundle_name} does not bind its exact subject')
    if statement.get('predicate') != {'composition': composition}:
        fail('attestation-composition', f'{bundle_name} omits or changes composition materials')


def verify_signed_release(repo_root, release_root, release_path):
    release = read_json(release_path, 'release-metadata')
    required = {
        'schema_version', 'kind', 'repository', 'workflow', 'ref', 'commit', 'issuer',
        'materials', 'composition', 'subjects',
    }
    if set(release) != required:
        fail('release-metadata', f'{release_path.name} fields are not strict')
    if release['schema_version'] != 1 or release['kind'] != 'pre_commit_review_provider_release':
        fail('release-metadata', f'{release_path.name} identity is invalid')
    if release['repository'] != REPOSITORY or release['workflow'] != WORKFLOW:
        fail('release-signer', f'{release_path.name} names another repository or workflow')
    expected_ref = os.environ.get('GITHUB_REF')
    expected_commit = os.environ.get('GITHUB_SHA')
    if (
        release['issuer'] != ISSUER
        or release.get('ref') != expected_ref
        or release.get('commit') != expected_commit
        or not COMMIT.fullmatch(release.get('commit', ''))
    ):
        fail('release-signer', f'{release_path.name} signer identity does not match this workflow')

    materials = release.get('materials')
    if not isinstance(materials, dict) or set(materials) != {
        'source_lock', 'upstream_archive', 'generator_configuration'
    }:
        fail('release-materials', f'{release_path.name} material inventory is incomplete')
    for name, entry in materials.items():
        if not isinstance(entry, dict) or set(entry) != {'path', 'sha256'}:
            fail('release-materials', f'{name} material fields are incomplete')
        plain_name(entry['path'], f'{name} path')
        require_digest(entry['sha256'], f'{name} digest')

    config_path = release_root / materials['generator_configuration']['path']
    config_digest = digest(config_path, MAX_JSON_BYTES)
    if config_digest != materials['generator_configuration']['sha256']:
        fail('release-materials', 'generator configuration digest does not match')
    config = read_json(config_path, 'generator-configuration')
    if set(config) != {
        'compression', 'gzip_mtime', 'gzip_os', 'pack_version', 'platform_id',
        'rust_toolchain', 'tar_format'
    }:
        fail('generator-configuration', 'generator configuration fields are not strict')
    platform = config.get('platform_id')
    expected_config = {
        'compression': 'gzip-level-9', 'gzip_mtime': 0, 'gzip_os': 255,
        'pack_version': PACK_VERSION, 'platform_id': platform,
        'rust_toolchain': RUST_TOOLCHAIN, 'tar_format': 'posix-ustar',
    }
    if platform not in PLATFORMS or config != expected_config:
        fail('generator-configuration', 'generator configuration is not reviewed')
    if release_path.name != f'rust-analyzer-{platform}.release.json':
        fail('release-metadata', 'release metadata basename does not match its platform')
    if materials['generator_configuration']['path'] != f'rust-analyzer-{platform}.generator-config.json':
        fail('release-materials', 'generator configuration basename is not platform-bound')
    if materials['source_lock']['path'] != 'rust-analyzer-2026-07-27.json':
        fail('release-materials', 'source lock material path is not reviewed')
    source_lock_path = (
        repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json'
    )
    source_lock_digest = digest(source_lock_path, MAX_JSON_BYTES)
    if (
        source_lock_digest != SOURCE_LOCK_SHA256
        or materials['source_lock']['sha256'] != SOURCE_LOCK_SHA256
    ):
        fail('release-materials', 'source lock is not the reviewed byte sequence')
    source_lock = read_json(source_lock_path, 'source-lock')
    try:
        asset = next(item for item in source_lock['assets'] if item['platform_id'] == platform)
    except (KeyError, StopIteration, TypeError):
        fail('release-materials', 'source lock has no reviewed platform asset')
    upstream_digest = materials['upstream_archive']['sha256']
    if materials['upstream_archive']['path'] != asset.get('archive_name'):
        fail('release-materials', 'upstream archive basename does not match the source lock')
    if upstream_digest != asset.get('archive_sha256'):
        fail('release-materials', 'upstream archive digest does not match the source lock')

    composition = release.get('composition')
    if not isinstance(composition, dict) or set(composition) != COMPOSITION_FIELDS:
        fail('attestation-composition', 'provider composition fields are incomplete')
    for field, value in composition.items():
        if field == 'pack_builder_commit':
            if value != release['commit']:
                fail('attestation-composition', 'pack builder commit is not signer-bound')
        else:
            require_digest(value, field)
    if composition['source_lock_sha256'] != source_lock_digest:
        fail('attestation-composition', 'composition does not bind the source lock')
    if composition['upstream_archive_sha256'] != upstream_digest:
        fail('attestation-composition', 'composition does not bind the upstream archive')
    if composition['generator_configuration_sha256'] != config_digest:
        fail('attestation-composition', 'composition does not bind generator configuration')
    predicate = read_json(
        release_root / f'rust-analyzer-{platform}.composition-predicate.json',
        'composition-predicate'
    )
    if predicate != {'composition': composition}:
        fail('attestation-composition', 'canonical predicate does not match release composition')

    subjects = release.get('subjects')
    expected_paths = [
        f'pre-commit-review-rust-analyzer-{PACK_VERSION}-{platform}.tar.gz',
        f'rust-analyzer-{platform}.pack-manifest.json',
        f'rust-analyzer-{platform}.sbom.cdx.json',
    ]
    if (
        not isinstance(subjects, list)
        or [item.get('role') for item in subjects] != ['pack', 'manifest', 'sbom']
        or [item.get('path') for item in subjects] != expected_paths
    ):
        fail('release-subjects', 'provider release subjects are not platform-bound')
    for subject in subjects:
        if not isinstance(subject, dict) or set(subject) != {'role', 'path', 'sha256', 'attestation'}:
            fail('release-subjects', 'provider release subject fields are incomplete')
        subject_path = release_root / plain_name(subject['path'], 'subject path')
        expected = require_digest(subject['sha256'], 'subject digest')
        if digest(subject_path) != expected:
            fail('attestation-subject', f"{subject['path']} digest does not match")
        if subject['role'] == 'manifest' and expected != composition['pack_manifest_sha256']:
            fail('attestation-composition', 'composition does not bind the pack manifest subject')
        if subject['role'] == 'sbom' and expected != composition['sbom_sha256']:
            fail('attestation-composition', 'composition does not bind the SBOM subject')
        verify_signed_statement(release_root, release, subject, composition)


def verify_signed_releases(repo_root, release_root):
    release_paths = sorted(release_root.glob('rust-analyzer-*.release.json'))
    expected_names = {f'rust-analyzer-{platform}.release.json' for platform in PLATFORMS}
    if {path.name for path in release_paths} != expected_names:
        fail('release-metadata', 'signed provider release metadata is not complete')
    for release_path in release_paths:
        verify_signed_release(repo_root, release_root, release_path)
    print(json.dumps({
        'status': 'verified', 'releases': len(release_paths),
        'subjects': len(release_paths) * 3,
    }, separators=(',', ':')))


try:
    root = Path(sys.argv[1]).resolve()
    mode = sys.argv[2]
    release_root = Path(sys.argv[3]).resolve()
    if not release_root.is_dir():
        fail('fixture-root', 'provider release root is not a directory')
    if mode == '--fixture':
        verify(root, release_root)
    else:
        verify_signed_releases(root, release_root)
except VerificationError as exc:
    print(f'provider release verification failed: {exc.code}: {exc}', file=sys.stderr)
    sys.exit(1)
except (KeyError, TypeError, ValueError) as exc:
    print(f'provider release verification failed: release-metadata: {exc}', file=sys.stderr)
    sys.exit(1)
PY
