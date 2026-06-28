#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
readme_en="$repo_root/README.md"
readme_zh="$repo_root/README.zh-CN.md"

fail() {
  printf 'readme host entrypoints test failed: %s\n' "$*" >&2
  exit 1
}

assert_readme() {
  local file="$1"
  local heading="$2"

  grep -Fq "$heading" "$file" \
    || fail "missing host entrypoints heading in $file"
  grep -Fq 'run_host_readiness_pipeline.sh' "$file" \
    || fail "missing primary entrypoint in $file"
  grep -Fq "\`Analysis\`" "$file" \
    || fail "missing analysis tier in $file"
  grep -Fq 'run_cross_host_readiness.sh' "$file" \
    || fail "missing cross-host analysis entrypoint in $file"
  grep -Fq 'analyze_host_readiness_diff.sh' "$file" \
    || fail "missing readiness diff analysis entrypoint in $file"
  grep -Fq 'run_real_host_smoke.sh' "$file" \
    || fail "missing real-host smoke entrypoint in $file"
  grep -Fq 'real-host-smoke.yml' "$file" \
    || fail "missing real-host smoke workflow in $file"
  grep -Fq 'check_host_availability.sh' "$file" \
    || fail "missing availability stage entrypoint in $file"
  grep -Fq 'run_helper_gateway_probe.sh' "$file" \
    || fail "missing helper gateway probe stage entrypoint in $file"
  grep -Fq 'run_layered_host_evals.sh' "$file" \
    || fail "missing layered stage entrypoint in $file"
  grep -Fq 'host_contract_subset.sh' "$file" \
    || fail "missing host contract subset stage entrypoint in $file"
  grep -Fq 'eval_contract_test.sh' "$file" \
    || fail "missing repo-wide contract surface in $file"
  grep -Fq 'host_failure_taxonomy.sh' "$file" \
    || fail "missing internal taxonomy helper in $file"
}

assert_readme "$readme_en" '### Host Entrypoints'
assert_readme "$readme_zh" '### Host Entrypoints'

printf 'readme host entrypoints tests passed\n'
