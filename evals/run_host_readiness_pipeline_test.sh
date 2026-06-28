#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
pipeline="$repo_root/evals/run_host_readiness_pipeline.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'host readiness pipeline test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

cat >"$tmp_dir/mock-availability" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'availability|%s\n' "$*" >>"${PIPELINE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"availability","host":"claude","status":"passed","failure_taxonomy":null}
JSON
printf 'availability ok\n'
EOF
chmod +x "$tmp_dir/mock-availability"

cat >"$tmp_dir/mock-layered" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'layered|%s\n' "$*" >>"${PIPELINE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"layered_host_smoke","host":"claude","status":"passed","failure_taxonomy":null}
JSON
printf 'layered ok\n'
EOF
chmod +x "$tmp_dir/mock-layered"

cat >"$tmp_dir/mock-contract" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'contract|%s\n' "$*" >>"${PIPELINE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"contract_subset","host":null,"status":"passed","failure_taxonomy":null}
JSON
printf 'contract ok\n'
EOF
chmod +x "$tmp_dir/mock-contract"

success_report="$tmp_dir/success-report.json"
PIPELINE_LOG_FILE="$tmp_dir/pipeline.log" \
  bash "$pipeline" \
    --host claude \
    --claude-bin /tmp/mock-claude \
    --availability-gate "$tmp_dir/mock-availability" \
    --layered-host-runner "$tmp_dir/mock-layered" \
    --contract-subset "$tmp_dir/mock-contract" \
    --report-json "$success_report" \
    --print-report-json \
    --case mixed-staged-unstaged \
    --keep-fixtures >"$tmp_dir/success.out"

[ "$(wc -l <"$tmp_dir/pipeline.log" | tr -d ' ')" = "3" ] \
  || fail 'pipeline must invoke exactly three stages'
grep -Fq '=== Availability: claude ===' "$tmp_dir/success.out" \
  || fail 'availability banner changed'
grep -Fq '=== Layered Host Smoke: claude ===' "$tmp_dir/success.out" \
  || fail 'layered banner changed'
grep -Fq '=== Contract Subset: claude ===' "$tmp_dir/success.out" \
  || fail 'contract banner changed'
grep -Fq '"host":"claude"' "$tmp_dir/success.out" \
  || fail 'printed json summary missing host'

require_json_field "$success_report" '.schema_version == "host-readiness-report/v1"' 'success report schema changed'
require_json_field "$success_report" '.host == "claude"' 'success report host changed'
require_json_field "$success_report" '.overall_status == "passed"' 'success report must be passed'
require_json_field "$success_report" '.failed_stage == null' 'success report failed_stage must be null'
require_json_field "$success_report" '.failure_taxonomy == null' 'success report failure_taxonomy must be null'
require_json_field "$success_report" '.stages.availability.status == "passed"' 'availability success status changed'
require_json_field "$success_report" '.stages.layered_host_smoke.status == "passed"' 'layered success status changed'
require_json_field "$success_report" '.stages.contract_subset.status == "passed"' 'contract success status changed'
require_json_field "$success_report" 'has("started_at") and has("finished_at") and has("duration_ms")' 'success report timing fields missing'

cat >"$tmp_dir/mock-availability-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'availability-fail|%s\n' "$*" >>"${PIPELINE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"availability","host":"codex","status":"failed","failure_taxonomy":"missing-binary"}
JSON
printf 'host eval failure [missing-binary]: mock availability failed\n' >&2
exit 9
EOF
chmod +x "$tmp_dir/mock-availability-fail"

availability_report="$tmp_dir/availability-fail-report.json"
: >"$tmp_dir/pipeline.log"
if PIPELINE_LOG_FILE="$tmp_dir/pipeline.log" \
  bash "$pipeline" \
    --host codex \
    --codex-bin /tmp/mock-codex \
    --availability-gate "$tmp_dir/mock-availability-fail" \
    --layered-host-runner "$tmp_dir/mock-layered" \
    --contract-subset "$tmp_dir/mock-contract" \
    --report-json "$availability_report" >"$tmp_dir/fail-availability.out" 2>"$tmp_dir/fail-availability.err"; then
  fail 'pipeline must fail when availability stage fails'
fi

[ "$(wc -l <"$tmp_dir/pipeline.log" | tr -d ' ')" = "1" ] \
  || fail 'availability failure must short-circuit later stages'
require_json_field "$availability_report" '.overall_status == "failed"' 'availability failure report must be failed'
require_json_field "$availability_report" '.failed_stage == "availability"' 'availability failure stage changed'
require_json_field "$availability_report" '.failure_taxonomy == "missing-binary"' 'availability failure taxonomy changed'
require_json_field "$availability_report" '.stages.availability.status == "failed"' 'availability failure status changed'
require_json_field "$availability_report" '.stages.layered_host_smoke.status == "skipped"' 'layered must be skipped after availability failure'
require_json_field "$availability_report" '.stages.contract_subset.status == "skipped"' 'contract must be skipped after availability failure'

cat >"$tmp_dir/mock-layered-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'layered-fail|%s\n' "$*" >>"${PIPELINE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"layered_host_smoke","host":"codex","status":"failed","failure_taxonomy":"runner-exit-nonzero"}
JSON
printf 'host eval failure [runner-exit-nonzero]: mock layered failed\n' >&2
exit 7
EOF
chmod +x "$tmp_dir/mock-layered-fail"

layered_report="$tmp_dir/layered-fail-report.json"
: >"$tmp_dir/pipeline.log"
if PIPELINE_LOG_FILE="$tmp_dir/pipeline.log" \
  bash "$pipeline" \
    --host codex \
    --codex-bin /tmp/mock-codex \
    --availability-gate "$tmp_dir/mock-availability" \
    --layered-host-runner "$tmp_dir/mock-layered-fail" \
    --contract-subset "$tmp_dir/mock-contract" \
    --report-json "$layered_report" >"$tmp_dir/fail-layered.out" 2>"$tmp_dir/fail-layered.err"; then
  fail 'pipeline must fail when layered stage fails'
fi

[ "$(wc -l <"$tmp_dir/pipeline.log" | tr -d ' ')" = "2" ] \
  || fail 'layered failure must short-circuit contract stage'
require_json_field "$layered_report" '.overall_status == "failed"' 'layered failure report must be failed'
require_json_field "$layered_report" '.failed_stage == "layered_host_smoke"' 'layered failure stage changed'
require_json_field "$layered_report" '.failure_taxonomy == "runner-exit-nonzero"' 'layered failure taxonomy changed'
require_json_field "$layered_report" '.stages.availability.status == "passed"' 'availability must stay passed after layered failure'
require_json_field "$layered_report" '.stages.layered_host_smoke.status == "failed"' 'layered failure status changed'
require_json_field "$layered_report" '.stages.contract_subset.status == "skipped"' 'contract must be skipped after layered failure'

cat >"$tmp_dir/mock-availability-json-first-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
report_json=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
  esac
  shift
done
printf 'availability-json-first-fail|report=%s\n' "$report_json" >>"${PIPELINE_LOG_FILE:?}"
printf 'human message changed\n' >&2
cat >"$report_json" <<'JSON'
{"schema_version":"host-stage-report/v1","stage":"availability","host":"codex","status":"failed","failure_taxonomy":"missing-binary"}
JSON
exit 9
EOF
chmod +x "$tmp_dir/mock-availability-json-first-fail"

json_first_report="$tmp_dir/json-first-failure-report.json"
: >"$tmp_dir/pipeline.log"
if PIPELINE_LOG_FILE="$tmp_dir/pipeline.log" \
  bash "$pipeline" \
    --host codex \
    --codex-bin /tmp/mock-codex \
    --availability-gate "$tmp_dir/mock-availability-json-first-fail" \
    --layered-host-runner "$tmp_dir/mock-layered" \
    --contract-subset "$tmp_dir/mock-contract" \
    --report-json "$json_first_report" >"$tmp_dir/json-first.out" 2>"$tmp_dir/json-first.err"; then
  fail 'pipeline must fail when stage report marks availability as failed'
fi

require_json_field "$json_first_report" '.schema_version == "host-readiness-report/v1"' 'json-first failure schema changed'
require_json_field "$json_first_report" '.failed_stage == "availability"' 'json-first failure stage changed'
require_json_field "$json_first_report" '.failure_taxonomy == "missing-binary"' 'json-first failure taxonomy must come from stage report json'

printf 'host readiness pipeline tests passed\n'
