#!/usr/bin/env bash

host_eval_taxonomy_fail() {
  local failure_type="$1"
  shift
  printf 'host eval failure [%s]: %s\n' "$failure_type" "$*" >&2
  exit 1
}

host_eval_require_command() {
  local command_name="$1"
  command -v "$command_name" >/dev/null 2>&1 \
    || host_eval_taxonomy_fail 'missing-binary' "required command not found: $command_name"
}
