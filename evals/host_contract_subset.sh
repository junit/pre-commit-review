#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
report_json_path=''
stage_name='contract_subset'
stage_status='failed'
failure_taxonomy='null'
host_json='null'

fail() {
  emit_stage_report
  printf 'host contract subset failed: %s\n' "$*" >&2
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

require_runtime_file() {
  local file_path="$1"
  local display_path="$2"

  if [ ! -f "$file_path" ]; then
    stage_status='failed'
    failure_taxonomy='"missing-file"'
    fail "missing $display_path"
  fi
}

usage() {
  cat <<'EOF'
Usage: host_contract_subset.sh [--report-json PATH]

Check that the required host-only contract surface exists.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
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
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

require_runtime_file "$repo_root/evals/check_host_availability.sh" 'evals/check_host_availability.sh'
require_runtime_file "$repo_root/evals/check_host_availability_test.sh" 'evals/check_host_availability_test.sh'
require_runtime_file "$repo_root/evals/host_failure_taxonomy.sh" 'evals/host_failure_taxonomy.sh'
require_runtime_file "$repo_root/evals/host_failure_taxonomy_test.sh" 'evals/host_failure_taxonomy_test.sh'
require_runtime_file "$repo_root/evals/run_layered_host_evals.sh" 'evals/run_layered_host_evals.sh'
require_runtime_file "$repo_root/evals/run_layered_host_evals_test.sh" 'evals/run_layered_host_evals_test.sh'
require_runtime_file "$repo_root/evals/run_host_readiness_pipeline.sh" 'evals/run_host_readiness_pipeline.sh'
require_runtime_file "$repo_root/evals/run_host_readiness_pipeline_test.sh" 'evals/run_host_readiness_pipeline_test.sh'

stage_status='passed'
failure_taxonomy='null'
emit_stage_report
printf 'host contract subset passed\n'
