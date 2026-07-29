#!/usr/bin/env bash

set -euo pipefail

case "$0" in
  */*) SCRIPT_PARENT=${0%/*} ;;
  *) SCRIPT_PARENT=. ;;
esac
SCRIPT_DIR="$(CDPATH='' cd -- "$SCRIPT_PARENT" && pwd -P)"
COMMAND_NAME="${SCRIPT_DIR##*/}/check_artifacts.sh"

if [ "$#" -ne 1 ]; then
  printf '%s: expected one absolute target root\n' "$COMMAND_NAME" >&2
  exit 2
fi

case "$1" in
  /*|[A-Za-z]:[\\/]*) ;;
  *)
    printf '%s: target root must be absolute\n' "$COMMAND_NAME" >&2
    exit 2
    ;;
esac

if ! TARGET_ROOT="$(CDPATH='' cd -- "$1" && pwd -P)"; then
  printf '%s: target root is unavailable\n' "$COMMAND_NAME" >&2
  exit 1
fi

RESOLVER="$TARGET_ROOT/scripts/lib/collect_diff_context_cli.sh"
if [ ! -r "$RESOLVER" ]; then
  printf '%s: target collector resolver is unavailable\n' "$COMMAND_NAME" >&2
  exit 1
fi
# shellcheck source=/dev/null
. "$RESOLVER"
if ! declare -F resolve_packaged_collect_diff_context_cli >/dev/null 2>&1; then
  printf '%s: target collector resolver is invalid\n' "$COMMAND_NAME" >&2
  exit 1
fi

if ! COLLECTOR="$(resolve_packaged_collect_diff_context_cli "$TARGET_ROOT/scripts")"; then
  printf '%s: target collector is unavailable\n' "$COMMAND_NAME" >&2
  exit 1
fi

exec "$COLLECTOR" artifacts doctor --target-root "$TARGET_ROOT"
