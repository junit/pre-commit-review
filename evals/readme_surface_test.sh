#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
readme_en="$repo_root/README.md"
readme_zh="$repo_root/README.zh-CN.md"

fail() {
  printf 'readme surface test failed: %s\n' "$*" >&2
  exit 1
}

assert_readme_surface() {
  local file="$1"
  local structure_heading="$2"
  local evals_heading="$3"

  grep -Fq "$structure_heading" "$file" \
    || fail "missing repository structure heading in $file"
  grep -Fq 'skill_contract_test.sh' "$file" \
    || fail "missing skill contract surface in $file"
  grep -Fq 'eval_contract_test.sh' "$file" \
    || fail "missing eval contract surface in $file"
  grep -Fq 'readme_surface_test.sh' "$file" \
    || fail "missing README surface test in $file"
  grep -Fq 'readme_host_entrypoints_test.sh' "$file" \
    || fail "missing README host entrypoints surface in $file"
  grep -Fq "$evals_heading" "$file" \
    || fail "missing evals heading in $file"
}

assert_readme_surface "$readme_en" '## Repository Structure' '### `evals/`'
assert_readme_surface "$readme_zh" '## 仓库结构' '### `evals/`'

printf 'readme surface tests passed\n'
