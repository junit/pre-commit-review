#!/usr/bin/env bash
set -uo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
RESOLVER="$SCRIPT_DIR/lib/repository_context_cli.sh"
SECRET_SCAN_MODE="${PRE_COMMIT_REVIEW_SECRET_SCAN:-auto}"

tmp_output="$(mktemp)"
tmp_error="$(mktemp)"
tmp_sanitized="$(mktemp)"
tmp_report="$(mktemp)"
trap 'rm -f "$tmp_output" "$tmp_error" "$tmp_sanitized" "$tmp_report"' EXIT

extract_argument() {
  local wanted="$1"
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      "$wanted")
        [ "$#" -ge 2 ] || return 1
        printf '%s\n' "$2"
        return 0
        ;;
      "$wanted="*)
        printf '%s\n' "${1#*=}"
        return 0
        ;;
    esac
    shift
  done
  return 1
}

emit_unavailable() {
  local source="$1"
  local fingerprint="$2"
  local mode="$3"
  local reason="$4"
  printf '%s\n' '## Impact Context JSON'
  printf '%s' "{\"schema_version\":1,\"kind\":\"impact_context\",\"scope\":{\"fingerprint\":\"$fingerprint\",\"source\":\"$source\",\"candidate_digest\":\"0000000000000000000000000000000000000000000000000000000000000000\"},\"mode\":\"$mode\",\"status\":\"unavailable\",\"providers\":[],\"units\":[],\"changed_symbols\":[],\"impact_edges\":[],\"domain_summaries\":[],\"coverage\":{\"total_candidate_files\":0,\"changed_candidate_files\":0,\"syntax_eligible_files\":0,\"parsed_files\":0,\"clean_parse_files\":0,\"recovered_parse_files\":0,\"degraded_parse_files\":0,\"unsupported_files\":0,\"resource_limited_files\":0,\"unavailable_files\":0,\"cache_hits\":0,\"cache_misses\":0,\"cache_stale\":0,\"cache_corrupt\":0,\"requested_graph_depth\":0,\"reached_graph_depth\":0,\"graph_index_completeness\":\"unavailable\",\"graph_query_completeness\":\"unavailable\",\"output_truncated\":false},\"limitations\":[{\"limitation_id\":\"0000000000000001\",\"code\":\"repository-context-cli-unavailable\",\"provider_id\":null,\"path\":null,\"symbol_id\":null,\"reason\":\"Trusted repository context CLI is unavailable.\",\"interpretation\":\"$reason\",\"improvable_in_deep_mode\":false}],\"metrics\":{\"elapsed_ms\":0,\"candidate_input_files\":0,\"candidate_input_bytes\":0,\"nodes_visited\":0,\"max_nesting_depth\":0,\"facts_emitted\":0,\"edges_emitted\":0,\"summaries_emitted\":0,\"output_bytes\":0}}"
}

if [ "${1:-}" = 'collect' ]; then
  shift
fi
source_name="$(extract_argument --source "$@" 2>/dev/null || true)"
expected_scope="$(extract_argument --expect-scope "$@" 2>/dev/null || true)"
mode_name="$(extract_argument --mode "$@" 2>/dev/null || true)"
case "$source_name" in
  staged|unstaged|branch) ;;
  *)
    printf '%s\n' 'collect_impact_context: --source is required and must be staged, unstaged, or branch' >&2
    exit 2
    ;;
esac
case "$expected_scope" in
  ''|*[!0-9a-f]*)
    printf '%s\n' 'collect_impact_context: --expect-scope must be lowercase hexadecimal' >&2
    exit 2
    ;;
esac
if [ "${#expected_scope}" -ne 40 ] && [ "${#expected_scope}" -ne 64 ]; then
  printf '%s\n' 'collect_impact_context: --expect-scope must contain 40 or 64 characters' >&2
  exit 2
fi
case "$mode_name" in
  fast|deep) ;;
  *)
    printf '%s\n' 'collect_impact_context: --mode is required and must be fast or deep' >&2
    exit 2
    ;;
esac

if [ ! -r "$RESOLVER" ]; then
  emit_unavailable "$source_name" "$expected_scope" "$mode_name" 'resolver-unavailable'
  exit 0
fi
# shellcheck source=scripts/lib/repository_context_cli.sh
source "$RESOLVER"
resolver_exit=0
repository_context_bin="$(resolve_repository_context_cli "$SCRIPT_DIR")" || resolver_exit=$?
if [ "$resolver_exit" -eq 2 ]; then
  printf '%s\n' 'collect_impact_context: repository context CLI override must be an absolute executable path' >&2
  exit 2
fi
if [ "$resolver_exit" -ne 0 ] || [ -z "$repository_context_bin" ]; then
  emit_unavailable "$source_name" "$expected_scope" "$mode_name" 'binary-unavailable'
  exit 0
fi

collector_exit=0
"$repository_context_bin" collect "$@" >"$tmp_output" 2>"$tmp_error" || collector_exit=$?
if [ "$collector_exit" -ne 0 ] && [ "$collector_exit" -ne 3 ]; then
  cat "$tmp_error" >&2
  emit_unavailable "$source_name" "$expected_scope" "$mode_name" 'collection-failed'
  exit 0
fi

if [ "$SECRET_SCAN_MODE" != 'off' ]; then
  sanitizer_bin="${PRE_COMMIT_REVIEW_SANITIZER_BIN:-}"
  if [ -z "$sanitizer_bin" ] && [ -x "$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli" ]; then
    sanitizer_bin="$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli"
  fi
  if [ -n "$sanitizer_bin" ] && [ -x "$sanitizer_bin" ]; then
    sanitize_exit=0
    PRE_COMMIT_REVIEW_SANITIZE_REPORT="$tmp_report" \
    PRE_COMMIT_REVIEW_SANITIZE_STREAM='impact-context-stdout' \
      "$sanitizer_bin" --sanitize-stdin <"$tmp_output" >"$tmp_sanitized" 2>>"$tmp_error" \
      || sanitize_exit=$?
    if [ "$sanitize_exit" -eq 0 ] \
      && grep -Fq 'protocol: pcr-sanitizer-v1' "$tmp_report" \
      && grep -Eq '^status: (clean|redacted)$' "$tmp_report"; then
      mv "$tmp_sanitized" "$tmp_output"
    fi
  fi
fi

printf '%s\n' '## Impact Context JSON'
cat "$tmp_output"
[ -s "$tmp_error" ] && cat "$tmp_error" >&2
exit "$collector_exit"
