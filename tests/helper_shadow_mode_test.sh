#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
source_helper="$repo_root/scripts/collect_diff_context.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'helper shadow mode test failed: %s\n' "$*" >&2
  exit 1
}

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) os_name="darwin" ;;
  linux) os_name="linux" ;;
  msys*|mingw*|cygwin*) os_name="windows" ;;
  *) os_name="linux" ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch_name="amd64" ;;
  arm64|aarch64) arch_name="arm64" ;;
  *) arch_name="amd64" ;;
esac

binary_name="collect_diff_context-${os_name}-${arch_name}"
if [ "$os_name" = "windows" ]; then
  binary_name="${binary_name}.exe"
fi

helper_root="$tmp_dir/helper"
mkdir -p "$helper_root/bin"
cp "$source_helper" "$helper_root/collect_diff_context.sh"

legacy_hit="$tmp_dir/legacy-hit"
rust_hit="$tmp_dir/rust-hit"

cat >"$helper_root/collect_diff_context.legacy.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf 'legacy\n'
printf 'legacy\n' >"$legacy_hit"
EOF
chmod +x "$helper_root/collect_diff_context.legacy.sh"

cat >"$helper_root/bin/$binary_name" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf 'rust\n'
printf 'rust\n' >"$rust_hit"
EOF
chmod +x "$helper_root/bin/$binary_name"

stdout_file="$tmp_dir/stdout.txt"
stderr_file="$tmp_dir/stderr.txt"
(
  cd "$tmp_dir"
  PRE_COMMIT_REVIEW_HELPER_IMPL=legacy PRE_COMMIT_REVIEW_SHADOW_MODE=1 \
    "$helper_root/collect_diff_context.sh"
) >"$stdout_file" 2>"$stderr_file"

grep -Fxq 'legacy' "$stdout_file" \
  || fail 'shadow mode should still return legacy stdout for safety'
[ -f "$legacy_hit" ] \
  || fail 'shadow mode should execute the legacy helper'
[ -f "$rust_hit" ] \
  || fail 'PRE_COMMIT_REVIEW_SHADOW_MODE=1 should force shadow execution even when helper impl is legacy'
grep -Fq 'Output mismatch detected' "$stderr_file" \
  || fail 'shadow mode should report mismatched shadow output to stderr'

printf 'helper shadow mode tests passed\n'
