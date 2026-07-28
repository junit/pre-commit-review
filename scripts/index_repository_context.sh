#!/usr/bin/env bash
set -uo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
RESOLVER="$SCRIPT_DIR/lib/repository_context_cli.sh"
SECRET_SCAN_MODE="${PRE_COMMIT_REVIEW_SECRET_SCAN:-auto}"

tmp_output="$(mktemp)"
tmp_error="$(mktemp)"
tmp_sanitized="$(mktemp)"
tmp_report="$(mktemp)"
tmp_sanitizer_error="$(mktemp)"
trap 'rm -f "$tmp_output" "$tmp_error" "$tmp_sanitized" "$tmp_report" "$tmp_sanitizer_error"' EXIT

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

valid_fingerprint() {
  local value="$1"
  case "$value" in
    ''|*[!0-9a-f]*) return 1 ;;
  esac
  [ "${#value}" -eq 40 ] || [ "${#value}" -eq 64 ]
}

argument_present() {
  local wanted="$1"
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      "$wanted"|"$wanted="*) return 0 ;;
    esac
    shift
  done
  return 1
}

valid_absolute_path() {
  case "$1" in
    /*) return 0 ;;
    *) return 1 ;;
  esac
}

emit_unavailable() {
  local action="$1"
  local fingerprint="$2"
  local reason="$3"
  local scope_json='null'
  if [ "$action" = 'build' ]; then
    scope_json="\"$fingerprint\""
  fi
  printf '%s' "{\"schema_version\":1,\"kind\":\"repository_index_report\",\"action\":\"$action\",\"status\":\"unavailable\",\"scope_fingerprint\":$scope_json,\"repository_id\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"generation_key\":null,\"metrics\":{\"elapsed_ms\":0,\"manifest_files\":0,\"manifest_bytes\":0,\"file_fact_hits\":0,\"file_fact_misses\":0,\"file_fact_writes\":0,\"parsed_files\":0,\"parsed_bytes\":0,\"symbols\":0,\"edges\":0,\"query_rows\":0,\"generation_bytes\":0,\"output_bytes\":0},\"limitations\":[{\"code\":\"repository-context-cli-unavailable\",\"path\":null,\"symbol_id\":null,\"reason\":\"Trusted repository context CLI is unavailable.\",\"interpretation\":\"$reason\"}]}"
}

if [ "${1:-}" = 'index' ]; then
  shift
fi
action="${1:-}"
case "$action" in
  build|doctor|inspect|clean) ;;
  *)
    printf '%s\n' 'index_repository_context: expected index build, doctor, inspect, or clean' >&2
    exit 2
    ;;
esac

case "${PRE_COMMIT_REVIEW_CACHE_DIR:-}" in
  ''|/*) ;;
  *)
    printf '%s\n' 'index_repository_context: PRE_COMMIT_REVIEW_CACHE_DIR must be an absolute path' >&2
    exit 2
    ;;
esac
cache_dir="$(extract_argument --cache-dir "$@" 2>/dev/null || true)"
if argument_present --cache-dir "$@" && ! valid_absolute_path "$cache_dir"; then
  printf '%s\n' 'index_repository_context: --cache-dir must be an absolute path' >&2
  exit 2
fi

scope=''
if [ "$action" = 'build' ]; then
  source_name="$(extract_argument --source "$@" 2>/dev/null || true)"
  scope="$(extract_argument --expect-scope "$@" 2>/dev/null || true)"
  case "$source_name" in
    staged|unstaged|branch) ;;
    *)
      printf '%s\n' 'index_repository_context: --source is required and must be staged, unstaged, or branch' >&2
      exit 2
      ;;
  esac
  if ! valid_fingerprint "$scope"; then
    printf '%s\n' 'index_repository_context: --expect-scope must be 40 or 64 lowercase hexadecimal characters' >&2
    exit 2
  fi
fi

if [ ! -r "$RESOLVER" ]; then
  emit_unavailable "$action" "$scope" 'resolver-unavailable'
  exit 0
fi
# shellcheck source=scripts/lib/repository_context_cli.sh
source "$RESOLVER"
resolver_exit=0
repository_context_bin="$(resolve_repository_context_cli "$SCRIPT_DIR")" || resolver_exit=$?
if [ "$resolver_exit" -eq 2 ]; then
  printf '%s\n' 'index_repository_context: repository context CLI override must be an absolute executable path' >&2
  exit 2
fi
if [ "$resolver_exit" -ne 0 ] || [ -z "$repository_context_bin" ]; then
  emit_unavailable "$action" "$scope" 'binary-unavailable'
  exit 0
fi

command_exit=0
"$repository_context_bin" index "$@" >"$tmp_output" 2>"$tmp_error" || command_exit=$?

if [ "$SECRET_SCAN_MODE" != 'off' ]; then
  sanitizer_bin="${PRE_COMMIT_REVIEW_SANITIZER_BIN:-}"
  if [ -z "$sanitizer_bin" ] && [ -x "$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli" ]; then
    sanitizer_bin="$SCRIPT_DIR/../collect-diff-context-cli/target/release/collect-diff-context-cli"
  fi
  if [ -n "$sanitizer_bin" ] && [ -x "$sanitizer_bin" ]; then
    sanitize_file_in_place() {
      local input_file="$1"
      local stream_name="$2"
      local sanitize_exit=0
      [ -s "$input_file" ] || return 0
      : >"$tmp_sanitized"
      : >"$tmp_report"
      : >"$tmp_sanitizer_error"
      PRE_COMMIT_REVIEW_SANITIZE_REPORT="$tmp_report" \
      PRE_COMMIT_REVIEW_SANITIZE_STREAM="$stream_name" \
        "$sanitizer_bin" --sanitize-stdin \
          <"$input_file" >"$tmp_sanitized" 2>"$tmp_sanitizer_error" \
        || sanitize_exit=$?
      if [ "$sanitize_exit" -eq 0 ] \
        && grep -Fq 'protocol: pcr-sanitizer-v1' "$tmp_report" \
        && grep -Eq '^status: (clean|redacted)$' "$tmp_report"; then
        mv "$tmp_sanitized" "$input_file"
      fi
    }
    sanitize_file_in_place "$tmp_output" 'repository-index-stdout'
    sanitize_file_in_place "$tmp_error" 'repository-index-stderr'
  fi
fi

if [ "$command_exit" -ne 0 ] && [ "$command_exit" -ne 3 ]; then
  [ -s "$tmp_error" ] && cat "$tmp_error" >&2
  emit_unavailable "$action" "$scope" 'operation-failed'
  exit 0
fi

cat "$tmp_output"
[ -s "$tmp_error" ] && cat "$tmp_error" >&2
exit "$command_exit"
