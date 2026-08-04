#!/usr/bin/env bash
# shellcheck disable=SC2016
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
  local graph_description="$4"
  local compiler_limit="$5"
  local fast_write_policy="$6"
  local deep_write_policy="$7"

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
  grep -Fq 'collect_static_evidence.sh' "$file" \
    || fail "missing static evidence collector surface in $file"
  grep -Fq 'static-analysis-evidence.md' "$file" \
    || fail "missing static analysis evidence documentation surface in $file"
  grep -Fq 'run_static_analysis.sh' "$file" \
    || fail "missing controlled static-analysis runner surface in $file"
  grep -Fq 'static-analysis-execution.md' "$file" \
    || fail "missing controlled static-analysis documentation surface in $file"
  for command in \
    'repository-context-cli index build' \
    'repository-context-cli index doctor' \
    'repository-context-cli index inspect' \
    'repository-context-cli index clean'; do
    grep -Fq "$command" "$file" \
      || fail "missing repository index command '$command' in $file"
  done
  grep -Fq "$graph_description" "$file" \
    || fail "missing heuristic repository graph description in $file"
  grep -Fq "$compiler_limit" "$file" \
    || fail "missing compiler-completeness limitation in $file"
  grep -Fq "$fast_write_policy" "$file" \
    || fail "missing Fast Mode zero-write policy in $file"
  grep -Fq "$deep_write_policy" "$file" \
    || fail "missing explicit Deep/index write policy in $file"
}

assert_readme_surface \
  "$readme_en" \
  '## Repository Structure' \
  '### `evals/`' \
  'heuristic repository graph' \
  'not compiler-complete' \
  'Fast Mode performs zero persistent writes' \
  'Deep/index operations write cache only when explicitly invoked'
assert_readme_surface \
  "$readme_zh" \
  '## 仓库结构' \
  '### `evals/`' \
  '启发式全仓图谱' \
  '并非编译器完备' \
  'Fast Mode 零持久化写入' \
  'Deep/index 仅在显式调用时写入缓存'

printf 'readme surface tests passed\n'
