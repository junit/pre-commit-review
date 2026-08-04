#!/usr/bin/env bash

resolve_repository_context_provider_cli() {
  local script_dir="$1"
  local os_name arch_name binary_name override release_dir release_binary

  override="${PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN:-}"
  if [ -n "$override" ]; then
    case "$override" in
      /*|[A-Za-z]:[\\/]*) ;;
      *) return 2 ;;
    esac
    [ -x "$override" ] || return 2
    printf '%s\n' "$override"
    return 0
  fi

  release_dir="$script_dir/../collect-diff-context-cli/target/release"
  release_binary="$release_dir/repository-context-provider-cli"
  if [ -x "$release_binary" ]; then
    release_dir="$(CDPATH='' cd -- "$release_dir" && pwd -P)" || return 1
    printf '%s\n' "$release_dir/repository-context-provider-cli"
    return 0
  fi

  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch_name="$(uname -m)"
  case "$os_name" in
    darwin) os_name=darwin ;;
    linux) os_name=linux ;;
    msys*|mingw*|cygwin*) os_name=windows ;;
    *) return 1 ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch_name=amd64 ;;
    arm64|aarch64) arch_name=arm64 ;;
    *) return 1 ;;
  esac

  binary_name="repository_context_provider-${os_name}-${arch_name}"
  [ "$os_name" = windows ] && binary_name="${binary_name}.exe"
  [ -x "$script_dir/bin/$binary_name" ] || return 1
  printf '%s\n' "$script_dir/bin/$binary_name"
}
