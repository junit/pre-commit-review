#!/usr/bin/env bash

resolve_repository_context_cli() {
  local script_dir="$1"
  local os_name arch_name binary_name

  if [ -n "${PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN:-}" ]; then
    case "$PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN" in
      /*) ;;
      *) return 2 ;;
    esac
    [ -x "$PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN" ] || return 2
    printf '%s\n' "$PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN"
    return 0
  fi

  if [ -x "$script_dir/../collect-diff-context-cli/target/release/repository-context-cli" ]; then
    printf '%s\n' "$script_dir/../collect-diff-context-cli/target/release/repository-context-cli"
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
    *) return 1 ;;
  esac

  binary_name="repository_context-${os_name}-${arch_name}"
  [ "$os_name" = 'windows' ] && binary_name="${binary_name}.exe"
  [ -x "$script_dir/bin/$binary_name" ] || return 1
  printf '%s\n' "$script_dir/bin/$binary_name"
}
