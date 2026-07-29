#!/usr/bin/env bash

resolve_packaged_collect_diff_context_cli() {
  [ "$#" -eq 1 ] || return 1
  local scripts_dir="$1"
  local os_name
  local arch_name
  local binary_name

  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch_name="$(uname -m)"
  case "$os_name" in
    darwin) os_name='darwin' ;;
    linux) os_name='linux' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) return 1 ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch_name='amd64' ;;
    arm64|aarch64) arch_name='arm64' ;;
    *) return 1 ;;
  esac

  binary_name="collect_diff_context-${os_name}-${arch_name}"
  [ "$os_name" = 'windows' ] && binary_name="${binary_name}.exe"
  [ -x "$scripts_dir/bin/$binary_name" ] || return 1
  printf '%s\n' "$scripts_dir/bin/$binary_name"
}
