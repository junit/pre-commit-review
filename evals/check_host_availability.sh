#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"

host=''
claude_bin='claude'
codex_bin='codex'
report_json_path=''
stage_name='availability'
stage_status='failed'
failure_taxonomy='null'
host_json='null'

usage() {
  cat <<'EOF'
Usage: check_host_availability.sh --host <claude|codex> [--claude-bin PATH] [--codex-bin PATH]

Check host binary availability and run a minimal host-specific handshake.
EOF
}

fail() {
  emit_stage_report
  printf 'check host availability failed: %s\n' "$*" >&2
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

[ -n "$host" ] || fail '--host is required'
host_json="\"$host\""

resolve_binary() {
  local binary_path="$1"

  if [ -x "$binary_path" ]; then
    printf '%s\n' "$binary_path"
    return 0
  fi

  command -v "$binary_path" 2>/dev/null || return 1
}

run_handshake() {
  local selected_host="$1"
  local binary_path="$2"
  local output=''
  local status=0

  case "$selected_host" in
    claude)
      output="$("$binary_path" -p --output-format text --permission-mode dontAsk --bare --no-session-persistence "say ok" 2>/dev/null)" || status=$?
      ;;
    codex)
      output="$(printf 'say ok\n' | "$binary_path" exec --skip-git-repo-check --ephemeral --sandbox read-only --color never - 2>/dev/null)" || status=$?
      ;;
    *)
      fail "unsupported host: $selected_host"
      ;;
  esac

  [ "$status" -eq 0 ] \
    || taxonomy_fail 'runner-exit-nonzero' "handshake exited non-zero for host $selected_host: $status"
  [ -n "$output" ] \
    || taxonomy_fail 'protocol-mismatch' "handshake produced empty output for host $selected_host"
}

case "$host" in
  claude)
    selected_bin="$(resolve_binary "$claude_bin")" \
      || taxonomy_fail 'missing-binary' "claude binary not found: $claude_bin"
    run_handshake claude "$selected_bin"
    stage_status='passed'
    failure_taxonomy='null'
    emit_stage_report
    printf 'host availability ok [claude]: binary and handshake succeeded\n'
    ;;
  codex)
    selected_bin="$(resolve_binary "$codex_bin")" \
      || taxonomy_fail 'missing-binary' "codex binary not found: $codex_bin"
    run_handshake codex "$selected_bin"
    stage_status='passed'
    failure_taxonomy='null'
    emit_stage_report
    printf 'host availability ok [codex]: binary and handshake succeeded\n'
    ;;
  *)
    fail "unsupported host: $host"
    ;;
esac
