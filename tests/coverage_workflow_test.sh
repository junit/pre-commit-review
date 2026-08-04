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
grep -Fq 'source <(cargo +1.95.0 llvm-cov show-env --sh)' <<<"$job" \
  || fail 'coverage job does not export LLVM coverage instrumentation'
grep -Fq 'cargo +1.95.0 llvm-cov clean --workspace' <<<"$job" \
  || fail 'coverage job does not clean stale profiles'
grep -Fq 'cargo +1.95.0 test --release --locked' <<<"$job" \
  || fail 'coverage job does not run instrumented Rust tests'
grep -Fq 'cargo +1.95.0 build --release --locked --bins' <<<"$job" \
  || fail 'coverage job does not build instrumented production binaries'
for integration_test in \
  collect_diff_context_test.sh \
  secret_gate_test.sh \
  static_analysis_execution_test.sh \
  static_analysis_orchestration_test.sh \
  repository_index_test.sh \
  repository_context_test.sh; do
  grep -Fq "./tests/$integration_test" <<<"$job" \
    || fail "coverage job does not run $integration_test"
done
grep -Fq 'cargo +1.95.0 llvm-cov report --release --fail-under-lines 80' <<<"$job" \
  || fail 'coverage job does not enforce 80 percent line coverage'
if grep -Fq 'continue-on-error: true' <<<"$job"; then
  fail 'coverage job is non-blocking'
fi

printf 'coverage workflow test passed\n'
