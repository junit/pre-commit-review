#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
collector="$repo_root/scripts/collect_static_evidence.sh"
helper="$repo_root/scripts/collect_diff_context.sh"
validator="$repo_root/scripts/validate_schemas.py"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'static analysis evidence test failed: %s\n' "$*" >&2
  exit 1
}

test_static_analysis_binary_resolution() {
  local layout="$tmp_dir/resolver-layout"
  local isolated_wrapper="$layout/scripts/collect_static_evidence.sh"
  local override_bin="$tmp_dir/override-static-analysis"
  local local_bin="$layout/collect-diff-context-cli/target/release/static-analysis-cli"
  local path_bin="$tmp_dir/path-only/static-analysis-cli"
  local os_name arch_name bundled_name bundled_bin

  mkdir -p "$layout/scripts/lib" "$layout/scripts/bin" \
    "$layout/collect-diff-context-cli/target/release" "$tmp_dir/path-only"
  cp "$collector" "$isolated_wrapper"
  cp "$repo_root/scripts/lib/static_analysis_cli.sh" "$layout/scripts/lib/static_analysis_cli.sh"

  cat >"$override_bin" <<'SH'
#!/bin/sh
printf 'override:%s\n' "$1"
SH
  chmod +x "$override_bin"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$override_bin" \
    "$isolated_wrapper" --expect-scope ignored >"$tmp_dir/resolver-override.out" 2>/dev/null
  grep -Fxq 'override:collect' "$tmp_dir/resolver-override.out" \
    || fail 'absolute static-analysis override was not selected'

  cat >"$local_bin" <<'SH'
#!/bin/sh
printf 'local:%s\n' "$1"
SH
  chmod +x "$local_bin"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$isolated_wrapper" --expect-scope ignored \
    >"$tmp_dir/resolver-local.out" 2>/dev/null
  grep -Fxq 'local:collect' "$tmp_dir/resolver-local.out" \
    || fail 'local release static-analysis binary was not selected'

  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch_name="$(uname -m)"
  case "$os_name" in
    darwin) os_name='darwin' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) os_name='linux' ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch_name='amd64' ;;
    arm64|aarch64) arch_name='arm64' ;;
    *) fail "unsupported resolver-test architecture: $arch_name" ;;
  esac
  bundled_name="static_analysis-${os_name}-${arch_name}"
  [ "$os_name" = 'windows' ] && bundled_name="${bundled_name}.exe"
  bundled_bin="$layout/scripts/bin/$bundled_name"
  cat >"$bundled_bin" <<'SH'
#!/bin/sh
printf 'bundled:%s\n' "$1"
SH
  chmod +x "$bundled_bin"
  rm -f "$local_bin"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off "$isolated_wrapper" --expect-scope ignored \
    >"$tmp_dir/resolver-bundled.out" 2>/dev/null
  grep -Fxq 'bundled:collect' "$tmp_dir/resolver-bundled.out" \
    || fail 'bundled platform static-analysis binary was not selected'

  if PRE_COMMIT_REVIEW_SECRET_SCAN=off PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN=relative-bin \
    "$isolated_wrapper" --expect-scope ignored >/dev/null 2>"$tmp_dir/resolver-relative.err"; then
    fail 'relative static-analysis override was accepted'
  fi
  non_executable="$tmp_dir/non-executable-static-analysis"
  : >"$non_executable"
  if PRE_COMMIT_REVIEW_SECRET_SCAN=off PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$non_executable" \
    "$isolated_wrapper" --expect-scope ignored >/dev/null 2>"$tmp_dir/resolver-nonexec.err"; then
    fail 'non-executable static-analysis override was accepted'
  fi

  rm -f "$bundled_bin"
  cat >"$path_bin" <<'SH'
#!/bin/sh
printf 'path search must not run\n' >"$PATH_SEARCH_MARKER"
SH
  chmod +x "$path_bin"
  if PATH="$tmp_dir/path-only:$PATH" PATH_SEARCH_MARKER="$tmp_dir/path-search-ran" \
    PRE_COMMIT_REVIEW_SECRET_SCAN=off "$isolated_wrapper" --expect-scope ignored \
    >/dev/null 2>"$tmp_dir/resolver-path.err"; then
    fail 'wrapper searched PATH for static-analysis-cli'
  fi
  [ ! -e "$tmp_dir/path-search-ran" ] || fail 'PATH-only static-analysis binary was executed'
}

test_static_analysis_binary_resolution

static_analysis_bin="$repo_root/collect-diff-context-cli/target/release/static-analysis-cli"
[ -x "$static_analysis_bin" ] || fail 'release static-analysis-cli is unavailable'
export PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN="$static_analysis_bin"

missing_dependency_error="$tmp_dir/missing-jsonschema.err"
if python3 -S "$validator" 2>"$missing_dependency_error"; then
  fail 'schema validator unexpectedly succeeded without jsonschema'
fi
grep -Fq "validate_schemas: Python package 'jsonschema' is required" \
  "$missing_dependency_error" \
  || fail 'schema validator did not explain its optional dependency'
if grep -Fq 'Traceback (most recent call last)' "$missing_dependency_error"; then
  fail 'schema validator exposed a traceback for a missing optional dependency'
fi

fixture="$tmp_dir/repo"
mkdir -p "$fixture/src"
git -C "$fixture" init -q
git -C "$fixture" config user.email a@example.com
git -C "$fixture" config user.name A
cat >"$fixture/src/app.ts" <<'EOF'
export function execute(input: string) {
  return input.trim();
}
EOF
git -C "$fixture" add src/app.ts
git -C "$fixture" commit -q -m baseline
cat >"$fixture/src/app.ts" <<'EOF'
export function execute(input: string) {
  eval(input);
  return input.trim();
}
EOF
git -C "$fixture" add src/app.ts

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
marker = lines.index('## Review Control Plane JSON')
print(json.loads(lines[marker + 1])['scope_fingerprint'])
PY
)"

normalized="$tmp_dir/normalized.json"
python3 - "$normalized" "$fingerprint" <<'PY'
import json
import pathlib
import sys

payload = {
    'schema_version': 1,
    'kind': 'static_analysis_input',
    'scope_fingerprint': sys.argv[2],
    'tool': {'name': 'fixture-analyzer', 'version': '1.2.3'},
    'status': 'completed',
    'findings': [
        {
            'rule_id': 'SEC-EVAL',
            'message': 'Dynamic evaluation accepts untrusted input.',
            'path': 'src/app.ts',
            'start_line': 2,
            'end_line': 2,
            'severity': 'critical',
            'category': 'security',
            'confidence': 'high',
            'baseline_state': 'unknown',
        },
        {
            'rule_id': 'SEC-EVAL',
            'message': 'Dynamic evaluation accepts untrusted input.',
            'path': 'src/app.ts',
            'start_line': 2,
            'end_line': 2,
            'severity': 'critical',
            'category': 'security',
            'confidence': 'high',
            'baseline_state': 'unknown',
        },
        {
            'rule_id': 'STYLE-RETURN',
            'message': 'Prefer an explicit local variable.',
            'path': 'src/app.ts',
            'start_line': 3,
            'end_line': 3,
            'severity': 'warning',
            'category': 'maintainability',
            'confidence': 'medium',
            'baseline_state': 'unknown',
        },
        {
            'rule_id': 'TYPE-OTHER',
            'message': 'A type error exists outside the selected change.',
            'path': 'src/other.ts',
            'start_line': 1,
            'end_line': 1,
            'severity': 'error',
            'category': 'build',
            'confidence': 'high',
            'baseline_state': 'unknown',
        },
    ],
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload), encoding='utf-8')
PY

normalized_output="$tmp_dir/normalized.out"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" --result "$normalized"
) >"$normalized_output" 2>"$tmp_dir/normalized.err"

python3 "$validator" --static-evidence-output "$normalized_output" >/dev/null \
  || fail 'normalized JSON evidence did not validate'

python3 - "$normalized_output" <<'PY' || fail 'normalized JSON evidence mapping was incorrect'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
payload = json.loads(lines[lines.index('## Static Analysis Evidence JSON') + 1])
assert payload['authoritative'] is True
assert payload['counts'] == {
    'reports': 1,
    'input_findings': 4,
    'deduplicated_findings': 3,
    'mapped_to_units': 2,
    'added_line': 1,
    'blocking_candidates': 1,
    'priority_candidates': 0,
    'notes': 1,
    'outside_scope': 1,
}
by_rule = {item['rule_id']: item for item in payload['findings']}
security = by_rule['SEC-EVAL']
assert security['manifest_unit_id'] == 'file:src/app.ts'
assert security['line_scope'] == 'added'
assert security['baseline_state'] == 'new'
assert security['disposition'] == 'blocking-candidate'
assert security['blocking_candidate'] is True
assert by_rule['STYLE-RETURN']['line_scope'] == 'unchanged'
assert by_rule['STYLE-RETURN']['disposition'] == 'note'
assert by_rule['TYPE-OTHER']['line_scope'] == 'outside-scope'
assert by_rule['TYPE-OTHER']['blocking_candidate'] is False
assert payload['decision_contract']['verification']
PY

(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" \
      --max-findings 1 --result "$normalized" --result "$normalized"
) >"$tmp_dir/truncated.out" 2>/dev/null
python3 "$validator" --static-evidence-output "$tmp_dir/truncated.out" >/dev/null \
  || fail 'truncated evidence did not validate'
jq -e '
  .truncated == true
  and .counts.reports == 1
  and .counts.deduplicated_findings == 3
  and (.findings | length) == 1
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/truncated.out") >/dev/null \
  || fail 'report deduplication or finding truncation was incorrect'

failed_report="$tmp_dir/failed.json"
python3 - "$normalized" "$failed_report" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload['status'] = 'failed'
payload['findings'] = payload['findings'][:1]
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" --result "$failed_report"
) >"$tmp_dir/failed.out" 2>/dev/null
jq -e '
  .reports[0].status == "failed"
  and .counts.blocking_candidates == 0
  and .counts.priority_candidates == 0
  and .findings[0].disposition == "note"
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/failed.out") >/dev/null \
  || fail 'failed analyzer output was allowed to block the review'

unbound_normalized="$tmp_dir/unbound-normalized.json"
python3 - "$normalized" "$unbound_normalized" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload.pop('scope_fingerprint')
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" \
      --result-scope "$fingerprint" --result "$unbound_normalized"
) >"$tmp_dir/unbound-normalized.out" 2>"$tmp_dir/unbound-normalized.err"; then
  fail 'collector accepted normalized JSON without an embedded scope fingerprint'
fi
grep -Fq 'normalized input must embed scope_fingerprint' "$tmp_dir/unbound-normalized.err" \
  || fail 'unbound normalized JSON did not fail with an actionable error'

sarif="$tmp_dir/results.sarif"
python3 - "$sarif" "$fingerprint" <<'PY'
import json
import pathlib
import sys

payload = {
    'version': '2.1.0',
    '$schema': 'https://json.schemastore.org/sarif-2.1.0.json',
    'runs': [{
        'properties': {'preCommitReviewScopeFingerprint': sys.argv[2]},
        'tool': {'driver': {
            'name': 'fixture-sarif',
            'version': '4.5.6',
            'rules': [{
                'id': 'js/dynamic-eval',
                'properties': {
                    'tags': ['security', 'external/cwe/cwe-95'],
                    'precision': 'high',
                },
            }],
        }},
        'results': [{
            'ruleId': 'js/dynamic-eval',
            'level': 'error',
            'baselineState': 'new',
            'message': {'text': 'Dynamic evaluation can execute attacker-controlled code.'},
            'locations': [{
                'physicalLocation': {
                    'artifactLocation': {'uri': 'src/app.ts'},
                    'region': {'startLine': 2, 'endLine': 2},
                },
            }],
        }],
    }],
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload), encoding='utf-8')
PY

sarif_output="$tmp_dir/sarif.out"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" --result "$sarif"
) >"$sarif_output" 2>/dev/null
python3 "$validator" --static-evidence-output "$sarif_output" >/dev/null \
  || fail 'SARIF evidence did not validate'
jq -e '
  .format == "sarif"
  and .scope_binding == "embedded"
  and .tool.name == "fixture-sarif"
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$sarif_output" | jq '.reports[0]') >/dev/null \
  || fail 'SARIF report metadata was not normalized'
jq -e '
  .category == "security"
  and .line_scope == "added"
  and .blocking_candidate == true
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$sarif_output" | jq '.findings[0]') >/dev/null \
  || fail 'SARIF finding was not mapped into a blocking candidate'

unbound_sarif="$tmp_dir/unbound.sarif"
python3 - "$sarif" "$unbound_sarif" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload['runs'][0].pop('properties')
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" \
      --result-scope "$fingerprint" --result "$unbound_sarif"
) >"$tmp_dir/asserted-sarif.out" 2>/dev/null
jq -e '.reports[0].scope_binding == "explicit-assertion"' \
  < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/asserted-sarif.out") >/dev/null \
  || fail 'explicit SARIF scope assertion was not recorded'

mismatched="$tmp_dir/mismatched.json"
python3 - "$normalized" "$mismatched" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload['scope_fingerprint'] = '0' * 40
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$collector" --source staged --expect-scope "$fingerprint" --result "$mismatched"
) >"$tmp_dir/mismatched.out" 2>"$tmp_dir/mismatched.err"; then
  fail 'collector accepted a static report bound to another scope'
fi
grep -Fq 'scope fingerprint does not match' "$tmp_dir/mismatched.err" \
  || fail 'scope mismatch did not fail with an actionable error'

secret_report="$tmp_dir/secret-message.json"
python3 - "$normalized" "$secret_report" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
payload['findings'] = payload['findings'][:1]
payload['findings'][0]['message'] = 'Analyzer accidentally echoed token sk_live_static_evidence_fixture_123456.'
pathlib.Path(sys.argv[2]).write_text(json.dumps(payload), encoding='utf-8')
PY
mock_sanitizer="$tmp_dir/mock-sanitizer.sh"
cat >"$mock_sanitizer" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
sed 's/sk_live_static_evidence_fixture_123456/[redacted:fixture-secret]/g'
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
    "$collector" --source staged --expect-scope "$fingerprint" --result "$secret_report"
) >"$tmp_dir/sanitized.out" 2>"$tmp_dir/sanitized.err"
if grep -Fq 'sk_live_static_evidence_fixture_123456' "$tmp_dir/sanitized.out"; then
  fail 'static evidence wrapper leaked a sanitizer-detected secret'
fi
grep -Fq '[redacted:fixture-secret]' "$tmp_dir/sanitized.out" \
  || fail 'static evidence wrapper did not release sanitized output'
grep -Fq 'status: redacted' "$tmp_dir/sanitized.err" \
  || fail 'static evidence wrapper did not report redaction status'
python3 "$validator" --static-evidence-output "$tmp_dir/sanitized.out" >/dev/null \
  || fail 'sanitized evidence no longer satisfied the JSON contract'

git -C "$fixture" diff --quiet \
  || fail 'collector modified the reviewed working tree'
git -C "$fixture" diff --cached --quiet && fail 'fixture unexpectedly lost its staged change'

printf 'static analysis evidence tests passed\n'
