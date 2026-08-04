#!/usr/bin/env bash
set -uo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
resolver="$script_dir/lib/repository_context_provider_cli.sh"

emit_error() {
  printf 'run_repository_context_provider: %s\n' "$1" >&2
}

if [ ! -r "$resolver" ]; then
  emit_error 'provider CLI resolver is unavailable'
  exit 2
fi
# shellcheck source=scripts/lib/repository_context_provider_cli.sh
source "$resolver"

resolver_status=0
provider_cli="$(resolve_repository_context_provider_cli "$script_dir")" \
  || resolver_status=$?
if [ "$resolver_status" -eq 2 ]; then
  emit_error 'provider CLI override is invalid'
  exit 2
fi
if [ "$resolver_status" -ne 0 ] || [ -z "$provider_cli" ]; then
  emit_error 'provider CLI is unavailable'
  exit 2
fi

tmp_dir="$(mktemp -d)" || {
  emit_error 'temporary output cannot be created'
  exit 3
}
tmp_output="$tmp_dir/stdout"
tmp_error="$tmp_dir/stderr"
trap 'rm -rf "$tmp_dir"' EXIT

provider_status=0
"$provider_cli" "$@" >"$tmp_output" 2>"$tmp_error" || provider_status=$?

case "$provider_status" in
  0)
    if [ -s "$tmp_error" ]; then
      emit_error 'provider CLI violated its stderr contract'
      exit 3
    fi
    cat "$tmp_output"
    ;;
  2)
    emit_error 'provider CLI rejected the invocation'
    exit 2
    ;;
  3)
    emit_error 'provider CLI execution failed'
    exit 3
    ;;
  *)
    emit_error 'provider CLI returned an invalid exit code'
    exit 3
    ;;
esac
