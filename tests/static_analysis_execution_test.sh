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
  printf 'static analysis execution test failed: %s\n' "$*" >&2
  exit 1
}

for surface in "$runner" "$helper" "$repo_root/scripts/lib/static_analysis_cli.sh"; do
  if grep -Eni 'rust-analyzer|repository-context-provider-cli|run_repository_context_provider|artifacts[[:space:]]+(verify|provision)|runtime/providers|provider-registry|rustup[[:space:]]+toolchain[[:space:]]+install|cargo[[:space:]]+install[[:space:]]+rust-analyzer|direct[-_]upstream|global[-_]registry' "$surface"; then
    fail "static-analysis execution surface can reach a provider or fallback: $surface"
  fi
done

static_analysis_bin="$repo_root/collect-diff-context-cli/target/release/static-analysis-cli"
[ -x "$static_analysis_bin" ] || fail 'release static-analysis-cli is unavailable'
export PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$static_analysis_bin"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

write_profile() {
  local output="$1"
  local executable="$2"
  local executable_hash="$3"
  local tool_name="$4"
  local tool_version="$5"
  local output_format="$6"
  local timeout_seconds="$7"
  local max_output_bytes="$8"
  python3 - "$output" "$executable" "$executable_hash" "$tool_name" \
    "$tool_version" "$output_format" "$timeout_seconds" "$max_output_bytes" <<'PY'
import json
import pathlib
import sys

payload = {
    'schema_version': 1,
    'kind': 'static_analysis_profile',
    'name': f'{sys.argv[4]} controlled profile',
    'tool': {'name': sys.argv[4], 'version': sys.argv[5]},
    'executable': {'path': sys.argv[2], 'sha256': sys.argv[3]},
    'arguments': [],
    'output_format': sys.argv[6],
    'success_exit_codes': [0],
    'limits': {
        'timeout_seconds': int(sys.argv[7]),
        'max_output_bytes': int(sys.argv[8]),
        'max_snapshot_bytes': 20_000_000,
        'max_snapshot_files': 1000,
    },
    'repository_configuration': 'disabled',
    'network_access': 'offline-required',
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload), encoding='utf-8')
PY
}

fixture="$tmp_dir/repo"
mkdir -p "$fixture/src"
git -C "$fixture" init -q
git -C "$fixture" config user.email a@example.com
git -C "$fixture" config user.name A
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    return value.strip()
EOF
git -C "$fixture" add src/app.py
git -C "$fixture" commit -q -m baseline
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    eval(value)
    return value.strip()
EOF
git -C "$fixture" add src/app.py
cat >>"$fixture/src/app.py" <<'EOF'
# unstaged-only marker
EOF

control="$tmp_dir/control.out"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source staged --control-plane
) >"$control" 2>/dev/null
fingerprint="$(python3 - "$control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"

marker="$tmp_dir/analyzer-ran"
analyzer="$tmp_dir/trusted-analyzer.py"
python3 - "$analyzer" "$marker" <<'PY'
import pathlib
import sys

output = pathlib.Path(sys.argv[1])
marker = sys.argv[2]
program = f'''#!/usr/bin/env python3
import json
import os
import pathlib
import sys

text = pathlib.Path("src/app.py").read_text(encoding="utf-8")
if "eval(value)" not in text or "unstaged-only" in text:
    print("snapshot does not match the staged candidate", file=sys.stderr)
    raise SystemExit(7)
if pathlib.Path(".git").exists():
    print("snapshot unexpectedly contains Git metadata", file=sys.stderr)
    raise SystemExit(8)
try:
    pathlib.Path("source-write-probe").write_text("unexpected", encoding="utf-8")
except OSError:
    pass
else:
    print("snapshot root is writable", file=sys.stderr)
    raise SystemExit(9)
pathlib.Path({marker!r}).write_text("ran", encoding="utf-8")
scope = os.environ.get("PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT", "")
if not scope:
    raise SystemExit(10)
print(json.dumps({{
    "version": "2.1.0",
    "runs": [{{
        "tool": {{"driver": {{
            "name": "fixture-controlled",
            "version": "2.0.0",
            "rules": [{{
                "id": "python/dynamic-eval",
                "properties": {{"tags": ["security", "cwe-95"], "precision": "high"}}
            }}]
        }}}},
        "results": [{{
            "ruleId": "python/dynamic-eval",
            "level": "error",
            "message": {{"text": "Dynamic evaluation accepts untrusted input."}},
            "locations": [{{"physicalLocation": {{
                "artifactLocation": {{"uri": "src/app.py"}},
                "region": {{"startLine": 2, "endLine": 2}}
            }}}}]
        }}]
    }}]
}}))
'''
output.write_text(program, encoding='utf-8')
PY
chmod +x "$analyzer"
analyzer_hash="$(sha256_file "$analyzer")"
profile="$tmp_dir/profile.json"
write_profile "$profile" "$analyzer" "$analyzer_hash" \
  fixture-controlled 2.0.0 sarif 10 1000000
profile_hash="$(sha256_file "$profile")"
python3 "$validator" --static-profile "$profile" >/dev/null \
  || fail 'controlled execution profile did not validate'

status_before="$(git -C "$fixture" status --short --untracked-files=all)"
execution_output="$tmp_dir/execution.out"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$execution_output" 2>"$tmp_dir/execution.err"
status_after="$(git -C "$fixture" status --short --untracked-files=all)"
[ "$status_before" = "$status_after" ] || fail 'controlled execution mutated the reviewed repository'
[ -f "$marker" ] || fail 'trusted analyzer was not executed'

python3 "$validator" --static-execution-output "$execution_output" >/dev/null \
  || fail 'controlled execution output did not validate'
python3 - "$execution_output" <<'PY' || fail 'controlled execution provenance or evidence was incorrect'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
execution = json.loads(lines[lines.index('## Static Analysis Execution JSON') + 1])
evidence = json.loads(lines[lines.index('## Static Analysis Evidence JSON') + 1])
assert execution['authoritative'] is True
assert execution['execution']['status'] == 'completed'
assert execution['execution']['result_accepted'] is True
assert execution['profile']['output_format'] == 'sarif'
assert execution['profile']['limits']['timeout_seconds'] == 10
assert execution['snapshot']['files'] >= 1
assert execution['snapshot']['bytes'] > 0
assert execution['profile']['limits'] == {
    'timeout_seconds': 10,
    'max_output_bytes': 1000000,
    'max_snapshot_bytes': 20000000,
    'max_snapshot_files': 1000,
}
assert execution['isolation'] == {
    'shell': False,
    'vcs_metadata': False,
    'environment': 'allowlist',
    'source_tree': 'read-only-temporary-snapshot',
    'original_repository_path': 'not-exposed',
    'network': 'best-effort-offline-profile-required',
}
assert evidence['scope'] == execution['scope']
assert evidence['counts']['blocking_candidates'] == 1
assert evidence['reports'][0]['trust'] == 'controlled-execution'
assert evidence['reports'][0]['scope_binding'] == 'controlled-execution'
assert evidence['reports'][0]['execution_id'] == execution['execution_id']
assert evidence['findings'][0]['rule_id'] == 'python/dynamic-eval'
assert evidence['findings'][0]['line_scope'] == 'added'
PY

mock_sanitizer="$tmp_dir/mock-sanitizer.sh"
cat >"$mock_sanitizer" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
sed 's/Dynamic evaluation accepts untrusted input\./[redacted:controlled-fixture-message]/g'
cat >"$PRE_COMMIT_REVIEW_SANITIZE_REPORT" <<'REPORT'
protocol: pcr-sanitizer-v1
status: redacted
redaction_applied: yes
review_continued: yes
REPORT
SH
chmod +x "$mock_sanitizer"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SANITIZER_BIN="$mock_sanitizer" \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$tmp_dir/sanitized.out" 2>"$tmp_dir/sanitized.err"
grep -Fq '[redacted:controlled-fixture-message]' "$tmp_dir/sanitized.out" \
  || fail 'controlled execution wrapper did not release sanitized output'
if grep -Fq 'Dynamic evaluation accepts untrusted input.' "$tmp_dir/sanitized.out"; then
  fail 'controlled execution wrapper leaked sanitizer-matched analyzer text'
fi
grep -Fq 'status: redacted' "$tmp_dir/sanitized.err" \
  || fail 'controlled execution wrapper did not report redaction status'
python3 "$validator" --static-execution-output "$tmp_dir/sanitized.out" >/dev/null \
  || fail 'sanitized controlled execution no longer satisfied the linked contracts'

identity_profile="$tmp_dir/identity-mismatch-profile.json"
write_profile "$identity_profile" "$analyzer" "$analyzer_hash" \
  unexpected-tool 9.9.9 sarif 10 1000000
identity_profile_hash="$(sha256_file "$identity_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$identity_profile" --expect-profile-sha256 "$identity_profile_hash"
) >"$tmp_dir/identity-mismatch.out" 2>"$tmp_dir/identity-mismatch.err"
jq -e '
  .execution.status == "invalid-output"
  and .execution.result_accepted == false
  and .execution.failure_reason == "invalid-output"
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/identity-mismatch.out") >/dev/null \
  || fail 'tool identity mismatch was accepted as controlled evidence'

rm -f "$marker"
trusted_config_profile="$tmp_dir/trusted-config-profile.json"
python3 - "$profile" "$trusted_config_profile" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload['repository_configuration'] = 'explicitly-trusted'
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
trusted_config_hash="$(sha256_file "$trusted_config_profile")"
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$trusted_config_profile" --expect-profile-sha256 "$trusted_config_hash"
) >"$tmp_dir/trusted-config-missing-flag.out" 2>"$tmp_dir/trusted-config-missing-flag.err"; then
  fail 'runner inferred repository-configuration trust from the profile hash alone'
fi
[ ! -e "$marker" ] || fail 'analyzer ran before repository configuration was separately authorized'
grep -Fq 'requires separate --allow-repository-configuration authorization' \
  "$tmp_dir/trusted-config-missing-flag.err" \
  || fail 'missing repository-configuration authorization was not actionable'
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$trusted_config_profile" --expect-profile-sha256 "$trusted_config_hash" \
      --allow-repository-configuration
) >"$tmp_dir/trusted-config.out" 2>"$tmp_dir/trusted-config.err"
python3 "$validator" --static-execution-output "$tmp_dir/trusted-config.out" >/dev/null \
  || fail 'separately authorized repository configuration did not validate'
[ -e "$marker" ] || fail 'separately authorized repository configuration did not execute'
rm -f "$marker"
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash" \
      --allow-repository-configuration
) >"$tmp_dir/disabled-config-flag.out" 2>"$tmp_dir/disabled-config-flag.err"; then
  fail 'runner accepted repository-configuration authorization for a disabled profile'
fi
grep -Fq 'valid only for an explicitly-trusted profile' "$tmp_dir/disabled-config-flag.err" \
  || fail 'unnecessary repository-configuration authorization was not rejected clearly'

if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$(printf '0%.0s' {1..64})"
) >"$tmp_dir/bad-profile-hash.out" 2>"$tmp_dir/bad-profile-hash.err"; then
  fail 'runner accepted a mismatched profile hash'
fi
[ ! -e "$marker" ] || fail 'analyzer ran before profile integrity was verified'
grep -Fq 'profile SHA256 does not match --expect-profile-sha256' "$tmp_dir/bad-profile-hash.err" \
  || fail 'profile hash mismatch was not actionable'

repo_analyzer="$fixture/repository-analyzer.py"
cp "$analyzer" "$repo_analyzer"
chmod +x "$repo_analyzer"
repo_analyzer_hash="$(sha256_file "$repo_analyzer")"
repo_profile="$tmp_dir/repository-profile.json"
write_profile "$repo_profile" "$repo_analyzer" "$repo_analyzer_hash" \
  fixture-controlled 2.0.0 sarif 10 1000000
repo_profile_hash="$(sha256_file "$repo_profile")"
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$repo_profile" --expect-profile-sha256 "$repo_profile_hash"
) >"$tmp_dir/repository-executable.out" 2>"$tmp_dir/repository-executable.err"; then
  fail 'runner executed a repository-owned analyzer'
fi
grep -Fq 'executable must be outside the reviewed repository' "$tmp_dir/repository-executable.err" \
  || fail 'repository executable rejection was not actionable'
rm -f "$repo_analyzer"

mutation_backup="$tmp_dir/app.py.before-mutation"
cp "$fixture/src/app.py" "$mutation_backup"
mutating_analyzer="$tmp_dir/mutating-analyzer.py"
python3 - "$mutating_analyzer" "$fixture/src/app.py" <<'PY'
import pathlib
import sys

output = pathlib.Path(sys.argv[1])
target = sys.argv[2]
program = f'''#!/usr/bin/env python3
import json
import pathlib

target = pathlib.Path({target!r})
target.write_text(target.read_text(encoding="utf-8") + "# analyzer mutation\\n", encoding="utf-8")
print(json.dumps({{
    "version": "2.1.0",
    "runs": [{{
        "tool": {{"driver": {{"name": "fixture-mutator", "version": "1.0.0"}}}},
        "results": []
    }}]
}}))
'''
output.write_text(program, encoding='utf-8')
PY
chmod +x "$mutating_analyzer"
mutating_profile="$tmp_dir/mutating-profile.json"
write_profile "$mutating_profile" "$mutating_analyzer" "$(sha256_file "$mutating_analyzer")" \
  fixture-mutator 1.0.0 sarif 10 1000000
mutating_profile_hash="$(sha256_file "$mutating_profile")"
mutation_accepted='no'
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$mutating_profile" --expect-profile-sha256 "$mutating_profile_hash"
) >"$tmp_dir/mutation.out" 2>"$tmp_dir/mutation.err"; then
  mutation_accepted='yes'
fi
cp "$mutation_backup" "$fixture/src/app.py"
[ "$mutation_accepted" = 'no' ] \
  || fail 'runner accepted evidence after the analyzer changed tracked working-tree bytes'
grep -Fq 'reviewed repository state changed during controlled execution' "$tmp_dir/mutation.err" \
  || fail 'tracked working-tree mutation did not fail with an actionable error'

failed_analyzer="$tmp_dir/failed-analyzer.sh"
cat >"$failed_analyzer" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'analyzer-private-stderr-fixture' >&2
exit 7
EOF
chmod +x "$failed_analyzer"
failed_profile="$tmp_dir/failed-profile.json"
write_profile "$failed_profile" "$failed_analyzer" "$(sha256_file "$failed_analyzer")" \
  failed-fixture 1.0.0 sarif 10 1000000
failed_profile_hash="$(sha256_file "$failed_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$failed_profile" --expect-profile-sha256 "$failed_profile_hash"
) >"$tmp_dir/failed-execution.out" 2>"$tmp_dir/failed-execution.err"
python3 "$validator" --static-execution-output "$tmp_dir/failed-execution.out" >/dev/null \
  || fail 'non-success execution output did not validate'
jq -e '
  .execution.status == "failed"
  and .execution.exit_code == 7
  and .execution.failure_reason == "non-success-exit"
  and .execution.result_accepted == false
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/failed-execution.out") >/dev/null \
  || fail 'non-success exit did not become failed execution evidence'
jq -e '.reports[0].status == "failed" and .counts.blocking_candidates == 0' \
  < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/failed-execution.out") >/dev/null \
  || fail 'failed execution evidence was allowed to block'
if grep -Fq 'analyzer-private-stderr-fixture' \
  "$tmp_dir/failed-execution.out" "$tmp_dir/failed-execution.err"; then
  fail 'raw analyzer stderr escaped the controlled execution runtime'
fi

timeout_analyzer="$tmp_dir/timeout-analyzer.sh"
cat >"$timeout_analyzer" <<'EOF'
#!/usr/bin/env bash
sleep 5
printf '%s\n' '{}'
EOF
chmod +x "$timeout_analyzer"
timeout_profile="$tmp_dir/timeout-profile.json"
write_profile "$timeout_profile" "$timeout_analyzer" "$(sha256_file "$timeout_analyzer")" \
  timeout-fixture 1.0.0 normalized-json 1 1000000
timeout_profile_hash="$(sha256_file "$timeout_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$timeout_profile" --expect-profile-sha256 "$timeout_profile_hash"
) >"$tmp_dir/timeout.out" 2>"$tmp_dir/timeout.err"
python3 "$validator" --static-execution-output "$tmp_dir/timeout.out" >/dev/null \
  || fail 'timeout execution output did not validate'
jq -e '
  .execution.status == "timeout"
  and .execution.result_accepted == false
  and .execution.failure_reason == "timeout"
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/timeout.out") >/dev/null \
  || fail 'timeout did not become bounded non-blocking execution evidence'
jq -e '
  .reports[0].status == "timeout"
  and .counts.blocking_candidates == 0
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/timeout.out") >/dev/null \
  || fail 'timeout evidence was allowed to block'

invalid_analyzer="$tmp_dir/invalid-analyzer.sh"
cat >"$invalid_analyzer" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'not-json'
EOF
chmod +x "$invalid_analyzer"
invalid_profile="$tmp_dir/invalid-profile.json"
write_profile "$invalid_profile" "$invalid_analyzer" "$(sha256_file "$invalid_analyzer")" \
  invalid-fixture 1.0.0 sarif 10 1000000
invalid_profile_hash="$(sha256_file "$invalid_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$invalid_profile" --expect-profile-sha256 "$invalid_profile_hash"
) >"$tmp_dir/invalid.out" 2>"$tmp_dir/invalid.err"
python3 "$validator" --static-execution-output "$tmp_dir/invalid.out" >/dev/null \
  || fail 'invalid-output execution did not validate'
jq -e '
  .execution.status == "invalid-output"
  and .execution.failure_reason == "invalid-output"
  and .execution.result_accepted == false
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/invalid.out") >/dev/null \
  || fail 'invalid analyzer output did not fail closed'

failed_analyzer="$tmp_dir/failed-analyzer.sh"
cat >"$failed_analyzer" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'raw-stderr-secret-must-not-be-emitted' >&2
exit 7
EOF
chmod +x "$failed_analyzer"
failed_profile="$tmp_dir/failed-profile.json"
write_profile "$failed_profile" "$failed_analyzer" "$(sha256_file "$failed_analyzer")" \
  failed-fixture 1.0.0 normalized-json 10 1000000
failed_profile_hash="$(sha256_file "$failed_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$failed_profile" --expect-profile-sha256 "$failed_profile_hash"
) >"$tmp_dir/failed-execution.out" 2>"$tmp_dir/failed-execution.err"
python3 "$validator" --static-execution-output "$tmp_dir/failed-execution.out" >/dev/null \
  || fail 'failed execution output did not validate'
if grep -Fq 'raw-stderr-secret-must-not-be-emitted' \
  "$tmp_dir/failed-execution.out" "$tmp_dir/failed-execution.err"; then
  fail 'raw analyzer stderr escaped controlled execution'
fi
jq -e '
  .execution.status == "failed"
  and .execution.exit_code == 7
  and .execution.stderr_bytes > 0
  and .execution.result_accepted == false
  and .execution.failure_reason == "non-success-exit"
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/failed-execution.out") >/dev/null \
  || fail 'non-success analyzer exit was not recorded as unavailable verification'

limit_analyzer="$tmp_dir/limit-analyzer.py"
cat >"$limit_analyzer" <<'PY'
#!/usr/bin/env python3
print('x' * 5000)
PY
chmod +x "$limit_analyzer"
limit_profile="$tmp_dir/limit-profile.json"
write_profile "$limit_profile" "$limit_analyzer" "$(sha256_file "$limit_analyzer")" \
  limit-fixture 1.0.0 sarif 10 1024
limit_profile_hash="$(sha256_file "$limit_profile")"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$limit_profile" --expect-profile-sha256 "$limit_profile_hash"
) >"$tmp_dir/limit.out" 2>"$tmp_dir/limit.err"
python3 "$validator" --static-execution-output "$tmp_dir/limit.out" >/dev/null \
  || fail 'output-limit execution did not validate'
jq -e '
  .execution.status == "output-limit"
  and .execution.failure_reason == "output-limit"
  and .execution.stdout_bytes == 1025
' < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/limit.out") >/dev/null \
  || fail 'analyzer output was not capped at one byte beyond the configured limit'

write_snapshot_analyzer() {
  local output="$1"
  local expected="$2"
  python3 - "$output" "$expected" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
expected = sys.argv[2]
path.write_text(f'''#!/usr/bin/env python3
import json
import pathlib
import sys

if pathlib.Path("state.txt").read_text(encoding="utf-8").strip() != {expected!r}:
    print("unexpected snapshot content", file=sys.stderr)
    raise SystemExit(12)
print(json.dumps({{
    "version": "2.1.0",
    "runs": [{{
        "tool": {{"driver": {{"name": "snapshot-fixture", "version": "1.0.0"}}}},
        "results": []
    }}]
}}))
''', encoding='utf-8')
PY
  chmod +x "$output"
}

unstaged_repo="$tmp_dir/unstaged-repo"
mkdir -p "$unstaged_repo"
git -C "$unstaged_repo" init -q
git -C "$unstaged_repo" config user.email a@example.com
git -C "$unstaged_repo" config user.name A
printf '%s\n' 'base' >"$unstaged_repo/state.txt"
git -C "$unstaged_repo" add state.txt
git -C "$unstaged_repo" commit -q -m baseline
printf '%s\n' 'unstaged-candidate' >"$unstaged_repo/state.txt"
unstaged_control="$tmp_dir/unstaged-control.out"
(
  cd "$unstaged_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source unstaged --control-plane
) >"$unstaged_control" 2>/dev/null
unstaged_fingerprint="$(python3 - "$unstaged_control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
unstaged_analyzer="$tmp_dir/unstaged-analyzer.py"
write_snapshot_analyzer "$unstaged_analyzer" unstaged-candidate
unstaged_profile="$tmp_dir/unstaged-profile.json"
write_profile "$unstaged_profile" "$unstaged_analyzer" "$(sha256_file "$unstaged_analyzer")" \
  snapshot-fixture 1.0.0 sarif 10 1000000
(
  cd "$unstaged_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source unstaged --expect-scope "$unstaged_fingerprint" \
      --profile "$unstaged_profile" \
      --expect-profile-sha256 "$(sha256_file "$unstaged_profile")"
) >"$tmp_dir/unstaged.out" 2>"$tmp_dir/unstaged.err"
python3 "$validator" --static-execution-output "$tmp_dir/unstaged.out" >/dev/null \
  || fail 'unstaged controlled snapshot did not validate'
jq -e '.scope.source == "unstaged" and .execution.status == "completed"' \
  < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/unstaged.out") >/dev/null \
  || fail 'unstaged controlled snapshot used the wrong source content'

branch_repo="$tmp_dir/branch-repo"
mkdir -p "$branch_repo"
git -C "$branch_repo" init -q
git -C "$branch_repo" config user.email a@example.com
git -C "$branch_repo" config user.name A
printf '%s\n' 'base' >"$branch_repo/state.txt"
git -C "$branch_repo" add state.txt
git -C "$branch_repo" commit -q -m baseline
git -C "$branch_repo" switch -q -c feature
printf '%s\n' 'branch-candidate' >"$branch_repo/state.txt"
git -C "$branch_repo" add state.txt
git -C "$branch_repo" commit -q -m feature
printf '%s\n' 'working-tree-noise' >"$branch_repo/state.txt"
branch_control="$tmp_dir/branch-control.out"
(
  cd "$branch_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source branch --control-plane
) >"$branch_control" 2>/dev/null
branch_fingerprint="$(python3 - "$branch_control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
branch_analyzer="$tmp_dir/branch-analyzer.py"
write_snapshot_analyzer "$branch_analyzer" branch-candidate
branch_profile="$tmp_dir/branch-profile.json"
write_profile "$branch_profile" "$branch_analyzer" "$(sha256_file "$branch_analyzer")" \
  snapshot-fixture 1.0.0 sarif 10 1000000
(
  cd "$branch_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source branch --expect-scope "$branch_fingerprint" \
      --profile "$branch_profile" \
      --expect-profile-sha256 "$(sha256_file "$branch_profile")"
) >"$tmp_dir/branch.out" 2>"$tmp_dir/branch.err"
python3 "$validator" --static-execution-output "$tmp_dir/branch.out" >/dev/null \
  || fail 'branch controlled snapshot did not validate'
jq -e '.scope.source == "branch" and .execution.status == "completed"' \
  < <(awk '/^## Static Analysis Execution JSON$/ { getline; print; exit }' "$tmp_dir/branch.out") >/dev/null \
  || fail 'branch controlled snapshot used working-tree content instead of HEAD'

normalized_analyzer="$tmp_dir/normalized-analyzer.py"
cat >"$normalized_analyzer" <<'PY'
#!/usr/bin/env python3
import json
import os

print(json.dumps({
    'schema_version': 1,
    'kind': 'static_analysis_input',
    'scope_fingerprint': os.environ['PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT'],
    'tool': {'name': 'normalized-fixture', 'version': '1.0.0'},
    'status': 'completed',
    'findings': [{
        'rule_id': 'NORMALIZED-EVAL',
        'message': 'Dynamic evaluation accepts untrusted input.',
        'path': 'src/app.py',
        'start_line': 2,
        'end_line': 2,
        'severity': 'critical',
        'category': 'security',
        'confidence': 'high',
        'baseline_state': 'unknown',
    }],
}))
PY
chmod +x "$normalized_analyzer"
normalized_profile="$tmp_dir/normalized-profile.json"
write_profile "$normalized_profile" "$normalized_analyzer" "$(sha256_file "$normalized_analyzer")" \
  normalized-fixture 1.0.0 normalized-json 10 1000000
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$normalized_profile" \
      --expect-profile-sha256 "$(sha256_file "$normalized_profile")"
) >"$tmp_dir/normalized-execution.out" 2>"$tmp_dir/normalized-execution.err"
python3 "$validator" --static-execution-output "$tmp_dir/normalized-execution.out" >/dev/null \
  || fail 'completed normalized JSON controlled execution did not validate'
jq -e '
  .reports[0].format == "normalized-json"
  and .reports[0].trust == "controlled-execution"
  and .counts.blocking_candidates == 1
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/normalized-execution.out") >/dev/null \
  || fail 'normalized JSON controlled result did not enter Phase 1 reduction'

tampered_analyzer="$tmp_dir/tampered-analyzer.py"
cp "$analyzer" "$tampered_analyzer"
chmod +x "$tampered_analyzer"
tampered_profile="$tmp_dir/tampered-profile.json"
write_profile "$tampered_profile" "$tampered_analyzer" "$(sha256_file "$tampered_analyzer")" \
  fixture-controlled 2.0.0 sarif 10 1000000
printf '%s\n' '# changed after authorization' >>"$tampered_analyzer"
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$fingerprint" \
      --profile "$tampered_profile" \
      --expect-profile-sha256 "$(sha256_file "$tampered_profile")"
) >"$tmp_dir/tampered.out" 2>"$tmp_dir/tampered.err"; then
  fail 'runner accepted an analyzer whose bytes no longer matched the profile'
fi
grep -Fq 'executable SHA256 does not match the profile' "$tmp_dir/tampered.err" \
  || fail 'analyzer integrity mismatch was not actionable'

symlink_repo="$tmp_dir/symlink-repo"
mkdir -p "$symlink_repo/src"
git -C "$symlink_repo" init -q
git -C "$symlink_repo" config user.email a@example.com
git -C "$symlink_repo" config user.name A
printf '%s\n' 'base' >"$symlink_repo/src/app.py"
git -C "$symlink_repo" add src/app.py
git -C "$symlink_repo" commit -q -m baseline
ln -s /etc/passwd "$symlink_repo/escape-link"
git -C "$symlink_repo" add escape-link
symlink_control="$tmp_dir/symlink-control.out"
(
  cd "$symlink_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source staged --control-plane
) >"$symlink_control" 2>/dev/null
symlink_fingerprint="$(python3 - "$symlink_control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
if (
  cd "$symlink_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$runner" --source staged --expect-scope "$symlink_fingerprint" \
      --profile "$profile" --expect-profile-sha256 "$profile_hash"
) >"$tmp_dir/symlink.out" 2>"$tmp_dir/symlink.err"; then
  fail 'runner accepted a tracked symlink that escapes the snapshot'
fi
grep -Fq 'analysis snapshot contains an absolute symlink' "$tmp_dir/symlink.err" \
  || fail 'unsafe symlink rejection was not actionable'

printf 'static analysis execution tests passed\n'
