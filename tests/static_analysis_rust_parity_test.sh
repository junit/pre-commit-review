#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
python_collector="$repo_root/scripts/collect_static_evidence.py"
python_runner="$repo_root/scripts/run_static_analysis.py"
rust_binary="${PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN:-$repo_root/collect-diff-context-cli/target/release/static-analysis-cli}"
helper="$repo_root/scripts/collect_diff_context.sh"
normalizer="$repo_root/tests/lib/normalize_parity_output.py"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'static analysis Rust parity test failed: %s\n' "$*" >&2
  exit 1
}

[ -x "$rust_binary" ] || fail "Rust static-analysis binary is unavailable: $rust_binary"
if printf '%s\n' '## Static Analysis Execution JSON' '{"runtime_path":"/tmp/leak"}' \
  | python3 "$normalizer" >/dev/null 2>&1; then
  fail 'parity normalizer accepted a serialized runtime-only field'
fi

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
  local output="$tmp_dir/control-${source}.out"
  (
    cd "$repository"
    PRE_COMMIT_REVIEW_SECRET_SCAN=off "$helper" --source "$source" --control-plane
  ) >"$output" 2>/dev/null
  python3 - "$output" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
print(json.loads(lines[lines.index("## Review Control Plane JSON") + 1])["scope_fingerprint"])
PY
}

capture() {
  local prefix="$1"
  local repository="$2"
  shift 2
  local status
  set +e
  (
    cd "$repository"
    PRE_COMMIT_REVIEW_SECRET_SCAN=off "$@"
  ) >"${prefix}.out" 2>"${prefix}.err"
  status=$?
  set -e
  printf '%s\n' "$status" >"${prefix}.status"
}

compare_files() {
  local scenario="$1"
  local label="$2"
  local left="$3"
  local right="$4"
  if ! diff -u "$left" "$right" >"$tmp_dir/${scenario}-${label}.diff"; then
    sed -n '1,240p' "$tmp_dir/${scenario}-${label}.diff" >&2
    fail "$scenario $label differs"
  fi
}

compare_artifact() {
  local scenario="$1"
  local python_prefix="$tmp_dir/${scenario}-python"
  local rust_prefix="$tmp_dir/${scenario}-rust"
  compare_files "$scenario" status "$python_prefix.status" "$rust_prefix.status"
  [ "$(cat "$python_prefix.status")" = "0" ] || fail "$scenario did not succeed"
  python3 "$normalizer" <"$python_prefix.out" >"$python_prefix.normalized"
  python3 "$normalizer" <"$rust_prefix.out" >"$rust_prefix.normalized"
  compare_files "$scenario" stdout "$python_prefix.normalized" "$rust_prefix.normalized"
  compare_files "$scenario" stderr "$python_prefix.err" "$rust_prefix.err"
}

compare_collect() {
  local scenario="$1"
  local repository="$2"
  shift 2
  capture "$tmp_dir/${scenario}-python" "$repository" python3 "$python_collector" "$@"
  capture "$tmp_dir/${scenario}-rust" "$repository" "$rust_binary" collect "$@"
  compare_artifact "$scenario"
}

compare_collect_scope_error() {
  local scenario="$1"
  local repository="$2"
  shift 2
  local python_prefix="$tmp_dir/${scenario}-python"
  local rust_prefix="$tmp_dir/${scenario}-rust"
  capture "$python_prefix" "$repository" python3 "$python_collector" "$@"
  capture "$rust_prefix" "$repository" "$rust_binary" collect "$@"
  compare_files "$scenario" status "$python_prefix.status" "$rust_prefix.status"
  [ "$(cat "$python_prefix.status")" = "2" ] || fail "$scenario did not return usage/error status 2"
  [ ! -s "$python_prefix.out" ] || fail "$scenario Python emitted an authoritative artifact"
  [ ! -s "$rust_prefix.out" ] || fail "$scenario Rust emitted an authoritative artifact"
  if ! grep -Fq 'scope' "$python_prefix.err"; then
    sed -n '1,40p' "$python_prefix.err" >&2
    fail "$scenario Python error did not identify scope drift"
  fi
  if ! grep -Fq 'scope' "$rust_prefix.err"; then
    sed -n '1,40p' "$rust_prefix.err" >&2
    fail "$scenario Rust error did not identify scope drift"
  fi
}

compare_run() {
  local scenario="$1"
  local repository="$2"
  shift 2
  capture "$tmp_dir/${scenario}-python" "$repository" python3 "$python_runner" "$@"
  capture "$tmp_dir/${scenario}-rust" "$repository" "$rust_binary" run "$@"
  compare_artifact "$scenario"
}

write_profile() {
  local output="$1"
  local executable="$2"
  local tool_name="$3"
  local tool_version="$4"
  local timeout_seconds="$5"
  local max_output_bytes="$6"
  shift 6
  python3 - "$output" "$executable" "$(sha256_file "$executable")" "$tool_name" \
    "$tool_version" "$timeout_seconds" "$max_output_bytes" "$@" <<'PY'
import json
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_text(json.dumps({
    "schema_version": 1,
    "kind": "static_analysis_profile",
    "name": f"{sys.argv[4]} parity profile",
    "tool": {"name": sys.argv[4], "version": sys.argv[5]},
    "executable": {"path": sys.argv[2], "sha256": sys.argv[3]},
    "arguments": sys.argv[8:],
    "output_format": "normalized-json",
    "success_exit_codes": [0],
    "limits": {
        "timeout_seconds": int(sys.argv[6]),
        "max_output_bytes": int(sys.argv[7]),
        "max_snapshot_bytes": 20_000_000,
        "max_snapshot_files": 1000,
    },
    "repository_configuration": "disabled",
    "network_access": "offline-required",
}, separators=(",", ":")), encoding="utf-8")
PY
}

fixture="$tmp_dir/repository"
mkdir -p "$fixture/src"
git -C "$fixture" init -q -b main
git -C "$fixture" config user.email review@example.test
git -C "$fixture" config user.name 'Review Test'
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    return value.strip()
EOF
git -C "$fixture" add src/app.py
git -C "$fixture" commit -qm main
git -C "$fixture" switch -qc feature
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    eval(value)  # branch
    return value.strip()
EOF
git -C "$fixture" add src/app.py
git -C "$fixture" commit -qm branch
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    eval(value)  # staged
    return value.strip()
EOF
git -C "$fixture" add src/app.py
cat >"$fixture/src/app.py" <<'EOF'
def execute(value):
    eval(value)  # unstaged
    return value.strip()
EOF

staged_fingerprint="$(control_fingerprint "$fixture" staged)"
unstaged_fingerprint="$(control_fingerprint "$fixture" unstaged)"
branch_fingerprint="$(control_fingerprint "$fixture" branch)"

normalized_result="$tmp_dir/normalized.json"
python3 - "$normalized_result" "$staged_fingerprint" <<'PY'
import json
import pathlib
import sys

finding = {
    "rule_id": "PY-EVAL",
    "message": "Dynamic evaluation accepts untrusted input.",
    "path": "src/app.py",
    "start_line": 2,
    "end_line": 2,
    "severity": "critical",
    "category": "security",
    "confidence": "high",
    "baseline_state": "unknown",
}
pathlib.Path(sys.argv[1]).write_text(json.dumps({
    "schema_version": 1,
    "kind": "static_analysis_input",
    "scope_fingerprint": sys.argv[2],
    "tool": {"name": "fixture-collect", "version": "1.0"},
    "status": "completed",
    "findings": [finding, finding, {
        **finding,
        "rule_id": "PY-NOTE",
        "message": "Unchanged-line note.",
        "start_line": 3,
        "end_line": 3,
        "severity": "warning",
        "category": "maintainability",
        "confidence": "medium",
    }],
}, separators=(",", ":")), encoding="utf-8")
PY

sarif_result="$tmp_dir/results.sarif"
python3 - "$sarif_result" "$staged_fingerprint" <<'PY'
import json
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_text(json.dumps({
    "version": "2.1.0",
    "runs": [{
        "properties": {"preCommitReviewScopeFingerprint": sys.argv[2]},
        "tool": {"driver": {
            "name": "fixture-sarif",
            "version": "2.0",
            "rules": [{
                "id": "python/dynamic-eval",
                "properties": {"tags": ["security", "cwe-95"], "precision": "high"},
            }],
        }},
        "results": [{
            "ruleId": "python/dynamic-eval",
            "level": "error",
            "message": {"text": "Dynamic evaluation accepts untrusted input."},
            "locations": [{"physicalLocation": {
                "artifactLocation": {"uri": "src/app.py"},
                "region": {"startLine": 2, "endLine": 2},
            }}],
        }],
    }],
}, separators=(",", ":")), encoding="utf-8")
PY

failed_result="$tmp_dir/failed.json"
python3 - "$normalized_result" "$failed_result" <<'PY'
import json
import pathlib
import sys

value = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
value["status"] = "failed"
value["findings"] = value["findings"][:1]
pathlib.Path(sys.argv[2]).write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8")
PY

compare_collect collect-normalized "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --result "$normalized_result"
compare_collect collect-sarif "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --result "$sarif_result"
compare_collect collect-truncated "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --max-findings 1 --result "$normalized_result" --result "$normalized_result"
compare_collect collect-failed "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --result "$failed_result"
compare_collect_scope_error collect-scope-error "$fixture" --source staged \
  --expect-scope 0000000000000000000000000000000000000000 --result "$normalized_result"

mode_analyzer="$tmp_dir/mode-analyzer.sh"
cat >"$mode_analyzer" <<'SH'
#!/bin/sh
expected="$1"
observed="$(sed -n '2p' src/app.py)"
case "$observed" in
  *"$expected"*) ;;
  *) printf 'expected %s candidate, observed %s\n' "$expected" "$observed" >&2; exit 9 ;;
esac
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"fixture-run","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
SH
chmod +x "$mode_analyzer"

for source in staged unstaged branch; do
  case "$source" in
    staged) fingerprint="$staged_fingerprint" ;;
    unstaged) fingerprint="$unstaged_fingerprint" ;;
    branch) fingerprint="$branch_fingerprint" ;;
  esac
  profile="$tmp_dir/profile-${source}.json"
  write_profile "$profile" "$mode_analyzer" fixture-run 1.0 10 1000000 "$source"
  compare_run "run-${source}" "$fixture" --source "$source" --expect-scope "$fingerprint" \
    --profile "$profile" --expect-profile-sha256 "$(sha256_file "$profile")"
done

failed_analyzer="$tmp_dir/failed-analyzer.sh"
cat >"$failed_analyzer" <<'SH'
#!/bin/sh
printf 'fixture failure' >&2
exit 7
SH
chmod +x "$failed_analyzer"
failed_profile="$tmp_dir/failed-profile.json"
write_profile "$failed_profile" "$failed_analyzer" fixture-failed 1.0 10 1000000
compare_run run-failed "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --profile "$failed_profile" --expect-profile-sha256 "$(sha256_file "$failed_profile")"

timeout_analyzer="$tmp_dir/timeout-analyzer.sh"
cat >"$timeout_analyzer" <<'SH'
#!/bin/sh
sleep 2
SH
chmod +x "$timeout_analyzer"
timeout_profile="$tmp_dir/timeout-profile.json"
write_profile "$timeout_profile" "$timeout_analyzer" fixture-timeout 1.0 1 1000000
compare_run run-timeout "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --profile "$timeout_profile" --expect-profile-sha256 "$(sha256_file "$timeout_profile")"

invalid_analyzer="$tmp_dir/invalid-analyzer.sh"
cat >"$invalid_analyzer" <<'SH'
#!/bin/sh
printf '{'
SH
chmod +x "$invalid_analyzer"
invalid_profile="$tmp_dir/invalid-profile.json"
write_profile "$invalid_profile" "$invalid_analyzer" fixture-invalid 1.0 10 1000000
compare_run run-invalid-output "$fixture" --source staged --expect-scope "$staged_fingerprint" \
  --profile "$invalid_profile" --expect-profile-sha256 "$(sha256_file "$invalid_profile")"

printf 'static analysis Rust parity tests passed\n'
