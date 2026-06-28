#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
pipeline="$script_dir/run_host_readiness_pipeline.sh"
artifact_dir="$repo_root/.tmp/real-host-smoke"

host=''
claude_bin=''
codex_bin=''
print_report_json='false'
forward_args=()

usage() {
  cat <<'EOF'
Usage: run_real_host_smoke.sh --host <claude|codex> [options]

Run the host readiness pipeline against a real host CLI and save the final report
to a stable artifact path.

Options:
  --host HOST           Required. One of: claude, codex
  --pipeline PATH       Override the host readiness pipeline script
  --artifact-dir PATH   Directory for saved host-readiness-report/v1 files
  --claude-bin PATH     Override the Claude CLI binary path
  --codex-bin PATH      Override the Codex CLI binary path
  --print-report-json   Also print the final pipeline JSON to stdout
  -h, --help            Show this help

All other arguments are forwarded to run_host_readiness_pipeline.sh unchanged.
EOF
}

fail() {
  printf 'run real host smoke failed: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      [ "$#" -gt 0 ] || fail '--host requires a value'
      host="$1"
      ;;
    --pipeline)
      shift
      [ "$#" -gt 0 ] || fail '--pipeline requires a value'
      pipeline="$1"
      ;;
    --artifact-dir)
      shift
      [ "$#" -gt 0 ] || fail '--artifact-dir requires a value'
      artifact_dir="$1"
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
[ -f "$pipeline" ] || fail "missing pipeline: $pipeline"

mkdir -p "$artifact_dir"
report_json_path="$artifact_dir/$host-host-readiness-report.json"

pipeline_cmd=(bash "$pipeline" --host "$host" --report-json "$report_json_path")

case "$host" in
  claude)
    if [ -n "$claude_bin" ]; then
      pipeline_cmd+=(--claude-bin "$claude_bin")
    fi
    ;;
  codex)
    if [ -n "$codex_bin" ]; then
      pipeline_cmd+=(--codex-bin "$codex_bin")
    fi
    ;;
  *)
    fail "unsupported host: $host"
    ;;
esac

if [ "$print_report_json" = 'true' ]; then
  pipeline_cmd+=(--print-report-json)
fi

if [ "${#forward_args[@]}" -gt 0 ]; then
  pipeline_cmd+=("${forward_args[@]}")
fi

printf '=== Real Host Smoke: %s ===\n' "$host"
"${pipeline_cmd[@]}"
[ -f "$report_json_path" ] || fail "missing pipeline report: $report_json_path"
printf 'real host smoke report saved to %s\n' "$report_json_path"
