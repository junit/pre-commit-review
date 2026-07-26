#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/scripts/orchestrate_static_analysis.sh"
helper="$repo_root/scripts/collect_diff_context.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'static analysis orchestration test failed: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

static_analysis_bin="$repo_root/collect-diff-context-cli/target/release/static-analysis-cli"
context_bin="$repo_root/collect-diff-context-cli/target/release/collect-diff-context-cli"
if [ ! -x "$static_analysis_bin" ] || [ ! -x "$context_bin" ]; then
  cargo build --release --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" --bins
fi
export PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$static_analysis_bin"

fixture="$tmp_dir/repo"
mkdir -p "$fixture/src"
git -C "$fixture" init -q
git -C "$fixture" config user.email a@example.com
git -C "$fixture" config user.name A
cat >"$fixture/src/app.rs" <<'EOF'
pub fn execute(value: &str) -> &str {
    value
}
EOF
git -C "$fixture" add src/app.rs
git -C "$fixture" commit -q -m baseline
cat >"$fixture/src/app.rs" <<'EOF'
pub fn execute(value: &str) -> &str {
    unsafe { std::env::set_var("REVIEW_VALUE", value); }
    value
}
EOF
git -C "$fixture" add src/app.rs

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

first_marker="$tmp_dir/first.marker"
second_marker="$tmp_dir/second.marker"
first_analyzer="$tmp_dir/first-analyzer.sh"
second_analyzer="$tmp_dir/second-analyzer.sh"
cat >"$first_analyzer" <<EOF
#!/bin/sh
set -eu
printf ran >'$first_marker'
printf '%s\n' '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"'"\$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"'","tool":{"name":"orchestration-first","version":"1.0"},"status":"completed","findings":[{"rule_id":"SEC-ENV","message":"sk_live_orchestration_fixture_123456 reaches a process environment mutation.","path":"src/app.rs","start_line":2,"end_line":2,"severity":"error","category":"security","confidence":"high","baseline_state":"new"}]}'
EOF
cat >"$second_analyzer" <<EOF
#!/bin/sh
set -eu
printf ran >'$second_marker'
printf '%s\n' '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"'"\$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"'","tool":{"name":"orchestration-second","version":"1.0"},"status":"completed","findings":[]}'
EOF
chmod +x "$first_analyzer" "$second_analyzer"

first_profile="$tmp_dir/first-profile.json"
second_profile="$tmp_dir/second-profile.json"
python3 - "$first_profile" "$second_profile" \
  "$first_analyzer" "$(sha256_file "$first_analyzer")" \
  "$second_analyzer" "$(sha256_file "$second_analyzer")" <<'PY'
import json
import pathlib
import sys

def profile(name, executable, digest, repository_configuration):
    return {
        'schema_version': 1,
        'kind': 'static_analysis_profile',
        'name': f'{name} orchestration profile',
        'tool': {'name': name, 'version': '1.0'},
        'executable': {'path': executable, 'sha256': digest},
        'arguments': [],
        'output_format': 'normalized-json',
        'success_exit_codes': [0],
        'limits': {
            'timeout_seconds': 10,
            'max_output_bytes': 1_000_000,
            'max_snapshot_bytes': 20_000_000,
            'max_snapshot_files': 1000,
        },
        'repository_configuration': repository_configuration,
        'network_access': 'offline-required',
    }

pathlib.Path(sys.argv[1]).write_text(
    json.dumps(profile('orchestration-first', sys.argv[3], sys.argv[4], 'disabled')),
    encoding='utf-8',
)
pathlib.Path(sys.argv[2]).write_text(
    json.dumps(profile('orchestration-second', sys.argv[5], sys.argv[6], 'explicitly-trusted')),
    encoding='utf-8',
)
PY

manifest="$tmp_dir/manifest.json"
python3 - "$manifest" "$first_profile" "$(sha256_file "$first_profile")" \
  "$second_profile" "$(sha256_file "$second_profile")" <<'PY'
import json
import pathlib
import sys

payload = {
    'schema_version': 1,
    'kind': 'static_analysis_orchestration_manifest',
    'name': 'public wrapper fixture',
    'profiles': [
        {'profile_id': 'security', 'path': sys.argv[2], 'sha256': sys.argv[3]},
        {'profile_id': 'policy', 'path': sys.argv[4], 'sha256': sys.argv[5]},
    ],
    'limits': {
        'max_execution_seconds': 30,
        'max_captured_output_bytes': 5_000_000,
        'max_findings': 100,
        'max_snapshot_bytes': 20_000_000,
        'max_snapshot_files': 1000,
    },
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload), encoding='utf-8')
PY
manifest_hash="$(sha256_file "$manifest")"

if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$wrapper" --source staged --expect-scope "$fingerprint" \
      --manifest "$manifest" --expect-manifest-sha256 "$manifest_hash"
) >"$tmp_dir/missing-authorization.out" 2>"$tmp_dir/missing-authorization.err"; then
  fail 'wrapper inferred repository-configuration authorization from the manifest hash'
fi
[ ! -e "$first_marker" ] && [ ! -e "$second_marker" ] \
  || fail 'analyzer ran before the complete manifest authorization set passed'
grep -Fq 'orchestrate_static_analysis: profile requires separate --allow-repository-configuration authorization' \
  "$tmp_dir/missing-authorization.err" \
  || fail 'missing repository-configuration authorization was not actionable'

output="$tmp_dir/orchestration.out"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$wrapper" --source staged --expect-scope "$fingerprint" \
      --manifest "$manifest" --expect-manifest-sha256 "$manifest_hash" \
      --allow-repository-configuration
) >"$output" 2>"$tmp_dir/orchestration.err"
[ -e "$first_marker" ] && [ -e "$second_marker" ] \
  || fail 'authorized analyzers did not execute'
grep -Fq 'status: disabled' "$tmp_dir/orchestration.err" \
  || fail 'disabled sanitizer state was not reported'

python3 "$repo_root/scripts/validate_schemas.py" \
  --static-orchestration-manifest "$manifest" \
  --static-orchestration-output "$output" >/dev/null \
  || fail 'schema validator rejected valid orchestration manifest/output'

python3 - "$manifest" "$tmp_dir/relative-manifest.json" "$tmp_dir/duplicate-profile-manifest.json" <<'PY'
import copy
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
relative = copy.deepcopy(manifest)
relative['profiles'][0]['path'] = 'relative-profile.json'
pathlib.Path(sys.argv[2]).write_text(json.dumps(relative), encoding='utf-8')
duplicate = copy.deepcopy(manifest)
duplicate['profiles'][1]['profile_id'] = duplicate['profiles'][0]['profile_id']
pathlib.Path(sys.argv[3]).write_text(json.dumps(duplicate), encoding='utf-8')
PY
for invalid_manifest in \
  "$tmp_dir/relative-manifest.json" \
  "$tmp_dir/duplicate-profile-manifest.json"; do
  if python3 "$repo_root/scripts/validate_schemas.py" \
    --static-orchestration-manifest "$invalid_manifest" >/dev/null 2>&1; then
    fail "schema validator accepted invalid orchestration manifest: $(basename "$invalid_manifest")"
  fi
done

python3 - "$output" "$fingerprint" <<'PY' \
  || fail 'public orchestration output did not satisfy its linked contracts'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
orchestration = json.loads(lines[lines.index('## Static Analysis Orchestration JSON') + 1])
evidence = json.loads(lines[lines.index('## Static Analysis Evidence JSON') + 1])
assert orchestration['kind'] == 'static_analysis_orchestration'
assert orchestration['authoritative'] is True
assert orchestration['status'] == 'completed'
assert orchestration['scope']['fingerprint'] == sys.argv[2]
assert orchestration['scope'] == evidence['scope']
assert [run['profile_id'] for run in orchestration['runs']] == ['security', 'policy']
assert all(run['run_kind'] == 'executed' for run in orchestration['runs'])
assert len(evidence['reports']) == 2
assert len(set(orchestration['report_ids'])) == 2
assert orchestration['report_ids'] == [item['report_id'] for item in evidence['reports']]
assert orchestration['finding_ids'] == [item['finding_id'] for item in evidence['findings']]
assert evidence['counts']['blocking_candidates'] == 1
PY

python3 - "$output" "$tmp_dir" <<'PY'
import copy
import hashlib
import json
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
target = pathlib.Path(sys.argv[2])
lines = source.read_text(encoding='utf-8').splitlines()
orchestration_index = lines.index('## Static Analysis Orchestration JSON') + 1
evidence_index = lines.index('## Static Analysis Evidence JSON') + 1
orchestration = json.loads(lines[orchestration_index])
evidence = json.loads(lines[evidence_index])

def write_case(name, orchestration_payload, evidence_payload):
    rendered = list(lines)
    rendered[orchestration_index] = json.dumps(orchestration_payload, separators=(',', ':'))
    rendered[evidence_index] = json.dumps(evidence_payload, separators=(',', ':'))
    (target / f'{name}.out').write_text('\n'.join(rendered) + '\n', encoding='utf-8')

scope_mismatch = copy.deepcopy(evidence)
scope_mismatch['scope']['fingerprint'] = '0' * 64
write_case('scope-mismatch', orchestration, scope_mismatch)

report_mismatch = copy.deepcopy(orchestration)
report_mismatch['report_ids'] = report_mismatch['report_ids'][:-1]
write_case('report-mismatch', report_mismatch, evidence)

status_mismatch = copy.deepcopy(orchestration)
status_mismatch['status'] = 'partial'
write_case('status-mismatch', status_mismatch, evidence)

incomplete_orchestration = copy.deepcopy(orchestration)
incomplete_candidate = copy.deepcopy(evidence)
timeout_execution = incomplete_orchestration['runs'][1]['execution']
timeout_process = timeout_execution['execution']
timeout_process['status'] = 'timeout'
timeout_process['result_accepted'] = False
timeout_process['failure_reason'] = 'timeout'
timeout_process['exit_code'] = None
execution_digest = hashlib.sha256()
for value in (
    timeout_execution['scope']['fingerprint'],
    timeout_execution['profile']['sha256'],
    timeout_execution['executable']['sha256'],
    timeout_process['stdout_sha256'],
    timeout_process['status'],
):
    execution_digest.update(str(value).encode('utf-8'))
    execution_digest.update(b'\0')
timeout_execution['execution_id'] = execution_digest.hexdigest()[:16]
timeout_report_id = timeout_execution['evidence']['report_ids'][0]
timeout_report = next(
    report for report in incomplete_candidate['reports']
    if report['report_id'] == timeout_report_id
)
timeout_report['status'] = 'timeout'
timeout_report['execution_id'] = timeout_execution['execution_id']
timeout_report['finding_count'] = 1
timeout_finding = copy.deepcopy(incomplete_candidate['findings'][0])
timeout_finding['finding_id'] = 'f' * 16
timeout_finding['report_ids'] = [timeout_report_id]
timeout_finding['tool'] = timeout_execution['tool']
timeout_finding['rule_id'] = 'TIMEOUT-CANDIDATE'
timeout_finding['message'] = 'A timeout report must not support a blocking candidate.'
incomplete_candidate['findings'].append(timeout_finding)
for count_name in ('input_findings', 'deduplicated_findings', 'mapped_to_units', 'added_line', 'blocking_candidates'):
    incomplete_candidate['counts'][count_name] += 1
incomplete_orchestration['status'] = 'partial'
incomplete_orchestration['finding_ids'].append(timeout_finding['finding_id'])
findings_budget = incomplete_orchestration['budgets']['findings']
findings_budget['consumed'] += 1
findings_budget['remaining'] -= 1
orchestration_digest = hashlib.sha256()
for value in (
    incomplete_orchestration['scope']['fingerprint'],
    incomplete_orchestration['manifest']['sha256'],
    incomplete_orchestration['snapshot']['sha256'],
):
    orchestration_digest.update(value.encode('utf-8'))
    orchestration_digest.update(b'\0')
for run in incomplete_orchestration['runs']:
    for value in (run['profile_id'], 'executed', run['execution']['execution_id']):
        orchestration_digest.update(value.encode('utf-8'))
        orchestration_digest.update(b'\0')
incomplete_orchestration['orchestration_id'] = orchestration_digest.hexdigest()[:16]
write_case('incomplete-candidate', incomplete_orchestration, incomplete_candidate)

empty_evidence = copy.deepcopy(evidence)
empty_evidence['reports'] = []
empty_evidence['findings'] = []
empty_evidence['truncated'] = False
empty_evidence['counts'] = {
    'reports': 0,
    'input_findings': 0,
    'deduplicated_findings': 0,
    'mapped_to_units': 0,
    'added_line': 0,
    'blocking_candidates': 0,
    'priority_candidates': 0,
    'notes': 0,
    'outside_scope': 0,
}
empty_ids = copy.deepcopy(orchestration)
empty_ids['report_ids'] = []
empty_ids['finding_ids'] = []
write_case('executed-without-reports', empty_ids, empty_evidence)
PY

for invalid_output in \
  "$tmp_dir/scope-mismatch.out" \
  "$tmp_dir/report-mismatch.out" \
  "$tmp_dir/status-mismatch.out" \
  "$tmp_dir/incomplete-candidate.out" \
  "$tmp_dir/executed-without-reports.out"; do
  if python3 "$repo_root/scripts/validate_schemas.py" \
    --static-orchestration-output "$invalid_output" >/dev/null 2>&1; then
    fail "schema validator accepted semantically inconsistent output: $(basename "$invalid_output")"
  fi
done

if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$wrapper" --source staged --expect-scope "$fingerprint" \
      --manifest "$manifest" --expect-manifest-sha256 "$(printf '0%.0s' {1..64})" \
      --allow-repository-configuration
) >"$tmp_dir/bad-hash.out" 2>"$tmp_dir/bad-hash.err"; then
  fail 'wrapper accepted a mismatched manifest hash'
fi
grep -Fq 'orchestrate_static_analysis: manifest SHA256 does not match --expect-manifest-sha256' \
  "$tmp_dir/bad-hash.err" || fail 'manifest hash mismatch did not use the public error prefix'

mock_sanitizer="$tmp_dir/mock-sanitizer.sh"
cat >"$mock_sanitizer" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[ "$PRE_COMMIT_REVIEW_SANITIZE_STREAM" = 'controlled-static-analysis-orchestration-stdout' ]
sed 's/sk_live_orchestration_fixture_123456/[redacted:orchestration-fixture]/g'
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
    "$wrapper" --source staged --expect-scope "$fingerprint" \
      --manifest "$manifest" --expect-manifest-sha256 "$manifest_hash" \
      --allow-repository-configuration
) >"$tmp_dir/sanitized.out" 2>"$tmp_dir/sanitized.err"
grep -Fq '[redacted:orchestration-fixture]' "$tmp_dir/sanitized.out" \
  || fail 'orchestration wrapper did not release sanitized output'
if grep -Fq 'sk_live_orchestration_fixture_123456' "$tmp_dir/sanitized.out"; then
  fail 'orchestration wrapper leaked sanitizer-matched analyzer text'
fi
grep -Fq 'status: redacted' "$tmp_dir/sanitized.err" \
  || fail 'redacted sanitizer state was not reported'

isolated="$tmp_dir/isolated"
mkdir -p "$isolated/scripts/lib"
cp "$wrapper" "$isolated/scripts/orchestrate_static_analysis.sh"
cp "$repo_root/scripts/lib/static_analysis_cli.sh" "$isolated/scripts/lib/static_analysis_cli.sh"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$static_analysis_bin" \
    "$isolated/scripts/orchestrate_static_analysis.sh" \
      --source staged --expect-scope "$fingerprint" \
      --manifest "$manifest" --expect-manifest-sha256 "$manifest_hash" \
      --allow-repository-configuration
) >"$tmp_dir/unavailable.out" 2>"$tmp_dir/unavailable.err"
grep -Fq 'status: unavailable' "$tmp_dir/unavailable.err" \
  || fail 'unavailable sanitizer state was not reported'
grep -Fq 'reason: sanitizer-unavailable' "$tmp_dir/unavailable.err" \
  || fail 'unavailable sanitizer reason was not stable'

echo 'static analysis orchestration tests passed'
