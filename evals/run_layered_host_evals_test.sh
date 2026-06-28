#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/evals/run_layered_host_evals.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'run layered host evals test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

cat >"$tmp_dir/mock-claude-runner" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
all_args="$*"
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
printf 'claude|%s\n' "$all_args" >>"${MOCK_HOST_LAYERED_LOG_FILE:?}"
if [ -n "$report_json" ]; then
  cat >"$report_json" <<'JSON'
{"schema_version":"output-eval-report/v1"}
JSON
fi
EOF
chmod +x "$tmp_dir/mock-claude-runner"

cat >"$tmp_dir/mock-codex-runner" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
all_args="$*"
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
printf 'codex|%s\n' "$all_args" >>"${MOCK_HOST_LAYERED_LOG_FILE:?}"
if [ -n "$report_json" ]; then
  cat >"$report_json" <<'JSON'
{"schema_version":"output-eval-report/v1"}
JSON
fi
EOF
chmod +x "$tmp_dir/mock-codex-runner"

claude_report="$tmp_dir/claude-layered-report.json"
MOCK_HOST_LAYERED_LOG_FILE="$tmp_dir/host.log" \
  bash "$wrapper" \
    --host claude \
    --claude-runner "$tmp_dir/mock-claude-runner" \
    --codex-runner "$tmp_dir/mock-codex-runner" \
    --report-json "$claude_report" \
    --case mixed-staged-unstaged \
    --keep-fixtures >"$tmp_dir/claude.out"

[ "$(grep -c '^claude|' "$tmp_dir/host.log")" = "4" ] \
  || fail 'claude host must run four layered eval files'
grep -Fq 'routine-output-eval.json' "$tmp_dir/host.log" \
  || fail 'claude host must run routine-output-eval.json'
grep -Fq 'localization-output-eval.json' "$tmp_dir/host.log" \
  || fail 'claude host must run localization-output-eval.json'
grep -Fq -- '--case mixed-staged-unstaged --keep-fixtures' "$tmp_dir/host.log" \
  || fail 'claude host must preserve passthrough arguments'
require_json_field "$claude_report" '.schema_version == "host-stage-report/v1"' 'claude layered schema changed'
require_json_field "$claude_report" '.stage == "layered_host_smoke"' 'claude layered stage changed'
require_json_field "$claude_report" '.host == "claude"' 'claude layered host changed'
require_json_field "$claude_report" '.status == "passed"' 'claude layered status changed'
require_json_field "$claude_report" '.failure_taxonomy == null' 'claude layered failure_taxonomy must be null'

: >"$tmp_dir/host.log"
codex_report="$tmp_dir/codex-layered-report.json"
MOCK_HOST_LAYERED_LOG_FILE="$tmp_dir/host.log" \
  bash "$wrapper" \
    --host codex \
    --claude-runner "$tmp_dir/mock-claude-runner" \
    --codex-runner "$tmp_dir/mock-codex-runner" \
    --report-json "$codex_report" \
    --runner "echo mock-runner" >"$tmp_dir/codex.out"

[ "$(grep -c '^codex|' "$tmp_dir/host.log")" = "4" ] \
  || fail 'codex host must run four layered eval files'
grep -Fq -- '--runner echo mock-runner' "$tmp_dir/host.log" \
  || fail 'codex host must forward the nested runner command'
require_json_field "$codex_report" '.schema_version == "host-stage-report/v1"' 'codex layered schema changed'
require_json_field "$codex_report" '.stage == "layered_host_smoke"' 'codex layered stage changed'
require_json_field "$codex_report" '.host == "codex"' 'codex layered host changed'
require_json_field "$codex_report" '.status == "passed"' 'codex layered status changed'
require_json_field "$codex_report" '.failure_taxonomy == null' 'codex layered failure_taxonomy must be null'

codex_failure_report="$tmp_dir/codex-layered-failure-report.json"
if MOCK_HOST_LAYERED_LOG_FILE="$tmp_dir/host.log" \
  bash "$wrapper" \
    --host codex \
    --codex-runner "$tmp_dir/does-not-exist" \
    --report-json "$codex_failure_report" >"$tmp_dir/codex-fail.out" 2>"$tmp_dir/codex-fail.err"; then
  fail 'codex layered failure case must fail'
fi
require_json_field "$codex_failure_report" '.schema_version == "host-stage-report/v1"' 'codex layered failure schema changed'
require_json_field "$codex_failure_report" '.stage == "layered_host_smoke"' 'codex layered failure stage changed'
require_json_field "$codex_failure_report" '.host == "codex"' 'codex layered failure host changed'
require_json_field "$codex_failure_report" '.status == "failed"' 'codex layered failure status changed'
require_json_field "$codex_failure_report" '.failure_taxonomy == "runner-missing"' 'codex layered failure taxonomy changed'

if bash "$wrapper" --claude-runner "$tmp_dir/mock-claude-runner" >"$tmp_dir/missing-host.out" 2>"$tmp_dir/missing-host.err"; then
  fail 'wrapper must reject missing --host'
fi
grep -Fq 'run layered host evals failed: --host is required' "$tmp_dir/missing-host.err" \
  || fail 'missing --host error message changed'

if bash "$wrapper" --host invalid >"$tmp_dir/invalid-host.out" 2>"$tmp_dir/invalid-host.err"; then
  fail 'wrapper must reject invalid host values'
fi
grep -Fq 'run layered host evals failed: unsupported host: invalid' "$tmp_dir/invalid-host.err" \
  || fail 'invalid host error message changed'

printf 'run layered host evals tests passed\n'
