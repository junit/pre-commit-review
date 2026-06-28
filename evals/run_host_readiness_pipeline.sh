#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
availability_gate="$script_dir/check_host_availability.sh"
layered_host_runner="$script_dir/run_layered_host_evals.sh"
contract_subset="$script_dir/host_contract_subset.sh"

host=''
claude_bin=''
codex_bin=''
report_json_path=''
print_report_json='false'
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
started_epoch_ms="$(date -u +"%s000")"

availability_status='skipped'
layered_host_smoke_status='skipped'
contract_subset_status='skipped'
overall_status='failed'
failed_stage='null'
failure_taxonomy='null'

forward_args=()

usage() {
  cat <<'EOF'
Usage: run_host_readiness_pipeline.sh --host <claude|codex> [options]

Run the single-host readiness sequence:
  1. availability
  2. layered host smoke
  3. contract subset
EOF
}

fail() {
  printf 'run host readiness pipeline failed: %s\n' "$*" >&2
  exit 1
}

require_stage_report() {
  local report_file="$1"
  local stage_key="$2"

  [ -f "$report_file" ] || fail "missing stage report for $stage_key: $report_file"
  jq -e --arg stage "$stage_key" '
    .schema_version == "host-stage-report/v1"
    and .stage == $stage
    and (.status == "passed" or .status == "failed")
  ' "$report_file" >/dev/null || fail "invalid stage report for $stage_key: $report_file"
}

build_report_json() {
  local finished_at="$1"
  local duration_ms="$2"

  jq -cn \
    --arg schema_version "host-readiness-report/v1" \
    --arg host "$host" \
    --arg overall_status "$overall_status" \
    --arg started_at "$started_at" \
    --arg finished_at "$finished_at" \
    --argjson duration_ms "$duration_ms" \
    --arg availability_status "$availability_status" \
    --arg layered_host_smoke_status "$layered_host_smoke_status" \
    --arg contract_subset_status "$contract_subset_status" \
    --argjson failed_stage "$failed_stage" \
    --argjson failure_taxonomy "$failure_taxonomy" \
    '{
      schema_version: $schema_version,
      host: $host,
      overall_status: $overall_status,
      failed_stage: $failed_stage,
      failure_taxonomy: $failure_taxonomy,
      started_at: $started_at,
      finished_at: $finished_at,
      duration_ms: $duration_ms,
      stages: {
        availability: {status: $availability_status},
        layered_host_smoke: {status: $layered_host_smoke_status},
        contract_subset: {status: $contract_subset_status}
      }
    }'
}

emit_final_report() {
  local finished_at
  local finished_epoch_ms
  local duration_ms
  local report_json

  finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  finished_epoch_ms="$(date -u +"%s000")"
  duration_ms=$((finished_epoch_ms - started_epoch_ms))
  report_json="$(build_report_json "$finished_at" "$duration_ms")"

  if [ -n "$report_json_path" ]; then
    printf '%s\n' "$report_json" >"$report_json_path"
  fi

  if [ "$print_report_json" = 'true' ]; then
    printf '%s\n' "$report_json"
  fi
}

run_stage_with_report() {
  local stage_key="$1"
  local out_file="$2"
  local err_file="$3"
  local report_file="$4"
  shift 4

  if "$@" --report-json "$report_file" >"$out_file" 2>"$err_file"; then
    cat "$out_file"
    [ ! -s "$err_file" ] || cat "$err_file" >&2
    require_stage_report "$report_file" "$stage_key"
    case "$stage_key" in
      availability)
        availability_status='passed'
        ;;
      layered_host_smoke)
        layered_host_smoke_status='passed'
        ;;
      contract_subset)
        contract_subset_status='passed'
        ;;
    esac
    return 0
  fi

  cat "$out_file"
  cat "$err_file" >&2
  require_stage_report "$report_file" "$stage_key"

  case "$stage_key" in
    availability)
      availability_status='failed'
      ;;
    layered_host_smoke)
      layered_host_smoke_status='failed'
      ;;
    contract_subset)
      contract_subset_status='failed'
      ;;
  esac

  failed_stage="\"$stage_key\""
  failure_taxonomy="$(jq -c '.failure_taxonomy' "$report_file")"

  return 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      [ "$#" -gt 0 ] || fail '--host requires a value'
      host="$1"
      ;;
    --claude-bin)
      shift
      [ "$#" -gt 0 ] || fail '--claude-bin requires a value'
      claude_bin="$1"
      ;;
    --codex-bin)
      shift
      [ "$#" -gt 0 ] || fail '--codex-bin requires a value'
      codex_bin="$1"
      ;;
    --availability-gate)
      shift
      [ "$#" -gt 0 ] || fail '--availability-gate requires a value'
      availability_gate="$1"
      ;;
    --layered-host-runner)
      shift
      [ "$#" -gt 0 ] || fail '--layered-host-runner requires a value'
      layered_host_runner="$1"
      ;;
    --contract-subset)
      shift
      [ "$#" -gt 0 ] || fail '--contract-subset requires a value'
      contract_subset="$1"
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json_path="$1"
      ;;
    --print-report-json)
      print_report_json='true'
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

availability_args=(--host "$host")
layered_args=(--host "$host")

case "$host" in
  claude)
    if [ -n "$claude_bin" ]; then
      availability_args+=(--claude-bin "$claude_bin")
      layered_args+=(--claude-bin "$claude_bin")
    fi
    ;;
  codex)
    if [ -n "$codex_bin" ]; then
      availability_args+=(--codex-bin "$codex_bin")
      layered_args+=(--codex-bin "$codex_bin")
    fi
    ;;
  *)
    fail "unsupported host: $host"
    ;;
esac

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

printf '=== Availability: %s ===\n' "$host"
if ! run_stage_with_report availability "$tmp_dir/availability.out" "$tmp_dir/availability.err" "$tmp_dir/availability-report.json" \
  bash "$availability_gate" "${availability_args[@]}"; then
  emit_final_report
  exit 1
fi

printf '=== Layered Host Smoke: %s ===\n' "$host"
layered_command=(bash "$layered_host_runner" "${layered_args[@]}")
if [ "${#forward_args[@]}" -gt 0 ]; then
  layered_command+=("${forward_args[@]}")
fi
if ! run_stage_with_report layered_host_smoke "$tmp_dir/layered.out" "$tmp_dir/layered.err" "$tmp_dir/layered-report.json" \
  "${layered_command[@]}"; then
  emit_final_report
  exit 1
fi

printf '=== Contract Subset: %s ===\n' "$host"
if ! run_stage_with_report contract_subset "$tmp_dir/contract.out" "$tmp_dir/contract.err" "$tmp_dir/contract-report.json" \
  bash "$contract_subset"; then
  emit_final_report
  exit 1
fi

overall_status='passed'
emit_final_report
