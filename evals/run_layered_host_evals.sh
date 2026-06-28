#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"
claude_runner="$script_dir/output_eval_claude_runner.sh"
codex_runner="$script_dir/output_eval_codex_runner.sh"

eval_files=(
  "$script_dir/output/routine-output-eval.json"
  "$script_dir/output/advanced-output-eval.json"
  "$script_dir/output/visual-output-eval.json"
  "$script_dir/output/localization-output-eval.json"
)

host=''
report_json_path=''
stage_name='layered_host_smoke'
stage_status='failed'
failure_taxonomy='null'
host_json='null'
forward_args=()

usage() {
  cat <<'EOF'
Usage: run_layered_host_evals.sh --host <claude|codex> [options passed through to host runner]

Run the layered output eval matrix through a specific host wrapper.

Options:
  --host HOST            Required. One of: claude, codex
  --claude-runner PATH   Override the Claude host runner script
  --codex-runner PATH    Override the Codex host runner script
  -h, --help             Show this help

All other arguments are forwarded to the selected host runner unchanged.
EOF
}

fail() {
  emit_stage_report
  printf 'run layered host evals failed: %s\n' "$*" >&2
  exit 1
}

build_stage_report_json() {
  jq -cn \
    --arg schema_version "host-stage-report/v1" \
    --arg stage "$stage_name" \
    --argjson host "$host_json" \
    --arg status "$stage_status" \
    --argjson failure_taxonomy "$failure_taxonomy" \
    '{
      schema_version: $schema_version,
      stage: $stage,
      host: $host,
      status: $status,
      failure_taxonomy: $failure_taxonomy
    }'
}

emit_stage_report() {
  [ -n "$report_json_path" ] || return 0
  build_stage_report_json >"$report_json_path"
}

taxonomy_fail() {
  local failure_type="$1"
  shift
  stage_status='failed'
  failure_taxonomy="\"$failure_type\""
  emit_stage_report
  host_eval_taxonomy_fail "$failure_type" "$*"
}

finalize_report_on_exit() {
  local exit_code="$1"

  if [ "$exit_code" -ne 0 ] && [ -n "$report_json_path" ] && [ ! -f "$report_json_path" ]; then
    emit_stage_report
  fi
}

trap 'finalize_report_on_exit "$?"' EXIT

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      [ "$#" -gt 0 ] || fail '--host requires a value'
      host="$1"
      ;;
    --claude-runner)
      shift
      [ "$#" -gt 0 ] || fail '--claude-runner requires a value'
      claude_runner="$1"
      ;;
    --codex-runner)
      shift
      [ "$#" -gt 0 ] || fail '--codex-runner requires a value'
      codex_runner="$1"
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json_path="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      forward_args+=("$1")
      ;;
  esac
  shift
done

[ -n "$host" ] || fail '--host is required'
host_json="\"$host\""

case "$host" in
  claude) selected_runner="$claude_runner" ;;
  codex) selected_runner="$codex_runner" ;;
  *) fail "unsupported host: $host" ;;
esac

[ -f "$selected_runner" ] \
  || taxonomy_fail 'runner-missing' "missing host runner: $selected_runner"

for eval_file in "${eval_files[@]}"; do
  runner_exit_code=0
  [ -f "$eval_file" ] || fail "missing eval file: $eval_file"
  printf '=== Running %s on %s ===\n' "$host" "$(basename "$eval_file")"
  bash "$selected_runner" --eval-file "$eval_file" "${forward_args[@]}" || runner_exit_code=$?
  if [ "$runner_exit_code" -ne 0 ]; then
    taxonomy_fail 'runner-exit-nonzero' "host runner exited non-zero on $(basename "$eval_file") for $host"
  fi
done

stage_status='passed'
failure_taxonomy='null'
emit_stage_report
