#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
runner="$repo_root/scripts/run_static_analysis.sh"
helper="$repo_root/scripts/collect_diff_context.sh"
validator="$repo_root/scripts/validate_schemas.py"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'static analysis execution modes test failed: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

control_fingerprint() {
  local repository="$1"
  local source="$2"
  local output
  output="$tmp_dir/control-${source}-$(basename "$repository").out"
  (
    cd "$repository"
    PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source "$source" --control-plane
  ) >"$output" 2>/dev/null
  python3 - "$output" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
}

analyzer="$tmp_dir/mode-analyzer.py"
cat >"$analyzer" <<'PY'
#!/usr/bin/env python3
import json
import os
import pathlib
import sys

source = os.environ['PRE_COMMIT_REVIEW_SOURCE']
text = pathlib.Path('src/app.py').read_text(encoding='utf-8')
if source == 'unstaged' and '# unstaged candidate' not in text:
    print('unstaged snapshot did not contain working-tree bytes', file=sys.stderr)
    raise SystemExit(20)
if source == 'branch':
    if '# branch candidate' not in text or '# working-only' in text:
        print('branch snapshot did not contain exactly HEAD bytes', file=sys.stderr)
        raise SystemExit(21)
print(json.dumps({
    'version': '2.1.0',
    'runs': [{
        'tool': {'driver': {
            'name': 'fixture-modes',
            'version': '3.0.0',
            'rules': [{
                'id': 'python/dynamic-eval',
                'properties': {'tags': ['security', 'cwe-95'], 'precision': 'high'},
            }],
        }},
        'results': [{
            'ruleId': 'python/dynamic-eval',
            'level': 'error',
            'message': {'text': 'Dynamic evaluation accepts untrusted input.'},
            'locations': [{'physicalLocation': {
                'artifactLocation': {'uri': 'src/app.py'},
                'region': {'startLine': 2, 'endLine': 2},
            }}],
        }],
    }],
}))
PY
chmod +x "$analyzer"

profile="$tmp_dir/profile.json"
python3 - "$profile" "$analyzer" "$(sha256_file "$analyzer")" <<'PY'
import json
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_text(json.dumps({
    'schema_version': 1,
    'kind': 'static_analysis_profile',
    'name': 'source mode profile',
    'tool': {'name': 'fixture-modes', 'version': '3.0.0'},
    'executable': {'path': sys.argv[2], 'sha256': sys.argv[3]},
    'arguments': [],
    'output_format': 'sarif',
    'success_exit_codes': [0],
    'limits': {
        'timeout_seconds': 10,
        'max_output_bytes': 1000000,
        'max_snapshot_bytes': 20000000,
        'max_snapshot_files': 1000,
    },
    'repository_configuration': 'disabled',
    'network_access': 'offline-required',
}), encoding='utf-8')
PY
profile_hash="$(sha256_file "$profile")"

unstaged_repo="$tmp_dir/unstaged-repo"
mkdir -p "$unstaged_repo/src"
git -C "$unstaged_repo" init -q
git -C "$unstaged_repo" config user.email a@example.com
git -C "$unstaged_repo" config user.name A
cat >"$unstaged_repo/src/app.py" <<'EOF'
def execute(value):
    return value.strip()
EOF
git -C "$unstaged_repo" add src/app.py
git -C "$unstaged_repo" commit -q -m baseline
cat >"$unstaged_repo/src/app.py" <<'EOF'
def execute(value):
    eval(value)  # unstaged candidate
    return value.strip()
EOF
unstaged_fingerprint="$(control_fingerprint "$unstaged_repo" unstaged)"
(
  cd "$unstaged_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source unstaged --expect-scope "$unstaged_fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$tmp_dir/unstaged.out" 2>"$tmp_dir/unstaged.err"
python3 "$validator" --static-execution-output "$tmp_dir/unstaged.out" >/dev/null \
  || fail 'unstaged execution output did not validate'
jq -e '.execution.status == "completed"' \
  < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/unstaged.out") >/dev/null \
  || fail 'unstaged candidate was not materialized from tracked working-tree bytes'

branch_repo="$tmp_dir/branch-repo"
mkdir -p "$branch_repo/src"
git -C "$branch_repo" init -q
git -C "$branch_repo" config user.email a@example.com
git -C "$branch_repo" config user.name A
cat >"$branch_repo/src/app.py" <<'EOF'
def execute(value):
    return value.strip()
EOF
git -C "$branch_repo" add src/app.py
git -C "$branch_repo" commit -q -m baseline
git -C "$branch_repo" switch -q -c feature
cat >"$branch_repo/src/app.py" <<'EOF'
def execute(value):
    eval(value)  # branch candidate
    return value.strip()
EOF
git -C "$branch_repo" add src/app.py
git -C "$branch_repo" commit -q -m feature
cat >>"$branch_repo/src/app.py" <<'EOF'
# working-only
EOF
branch_fingerprint="$(control_fingerprint "$branch_repo" branch)"
(
  cd "$branch_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source branch --expect-scope "$branch_fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$tmp_dir/branch.out" 2>"$tmp_dir/branch.err"
python3 "$validator" --static-execution-output "$tmp_dir/branch.out" >/dev/null \
  || fail 'branch execution output did not validate'
jq -e '.execution.status == "completed"' \
  < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/branch.out") >/dev/null \
  || fail 'branch candidate was not materialized from HEAD bytes'

submodule_source="$tmp_dir/submodule-source"
mkdir -p "$submodule_source"
git -C "$submodule_source" init -q
git -C "$submodule_source" config user.email a@example.com
git -C "$submodule_source" config user.name A
printf '%s\n' 'submodule content' >"$submodule_source/content.txt"
git -C "$submodule_source" add content.txt
git -C "$submodule_source" commit -q -m baseline

submodule_parent="$tmp_dir/submodule-parent"
mkdir -p "$submodule_parent/src"
git -C "$submodule_parent" init -q
git -C "$submodule_parent" config user.email a@example.com
git -C "$submodule_parent" config user.name A
cat >"$submodule_parent/src/app.py" <<'EOF'
def execute(value):
    return value.strip()
EOF
git -C "$submodule_parent" add src/app.py
git -C "$submodule_parent" commit -q -m baseline
git -c protocol.file.allow=always -C "$submodule_parent" submodule add -q \
  "$submodule_source" vendor/sub
submodule_fingerprint="$(control_fingerprint "$submodule_parent" staged)"
if ! (
  cd "$submodule_parent"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$submodule_fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$tmp_dir/submodule.out" 2>"$tmp_dir/submodule.err"; then
  cat "$tmp_dir/submodule.err" >&2
  fail 'staged gitlink execution failed'
fi
python3 "$validator" --static-execution-output "$tmp_dir/submodule.out" >/dev/null \
  || fail 'staged gitlink execution output did not validate'
jq -e '.execution.status == "completed"' \
  < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/submodule.out") >/dev/null \
  || fail 'tracked gitlink was not safely omitted from the snapshot'

printf 'static analysis execution source-mode tests passed\n'
