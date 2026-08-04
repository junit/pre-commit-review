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
grep -Fq 'uses: actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1' <<<"$job" \
  || fail 'coverage job does not use the pinned Python setup action'
grep -Fq "python-version: '3.13'" <<<"$job" \
  || fail 'coverage job does not pin Python 3.13'
grep -Fq "python3 -m pip install --disable-pip-version-check 'jsonschema==4.25.1'" <<<"$job" \
  || fail 'coverage job does not install pinned jsonschema 4.25.1'
grep -Fq "python3 -c 'import jsonschema; from referencing import Registry, Resource'" <<<"$job" \
  || fail 'coverage job does not verify the schema validator imports'
grep -Fq 'source <(cargo +1.95.0 llvm-cov show-env --sh)' <<<"$job" \
  || fail 'coverage job does not export LLVM coverage instrumentation'
grep -Fq 'cargo +1.95.0 llvm-cov clean --workspace' <<<"$job" \
  || fail 'coverage job does not clean stale profiles'
grep -Fq 'cargo +1.95.0 test --release --locked' <<<"$job" \
  || fail 'coverage job does not run instrumented Rust tests'
grep -Fq 'cargo +1.95.0 build --release --locked --bins' <<<"$job" \
  || fail 'coverage job does not build instrumented production binaries'
grep -Fq './scripts/fetch_gitleaks.sh --platform linux-amd64' <<<"$job" \
  || fail 'coverage job does not provision the pinned secret scanner'
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
