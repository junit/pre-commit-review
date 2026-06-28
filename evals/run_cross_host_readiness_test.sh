#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
aggregator="$repo_root/evals/run_cross_host_readiness.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'cross-host readiness test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

cat >"$tmp_dir/mock-pipeline" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

host=''
report_json=''

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      host="$1"
      ;;
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done

printf '%s\n' "$host" >>"${MOCK_CROSS_HOST_LOG_FILE:?}"
[ -n "$host" ] || exit 19
[ -n "$report_json" ] || exit 23

case "$host" in
  claude)
    cat >"$report_json" <<'JSON'
{"schema_version":"host-readiness-report/v1","host":"claude","overall_status":"passed","failed_stage":null,"failure_taxonomy":null}
JSON
    exit 0
    ;;
  codex)
    cat >"$report_json" <<'JSON'
{"schema_version":"host-readiness-report/v1","host":"codex","overall_status":"failed","failed_stage":"layered_host_smoke","failure_taxonomy":"runner-exit-nonzero"}
JSON
    exit 1
    ;;
  *)
    exit 29
    ;;
esac
EOF
chmod +x "$tmp_dir/mock-pipeline"

matrix_report="$tmp_dir/complete-matrix.json"
matrix_dir="$tmp_dir/matrix-reports"
if MOCK_CROSS_HOST_LOG_FILE="$tmp_dir/matrix.log" \
  bash "$aggregator" \
    --pipeline "$tmp_dir/mock-pipeline" \
    --report-dir "$matrix_dir" \
    --report-json "$matrix_report" >"$tmp_dir/matrix.out" 2>"$tmp_dir/matrix.err"; then
  fail 'complete-matrix must exit non-zero when any host fails'
fi

grep -Fxq 'claude' "$tmp_dir/matrix.log" \
  || fail 'complete-matrix must run claude'
grep -Fxq 'codex' "$tmp_dir/matrix.log" \
  || fail 'complete-matrix must run codex'
[ "$(wc -l <"$tmp_dir/matrix.log" | tr -d ' ')" = "2" ] \
  || fail 'complete-matrix must execute both hosts'

require_json_field "$matrix_report" '.schema_version == "cross-host-readiness-report/v1"' 'matrix schema changed'
require_json_field "$matrix_report" '.mode == "complete-matrix"' 'matrix mode changed'
require_json_field "$matrix_report" '.overall_status == "failed"' 'matrix overall_status changed'
require_json_field "$matrix_report" '.failed_hosts == ["codex"]' 'matrix failed_hosts changed'
require_json_field "$matrix_report" '.skipped_hosts == []' 'matrix skipped_hosts changed'
require_json_field "$matrix_report" '.hosts.claude.execution_status == "completed"' 'claude execution_status changed'
require_json_field "$matrix_report" '.hosts.codex.execution_status == "completed"' 'codex execution_status changed'
require_json_field "$matrix_report" '.hosts.claude.report.overall_status == "passed"' 'claude embedded report changed'
require_json_field "$matrix_report" '.hosts.codex.report.failure_taxonomy == "runner-exit-nonzero"' 'codex embedded report changed'
require_json_field "$matrix_report" 'has("started_at") and has("finished_at") and has("duration_ms")' 'matrix timing fields missing'

fail_fast_report="$tmp_dir/fail-fast.json"
fail_fast_dir="$tmp_dir/fail-fast-reports"
if MOCK_CROSS_HOST_LOG_FILE="$tmp_dir/fail-fast.log" \
  bash "$aggregator" \
    --pipeline "$tmp_dir/mock-pipeline" \
    --fail-fast \
    --report-dir "$fail_fast_dir" \
    --report-json "$fail_fast_report" >"$tmp_dir/fail-fast.out" 2>"$tmp_dir/fail-fast.err"; then
  fail 'fail-fast must exit non-zero when the first failing host fails'
fi

grep -Fxq 'claude' "$tmp_dir/fail-fast.log" \
  || fail 'fail-fast must run claude first'
grep -Fxq 'codex' "$tmp_dir/fail-fast.log" \
  || fail 'fail-fast must run codex second when claude passes'
[ "$(wc -l <"$tmp_dir/fail-fast.log" | tr -d ' ')" = "2" ] \
  || fail 'fail-fast must stop only after the first failing host'

require_json_field "$fail_fast_report" '.mode == "fail-fast"' 'fail-fast mode changed'
require_json_field "$fail_fast_report" '.overall_status == "failed"' 'fail-fast overall_status changed'
require_json_field "$fail_fast_report" '.failed_hosts == ["codex"]' 'fail-fast failed_hosts changed'
require_json_field "$fail_fast_report" '.skipped_hosts == []' 'fail-fast skipped_hosts changed'
require_json_field "$fail_fast_report" '.schema_version == "cross-host-readiness-report/v1"' 'fail-fast schema changed'

cat >"$tmp_dir/mock-pipeline-claude-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

host=''
report_json=''

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      host="$1"
      ;;
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done

printf '%s\n' "$host" >>"${MOCK_CROSS_HOST_LOG_FILE:?}"
[ -n "$host" ] || exit 31
[ -n "$report_json" ] || exit 37

if [ "$host" = 'claude' ]; then
  cat >"$report_json" <<'JSON'
{"schema_version":"host-readiness-report/v1","host":"claude","overall_status":"failed","failed_stage":"availability","failure_taxonomy":"missing-binary"}
JSON
  exit 1
fi

cat >"$report_json" <<'JSON'
{"schema_version":"host-readiness-report/v1","host":"codex","overall_status":"passed","failed_stage":null,"failure_taxonomy":null}
JSON
exit 0
EOF
chmod +x "$tmp_dir/mock-pipeline-claude-fail"

skip_report="$tmp_dir/fail-fast-skip.json"
skip_dir="$tmp_dir/fail-fast-skip-reports"
if MOCK_CROSS_HOST_LOG_FILE="$tmp_dir/fail-fast-skip.log" \
  bash "$aggregator" \
    --pipeline "$tmp_dir/mock-pipeline-claude-fail" \
    --fail-fast \
    --report-dir "$skip_dir" \
    --report-json "$skip_report" >"$tmp_dir/fail-fast-skip.out" 2>"$tmp_dir/fail-fast-skip.err"; then
  fail 'fail-fast must exit non-zero when the first host fails'
fi

[ "$(wc -l <"$tmp_dir/fail-fast-skip.log" | tr -d ' ')" = "1" ] \
  || fail 'fail-fast must skip later hosts after the first failure'
require_json_field "$skip_report" '.schema_version == "cross-host-readiness-report/v1"' 'skip report schema changed'
require_json_field "$skip_report" '.mode == "fail-fast"' 'skip report mode changed'
require_json_field "$skip_report" '.failed_hosts == ["claude"]' 'skip report failed_hosts changed'
require_json_field "$skip_report" '.skipped_hosts == ["codex"]' 'skip report skipped_hosts changed'
require_json_field "$skip_report" '.hosts.claude.execution_status == "completed"' 'claude must be completed in skip report'
require_json_field "$skip_report" '.hosts.codex.execution_status == "skipped"' 'codex must be skipped in skip report'
require_json_field "$skip_report" '.hosts.codex.report == null' 'skipped host report must be null'

printf 'cross-host readiness tests passed\n'
