#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
pipeline="$script_dir/run_host_readiness_pipeline.sh"
report_json_path=''
report_dir=''
fail_fast='false'
claude_bin=''
codex_bin=''

usage() {
  cat <<'EOF'
Usage: run_cross_host_readiness.sh [options]

Run the single-host readiness pipeline for claude and codex, then emit one aggregated summary.

Options:
  --pipeline <path>      Override the single-host readiness pipeline entrypoint
  --report-dir <path>    Directory for per-host JSON reports
  --report-json <path>   Output path for the aggregated JSON summary
  --fail-fast            Stop after the first failing host and mark later hosts as skipped
  --claude-bin <path>    Forward a custom Claude binary path to the single-host pipeline
  --codex-bin <path>     Forward a custom Codex binary path to the single-host pipeline
  -h, --help             Show this help
EOF
}

fail() {
  printf 'run cross-host readiness failed: %s\n' "$*" >&2
  exit 1
}

append_json_string() {
  local current_json="$1"
  local value="$2"

  jq -cn --argjson current "$current_json" --arg value "$value" '$current + [$value]'
}

started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
started_epoch_ms="$(date -u +"%s000")"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --fail-fast)
      fail_fast='true'
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json_path="$1"
      ;;
    --report-dir)
      shift
      [ "$#" -gt 0 ] || fail '--report-dir requires a value'
      report_dir="$1"
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
    --pipeline)
      shift
      [ "$#" -gt 0 ] || fail '--pipeline requires a value'
      pipeline="$1"
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

[ -n "$report_json_path" ] || fail '--report-json is required'

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

if [ -z "$report_dir" ]; then
  report_dir="$tmp_dir/reports"
fi
mkdir -p "$report_dir"

mode='complete-matrix'
if [ "$fail_fast" = 'true' ]; then
  mode='fail-fast'
fi

overall_status='passed'
failed_hosts='[]'
skipped_hosts='[]'

claude_execution_status='skipped'
claude_report='null'
codex_execution_status='skipped'
codex_report='null'

run_one_host() {
  local host="$1"
  local report_file="$2"
  local status_var="$3"
  local report_var="$4"
  local host_bin="$5"

  local exit_code=0
  local command=(bash "$pipeline" --host "$host" --report-json "$report_file")
  if [ -n "$host_bin" ]; then
    case "$host" in
      claude)
        command+=(--claude-bin "$host_bin")
        ;;
      codex)
        command+=(--codex-bin "$host_bin")
        ;;
    esac
  fi

  if "${command[@]}"; then
    exit_code=0
  else
    exit_code=$?
  fi

  [ -f "$report_file" ] || fail "missing host report for $host: $report_file"

  printf -v "$status_var" '%s' 'completed'
  printf -v "$report_var" '%s' "$(cat "$report_file")"

  if [ "$exit_code" -ne 0 ]; then
    overall_status='failed'
    failed_hosts="$(append_json_string "$failed_hosts" "$host")"
    return "$exit_code"
  fi

  return 0
}

claude_report_file="$report_dir/claude-readiness.json"
codex_report_file="$report_dir/codex-readiness.json"

claude_exit_code=0
printf '=== Cross-Host Readiness: claude ===\n'
run_one_host claude "$claude_report_file" claude_execution_status claude_report "$claude_bin" || claude_exit_code=$?

if [ "$claude_exit_code" -ne 0 ] && [ "$fail_fast" = 'true' ]; then
  skipped_hosts="$(append_json_string "$skipped_hosts" 'codex')"
else
  printf '=== Cross-Host Readiness: codex ===\n'
  run_one_host codex "$codex_report_file" codex_execution_status codex_report "$codex_bin" || true
fi

finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
finished_epoch_ms="$(date -u +"%s000")"
duration_ms=$((finished_epoch_ms - started_epoch_ms))

report_json="$(jq -cn \
  --arg schema_version "cross-host-readiness-report/v1" \
  --arg mode "$mode" \
  --arg overall_status "$overall_status" \
  --arg started_at "$started_at" \
  --arg finished_at "$finished_at" \
  --argjson duration_ms "$duration_ms" \
  --argjson failed_hosts "$failed_hosts" \
  --argjson skipped_hosts "$skipped_hosts" \
  --arg claude_execution_status "$claude_execution_status" \
  --arg codex_execution_status "$codex_execution_status" \
  --argjson claude_report "$claude_report" \
  --argjson codex_report "$codex_report" \
  '{
    schema_version: $schema_version,
    mode: $mode,
    overall_status: $overall_status,
    failed_hosts: $failed_hosts,
    skipped_hosts: $skipped_hosts,
    started_at: $started_at,
    finished_at: $finished_at,
    duration_ms: $duration_ms,
    hosts: {
      claude: {
        execution_status: $claude_execution_status,
        report: $claude_report
      },
      codex: {
        execution_status: $codex_execution_status,
        report: $codex_report
      }
    }
  }')"

printf '%s\n' "$report_json" >"$report_json_path"

if [ "$overall_status" = 'failed' ]; then
  exit 1
fi
