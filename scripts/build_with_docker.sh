#!/usr/bin/env bash
# Wrapper to run multi-platform build script
SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
exec "${SCRIPT_DIR}/build_all_binaries.sh" "$@"
