#!/usr/bin/env bash

resolve_static_analysis_cli() {
  local script_dir="$1"
  local os_name arch_name static_binary_name

  if [ -n "${PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN:-}" ]; then
    case "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN" in
      /*) ;;
      *) return 2 ;;
    esac
    [ -x "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN" ] || return 2
    printf '%s\n' "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN"
    return 0
  fi

  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch_name="$(uname -m)"
  case "$os_name" in
    darwin) os_name='darwin' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) os_name='linux' ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch_name='amd64' ;;
    arm64|aarch64) arch_name='arm64' ;;
    *) return 2 ;;
  esac

  static_binary_name="static_analysis-${os_name}-${arch_name}"
  [ "$os_name" = 'windows' ] && static_binary_name="${static_binary_name}.exe"
  if [ -x "$script_dir/../collect-diff-context-cli/target/release/static-analysis-cli" ]; then
    printf '%s\n' "$script_dir/../collect-diff-context-cli/target/release/static-analysis-cli"
    return 0
  fi
  [ -x "$script_dir/bin/$static_binary_name" ] || return 2
  printf '%s\n' "$script_dir/bin/$static_binary_name"
}
