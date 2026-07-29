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

fake_binary="$tmp_dir/gitleaks"
cat > "$fake_binary" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '8.30.1'
EOF
chmod +x "$fake_binary"

pack="$tmp_dir/gitleaks-darwin-arm64.tar.gz"
rebuild="$tmp_dir/gitleaks-darwin-arm64-rebuild.tar.gz"
record="$tmp_dir/gitleaks.record.json"
common_args=(
  --kind gitleaks
  --platform-id darwin-arm64
  --pack-version 8.30.1-pcr.1
  --source-root "$repo_root"
  --manifest "$repo_root/third_party_artifacts/manifest.json"
  --source-lock "$repo_root/third_party_artifacts/sources/gitleaks-8.30.1.json"
  --binary "$fake_binary"
)
"$repo_root/scripts/build_artifact_pack.sh" "${common_args[@]}" \
  --output "$pack" --record-output "$record" >/dev/null
"$repo_root/scripts/build_artifact_pack.sh" "${common_args[@]}" \
  --output "$rebuild" >/dev/null
cmp "$pack" "$rebuild" || fail 'identical inputs did not produce identical Gitleaks bytes'

python3 - "$pack" "$record" <<'PY'
import json
import sys
import tarfile

pack, record_path = sys.argv[1:]
with tarfile.open(pack, 'r:gz') as archive:
    names = archive.getnames()
    expected = [
        'bin', 'bin/gitleaks', 'licenses', 'licenses/GITLEAKS-LICENSE',
        'pack-manifest.json', 'sbom.cdx.json',
    ]
    if names != expected:
        raise SystemExit(f'unexpected Gitleaks members: {names!r}')
    if any(member.issym() or member.islnk() for member in archive.getmembers()):
        raise SystemExit('Gitleaks pack contains a link')
record = json.loads(open(record_path, encoding='utf-8').read())
if record['artifact_id'] != 'gitleaks' or record['platform_id'] != 'darwin-arm64':
    raise SystemExit('Gitleaks record identity is not bound to the selected platform')
PY

core="$tmp_dir/core-darwin-arm64.tar.gz"
"$repo_root/scripts/build_artifact_pack.sh" \
  --kind core --platform-id darwin-arm64 --pack-version 0.1.0-pcr.1 \
  --source-root "$repo_root" --output "$core" >/dev/null
python3 - "$core" <<'PY'
import sys
import tarfile

with tarfile.open(sys.argv[1], 'r:gz') as archive:
    names = archive.getnames()
    required = {
        'runtime', 'runtime/distribution', 'runtime/distribution/manifest.json',
        'runtime/distribution/revocations.json', 'scripts',
        'scripts/bin', 'scripts/bin/collect_diff_context-darwin-arm64',
        'core-pack-manifest.json', 'core-sbom.cdx.json',
    }
    missing = required.difference(names)
    if missing:
        raise SystemExit(f'core pack is missing required members: {sorted(missing)}')
    if any(name.startswith('scripts/bin/gitleaks-') for name in names):
        raise SystemExit('core pack contains a third-party Gitleaks binary')
PY

printf 'artifact distribution tests passed\n'
