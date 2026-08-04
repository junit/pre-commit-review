#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
workflow="$repo_root/.github/workflows/lint.yml"

fail() {
  printf 'coverage workflow test failed: %s\n' "$*" >&2
  exit 1
}

job="$({
  awk '
    /^  rust-coverage:/ { in_job = 1 }
    in_job && /^  [[:alnum:]_-]+:/ && $0 !~ /^  rust-coverage:/ { exit }
    in_job { print }
  ' "$workflow"
})"

[ -n "$job" ] || fail 'lint workflow does not define rust-coverage'
grep -Fq 'toolchain: 1.95.0' <<<"$job" \
  || fail 'coverage job does not pin Rust 1.95.0'
grep -Fq 'components: llvm-tools-preview' <<<"$job" \
  || fail 'coverage job does not install llvm-tools-preview'
grep -Fq 'cargo +1.95.0 install --locked --version 0.8.7 cargo-llvm-cov' <<<"$job" \
  || fail 'coverage job does not install pinned cargo-llvm-cov 0.8.7'
grep -Fq 'cargo +1.95.0 llvm-cov --locked --all-features --fail-under-lines 80' <<<"$job" \
  || fail 'coverage job does not enforce 80 percent line coverage'
if grep -Fq 'continue-on-error: true' <<<"$job"; then
  fail 'coverage job is non-blocking'
fi

printf 'coverage workflow test passed\n'
