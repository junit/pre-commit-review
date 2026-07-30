#!/usr/bin/env bash
set -euo pipefail

repo_root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
fixture="$repo_root/tests/fixtures/provider-release"
verifier="$repo_root/scripts/verify_provider_release.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

expect_rejection() {
  case_name=$1
  expected=$2
  if "$verifier" --fixture "$case_name" >"$tmp_dir/stdout" 2>"$tmp_dir/stderr"; then
    printf 'provider release verifier unexpectedly accepted %s\n' "$case_name" >&2
    exit 1
  fi
  grep -Fq "$expected" "$tmp_dir/stderr" || {
    printf 'provider release verifier did not report %s\n' "$expected" >&2
    cat "$tmp_dir/stderr" >&2
    exit 1
  }
}

"$verifier" --fixture "$fixture" >/dev/null

archive_case="$tmp_dir/archive"
cp -R "$fixture" "$archive_case"
printf 'tampered\n' >>"$archive_case/upstream-archive.bin"
expect_rejection "$archive_case" 'release-materials'

composition_case="$tmp_dir/composition"
cp -R "$fixture" "$composition_case"
python3 - "$composition_case/release.json" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
value = json.loads(path.read_text(encoding='utf-8'))
del value['composition']['sbom_sha256']
path.write_text(json.dumps(value, separators=(',', ':')), encoding='utf-8')
PY
expect_rejection "$composition_case" 'attestation-composition'

signed_fixture="$tmp_dir/signed"
fake_bin="$tmp_dir/bin"
mkdir -p "$signed_fixture" "$fake_bin"
cp -R "$fixture/." "$signed_fixture"
python3 - "$signed_fixture" "$repo_root" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
repo_root = Path(sys.argv[2])
source_lock = repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json'
source_lock_value = json.loads(source_lock.read_text(encoding='utf-8'))
asset = next(item for item in source_lock_value['assets'] if item['platform_id'] == 'linux-amd64')
source_lock_sha256 = hashlib.sha256(source_lock.read_bytes()).hexdigest()
config = root / 'rust-analyzer-linux-amd64.generator-config.json'
config.write_text(json.dumps({
    'compression': 'gzip-level-9', 'gzip_mtime': 0, 'gzip_os': 255,
    'pack_version': '2026.07.27-pcr.1', 'platform_id': 'linux-amd64',
    'rust_toolchain': '1.95.0', 'tar_format': 'posix-ustar'
}, separators=(',', ':')), encoding='utf-8')
config_sha256 = hashlib.sha256(config.read_bytes()).hexdigest()
release_path = root / 'release.json'
release = json.loads(release_path.read_text(encoding='utf-8'))
release['materials']['source_lock'] = {
    'path': source_lock.name, 'sha256': source_lock_sha256
}
release['materials']['upstream_archive'] = {
    'path': asset['archive_name'], 'sha256': asset['archive_sha256']
}
release['materials']['generator_configuration'] = {
    'path': config.name, 'sha256': config_sha256
}
release['composition']['source_lock_sha256'] = source_lock_sha256
release['composition']['upstream_archive_sha256'] = asset['archive_sha256']
release['composition']['generator_configuration_sha256'] = config_sha256
subject_names = {
    'pack': 'pre-commit-review-rust-analyzer-2026.07.27-pcr.1-linux-amd64.tar.gz',
    'manifest': 'rust-analyzer-linux-amd64.pack-manifest.json',
    'sbom': 'rust-analyzer-linux-amd64.sbom.cdx.json',
}
for subject in release['subjects']:
    old_subject = root / subject['path']
    old_bundle = root / subject['attestation']
    subject['path'] = subject_names[subject['role']]
    subject['attestation'] = f"{subject['path']}.attestation.json"
    old_subject.rename(root / subject['path'])
    old_bundle.rename(root / subject['attestation'])
    bundle = root / subject['attestation']
    statement = json.loads(bundle.read_text(encoding='utf-8'))
    statement['subject'][0]['name'] = subject['path']
    statement['predicate']['composition'] = release['composition']
    bundle.write_text(json.dumps(statement, separators=(',', ':')), encoding='utf-8')
(root / 'rust-analyzer-linux-amd64.composition-predicate.json').write_text(
    json.dumps({'composition': release['composition']}, separators=(',', ':')), encoding='utf-8'
)
(root / 'rust-analyzer-linux-amd64.release.json').write_text(
    json.dumps(release, separators=(',', ':')), encoding='utf-8'
)
release_path.unlink()
(root / 'generator-config.json').unlink()
PY

python3 - "$signed_fixture" "$repo_root" <<'PY'
import hashlib
import json
import shutil
import sys
from pathlib import Path

root = Path(sys.argv[1])
repo_root = Path(sys.argv[2])
linux_release = json.loads(
    (root / 'rust-analyzer-linux-amd64.release.json').read_text(encoding='utf-8')
)
source_lock_template = json.loads(
    (repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json')
    .read_text(encoding='utf-8')
)
linux_subjects = {item['role']: item for item in linux_release['subjects']}
source_lock_path = (
    repo_root / 'third_party_artifacts/sources/rust-analyzer-2026-07-27.json'
)
source_lock_sha256 = hashlib.sha256(source_lock_path.read_bytes()).hexdigest()

for platform in ['darwin-amd64', 'darwin-arm64', 'windows-amd64']:
    release = json.loads(json.dumps(linux_release))
    asset = next(item for item in source_lock_template['assets'] if item['platform_id'] == platform)

    config = root / f'rust-analyzer-{platform}.generator-config.json'
    config.write_text(json.dumps({
        'compression': 'gzip-level-9', 'gzip_mtime': 0, 'gzip_os': 255,
        'pack_version': '2026.07.27-pcr.1', 'platform_id': platform,
        'rust_toolchain': '1.95.0', 'tar_format': 'posix-ustar'
    }, separators=(',', ':')), encoding='utf-8')
    config_sha256 = hashlib.sha256(config.read_bytes()).hexdigest()
    release['materials'] = {
        'source_lock': {'path': source_lock_path.name, 'sha256': source_lock_sha256},
        'upstream_archive': {
            'path': asset['archive_name'], 'sha256': asset['archive_sha256']
        },
        'generator_configuration': {'path': config.name, 'sha256': config_sha256},
    }
    release['composition']['source_lock_sha256'] = source_lock_sha256
    release['composition']['upstream_archive_sha256'] = asset['archive_sha256']
    release['composition']['generator_configuration_sha256'] = config_sha256
    names = {
        'pack': f'pre-commit-review-rust-analyzer-2026.07.27-pcr.1-{platform}.tar.gz',
        'manifest': f'rust-analyzer-{platform}.pack-manifest.json',
        'sbom': f'rust-analyzer-{platform}.sbom.cdx.json',
    }
    for subject in release['subjects']:
        source_subject = linux_subjects[subject['role']]
        subject['path'] = names[subject['role']]
        subject['attestation'] = f"{subject['path']}.attestation.json"
        shutil.copy2(root / source_subject['path'], root / subject['path'])
        statement = json.loads(
            (root / source_subject['attestation']).read_text(encoding='utf-8')
        )
        statement['subject'][0]['name'] = subject['path']
        statement['predicate']['composition'] = release['composition']
        (root / subject['attestation']).write_text(
            json.dumps(statement, separators=(',', ':')), encoding='utf-8'
        )
    (root / f'rust-analyzer-{platform}.composition-predicate.json').write_text(
        json.dumps({'composition': release['composition']}, separators=(',', ':')),
        encoding='utf-8'
    )
    (root / f'rust-analyzer-{platform}.release.json').write_text(
        json.dumps(release, separators=(',', ':')), encoding='utf-8'
    )
PY

rm "$signed_fixture/upstream-archive.bin"

cat >"$fake_bin/gh" <<'PY'
#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
if args[:2] != ['attestation', 'verify'] or len(args) < 3:
    raise SystemExit('unexpected gh command')
expected = {
    '--repo': 'junit/pre-commit-review',
    '--signer-workflow': 'junit/pre-commit-review/.github/workflows/artifact-pack-release.yml',
    '--source-ref': os.environ['GITHUB_REF'],
    '--source-digest': os.environ['GITHUB_SHA'],
    '--cert-oidc-issuer': 'https://token.actions.githubusercontent.com',
    '--predicate-type': 'pre-commit-review.artifact-pack/v1',
    '--format': 'json',
}
for name, value in expected.items():
    try:
        observed = args[args.index(name) + 1]
    except (ValueError, IndexError):
        raise SystemExit(f'missing {name}')
    if observed != value:
        raise SystemExit(f'wrong {name}: {observed}')
bundle = Path(args[args.index('--bundle') + 1])
value = json.loads(bundle.read_text(encoding='utf-8'))
statement = {
    '_type': 'https://in-toto.io/Statement/v1',
    'predicateType': value['predicateType'],
    'subject': value['subject'],
    'predicate': value['predicate'],
}
if os.environ.get('FAKE_GH_TAMPER') == 'composition':
    statement['predicate']['composition']['sbom_sha256'] = '0' * 64
with open(os.environ['FAKE_GH_LOG'], 'a', encoding='utf-8') as log:
    log.write(f"{args[2]}\n")
print(json.dumps([{'verificationResult': {'statement': statement}}], separators=(',', ':')))
PY
chmod +x "$fake_bin/gh"

export PATH="$fake_bin:$PATH"
export GITHUB_REF='refs/tags/artifact-rust-analyzer-2026.07.27-pcr.1'
export GITHUB_SHA='1111111111111111111111111111111111111111'
export FAKE_GH_LOG="$tmp_dir/gh.log"
"$verifier" --signed-release-root "$signed_fixture" >/dev/null
test "$(wc -l <"$FAKE_GH_LOG")" -eq 12

incomplete_fixture="$tmp_dir/incomplete"
cp -R "$signed_fixture" "$incomplete_fixture"
rm "$incomplete_fixture/rust-analyzer-windows-amd64.release.json"
if "$verifier" --signed-release-root "$incomplete_fixture" \
  >"$tmp_dir/incomplete-stdout" 2>"$tmp_dir/incomplete-stderr"; then
  printf 'signed provider verifier accepted an incomplete platform release set\n' >&2
  exit 1
fi
grep -Fq 'release-metadata' "$tmp_dir/incomplete-stderr"

duplicate_fixture="$tmp_dir/duplicate-platform"
cp -R "$signed_fixture" "$duplicate_fixture"
cp "$duplicate_fixture/rust-analyzer-darwin-amd64.release.json" \
  "$duplicate_fixture/rust-analyzer-windows-amd64.release.json"
if "$verifier" --signed-release-root "$duplicate_fixture" \
  >"$tmp_dir/duplicate-stdout" 2>"$tmp_dir/duplicate-stderr"; then
  printf 'signed provider verifier accepted a release under another platform basename\n' >&2
  exit 1
fi
grep -Fq 'release-metadata' "$tmp_dir/duplicate-stderr"

source_lock_fixture="$tmp_dir/source-lock-drift"
cp -R "$signed_fixture" "$source_lock_fixture"
python3 - "$source_lock_fixture/rust-analyzer-linux-amd64.release.json" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
value = json.loads(path.read_text(encoding='utf-8'))
value['materials']['source_lock']['sha256'] = '0' * 64
value['composition']['source_lock_sha256'] = '0' * 64
path.write_text(json.dumps(value, separators=(',', ':')), encoding='utf-8')
PY
if "$verifier" --signed-release-root "$source_lock_fixture" \
  >"$tmp_dir/source-lock-stdout" 2>"$tmp_dir/source-lock-stderr"; then
  printf 'signed provider verifier accepted an unreviewed source lock digest\n' >&2
  exit 1
fi
grep -Fq 'source lock is not the reviewed byte sequence' "$tmp_dir/source-lock-stderr"

if FAKE_GH_TAMPER=composition "$verifier" --signed-release-root "$signed_fixture" \
  >"$tmp_dir/signed-stdout" 2>"$tmp_dir/signed-stderr"; then
  printf 'signed provider verifier accepted a changed composition statement\n' >&2
  exit 1
fi
grep -Fq 'attestation-composition' "$tmp_dir/signed-stderr"

printf 'provider release verifier tests passed\n'
