#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
trigger_eval_file="$repo_root/evals/trigger-eval.json"
output_eval_file="$repo_root/evals/output-eval.json"
routine_output_eval_file="$repo_root/evals/output/routine-output-eval.json"
advanced_output_eval_file="$repo_root/evals/output/advanced-output-eval.json"
visual_output_eval_file="$repo_root/evals/output/visual-output-eval.json"
localization_output_eval_file="$repo_root/evals/output/localization-output-eval.json"
marker_eval_file="$repo_root/evals/taxonomy/marker-eval.json"
layered_output_runner="$repo_root/evals/run_layered_output_evals.sh"
layered_output_runner_test="$repo_root/evals/run_layered_output_evals_test.sh"
marker_eval_checker="$repo_root/evals/run_marker_eval_checks.sh"
marker_eval_checker_test="$repo_root/evals/run_marker_eval_checks_test.sh"
layered_host_runner="$repo_root/evals/run_layered_host_evals.sh"
layered_host_runner_test="$repo_root/evals/run_layered_host_evals_test.sh"
helper_gateway_probe="$repo_root/evals/run_helper_gateway_probe.sh"
helper_gateway_probe_test="$repo_root/evals/run_helper_gateway_probe_test.sh"
host_availability_gate="$repo_root/evals/check_host_availability.sh"
host_availability_gate_test="$repo_root/evals/check_host_availability_test.sh"
host_contract_subset="$repo_root/evals/host_contract_subset.sh"
host_contract_subset_test="$repo_root/evals/host_contract_subset_test.sh"
readme_surface_test="$repo_root/evals/readme_surface_test.sh"
readme_host_entrypoints_test="$repo_root/evals/readme_host_entrypoints_test.sh"
real_host_smoke_runner="$repo_root/evals/run_real_host_smoke.sh"
real_host_smoke_runner_test="$repo_root/evals/run_real_host_smoke_test.sh"
cross_host_readiness_runner="$repo_root/evals/run_cross_host_readiness.sh"
cross_host_readiness_runner_test="$repo_root/evals/run_cross_host_readiness_test.sh"
host_readiness_diff_analyzer="$repo_root/evals/analyze_host_readiness_diff.sh"
host_readiness_diff_analyzer_test="$repo_root/evals/analyze_host_readiness_diff_test.sh"
host_readiness_pipeline="$repo_root/evals/run_host_readiness_pipeline.sh"
host_readiness_pipeline_test="$repo_root/evals/run_host_readiness_pipeline_test.sh"
host_failure_taxonomy_helper="$repo_root/evals/host_failure_taxonomy.sh"
host_failure_taxonomy_test="$repo_root/evals/host_failure_taxonomy_test.sh"
real_host_smoke_workflow="$repo_root/.github/workflows/real-host-smoke.yml"

fail() {
  printf 'eval contract test failed: %s\n' "$*" >&2
  exit 1
}

assert_jq() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

assert_contains() {
  local file="$1"
  local needle="$2"
  local message="$3"

  grep -Fq "$needle" "$file" || fail "$message"
}

[ -f "$trigger_eval_file" ] || fail 'missing evals/trigger-eval.json'
[ -f "$output_eval_file" ] || fail 'missing evals/output-eval.json'
[ -f "$routine_output_eval_file" ] || fail 'missing evals/output/routine-output-eval.json'
[ -f "$advanced_output_eval_file" ] || fail 'missing evals/output/advanced-output-eval.json'
[ -f "$visual_output_eval_file" ] || fail 'missing evals/output/visual-output-eval.json'
[ -f "$localization_output_eval_file" ] || fail 'missing evals/output/localization-output-eval.json'
[ -f "$marker_eval_file" ] || fail 'missing evals/taxonomy/marker-eval.json'
[ -f "$layered_output_runner" ] || fail 'missing evals/run_layered_output_evals.sh'
[ -f "$layered_output_runner_test" ] || fail 'missing evals/run_layered_output_evals_test.sh'
[ -f "$marker_eval_checker" ] || fail 'missing evals/run_marker_eval_checks.sh'
[ -f "$marker_eval_checker_test" ] || fail 'missing evals/run_marker_eval_checks_test.sh'
[ -f "$layered_host_runner" ] || fail 'missing evals/run_layered_host_evals.sh'
[ -f "$layered_host_runner_test" ] || fail 'missing evals/run_layered_host_evals_test.sh'
[ -f "$helper_gateway_probe" ] || fail 'missing evals/run_helper_gateway_probe.sh'
[ -f "$helper_gateway_probe_test" ] || fail 'missing evals/run_helper_gateway_probe_test.sh'
[ -f "$host_availability_gate" ] || fail 'missing evals/check_host_availability.sh'
[ -f "$host_availability_gate_test" ] || fail 'missing evals/check_host_availability_test.sh'
[ -f "$host_contract_subset" ] || fail 'missing evals/host_contract_subset.sh'
[ -f "$host_contract_subset_test" ] || fail 'missing evals/host_contract_subset_test.sh'
[ -f "$readme_surface_test" ] || fail 'missing evals/readme_surface_test.sh'
[ -f "$readme_host_entrypoints_test" ] || fail 'missing evals/readme_host_entrypoints_test.sh'
[ -f "$real_host_smoke_runner" ] || fail 'missing evals/run_real_host_smoke.sh'
[ -f "$real_host_smoke_runner_test" ] || fail 'missing evals/run_real_host_smoke_test.sh'
[ -f "$cross_host_readiness_runner" ] || fail 'missing evals/run_cross_host_readiness.sh'
[ -f "$cross_host_readiness_runner_test" ] || fail 'missing evals/run_cross_host_readiness_test.sh'
[ -f "$host_readiness_diff_analyzer" ] || fail 'missing evals/analyze_host_readiness_diff.sh'
[ -f "$host_readiness_diff_analyzer_test" ] || fail 'missing evals/analyze_host_readiness_diff_test.sh'
[ -f "$host_readiness_pipeline" ] || fail 'missing evals/run_host_readiness_pipeline.sh'
[ -f "$host_readiness_pipeline_test" ] || fail 'missing evals/run_host_readiness_pipeline_test.sh'
[ -f "$host_failure_taxonomy_helper" ] || fail 'missing evals/host_failure_taxonomy.sh'
[ -f "$host_failure_taxonomy_test" ] || fail 'missing evals/host_failure_taxonomy_test.sh'
[ -f "$real_host_smoke_workflow" ] || fail 'missing .github/workflows/real-host-smoke.yml'
command -v jq >/dev/null 2>&1 || fail 'jq is required to run eval contract tests'

jq empty "$trigger_eval_file" >/dev/null \
  || fail 'trigger-eval.json must be valid JSON'
jq empty "$output_eval_file" >/dev/null \
  || fail 'output-eval.json must be valid JSON'
jq empty "$routine_output_eval_file" >/dev/null \
  || fail 'routine-output-eval.json must be valid JSON'
jq empty "$advanced_output_eval_file" >/dev/null \
  || fail 'advanced-output-eval.json must be valid JSON'
jq empty "$visual_output_eval_file" >/dev/null \
  || fail 'visual-output-eval.json must be valid JSON'
jq empty "$localization_output_eval_file" >/dev/null \
  || fail 'localization-output-eval.json must be valid JSON'
jq empty "$marker_eval_file" >/dev/null \
  || fail 'marker-eval.json must be valid JSON'

assert_jq "$trigger_eval_file" \
  '.cases | type == "array" and length >= 8' \
  'trigger-eval.json must contain at least 8 cases'

assert_jq "$trigger_eval_file" \
  '([.cases[] | select(.expected_trigger == true)] | length >= 4) and ([.cases[] | select(.expected_trigger == false)] | length >= 4)' \
  'trigger-eval.json must contain at least 4 positive and 4 negative cases'

assert_jq "$trigger_eval_file" \
  'all(.cases[]; has("id") and has("prompt") and has("expected_trigger") and has("rationale"))' \
  'trigger case missing required fields'

assert_jq "$trigger_eval_file" \
  'any(.cases[] | select(.expected_trigger == true); (.prompt // "") | contains("提交"))' \
  'trigger positives must include a Chinese commit-readiness prompt'

assert_jq "$trigger_eval_file" \
  'any(.cases[] | select(.expected_trigger == true); (.prompt // "" | ascii_downcase | contains("staged")))' \
  'trigger positives must include a staged-changes prompt'

assert_jq "$trigger_eval_file" \
  'any(.cases[] | select(.expected_trigger == false); (.prompt // "" | ascii_downcase | contains("debug")))' \
  'trigger negatives must include a debugging prompt'

assert_jq "$trigger_eval_file" \
  'any(.cases[] | select(.expected_trigger == false); (.prompt // "" | ascii_downcase | contains("function")))' \
  'trigger negatives must include a single-function review prompt'

assert_jq "$output_eval_file" \
  '.cases | type == "array" and length >= 8' \
  'output-eval.json must contain at least 8 cases'

required_scenarios='[
  "tiny-docs",
  "mixed-staged-unstaged",
  "hardcoded-secret",
  "breaking-api",
  "large-generated",
  "full-review-split-reducer",
  "no-git-repo",
  "chinese-request",
  "pasted-diff"
]'

jq -e --argjson required "$required_scenarios" \
  '(.cases | map(.scenario)) as $seen | all($required[]; . as $scenario | ($seen | index($scenario) != null))' \
  "$output_eval_file" >/dev/null \
  || fail 'output-eval.json missing required scenarios'

assert_jq "$output_eval_file" \
  'all(.cases[]; has("id") and has("scenario") and has("prompt") and has("locale") and has("expected"))' \
  'output case missing required fields'

assert_jq "$output_eval_file" \
  'all(.cases[]; (.expected | type == "object" and has("verdict") and has("must_include")))' \
  'output case expected block is incomplete'

required_full_review_terms='[
  "coverage-led",
  "split",
  "DO_NOT_COMMIT",
  "drop",
  "compatibility"
]'

jq -e --argjson required "$required_full_review_terms" \
  '(.cases[] | select(.scenario == "full-review-split-reducer") | .expected.must_include) as $terms | ($terms != null and all($required[]; . as $term | ($terms | index($term) != null)))' \
  "$output_eval_file" >/dev/null \
  || fail 'full-review split/reducer scenario missing updated coverage-led required terms'

assert_jq "$output_eval_file" \
  'all(.cases[] | select(.scenario == "large-generated"); all(.expected.must_include[]?; contains("Partial review") | not))' \
  'large-generated scenario must not expect default partial review'

for layered_output_eval in \
  "$routine_output_eval_file" \
  "$advanced_output_eval_file" \
  "$visual_output_eval_file" \
  "$localization_output_eval_file"; do
  assert_jq "$layered_output_eval" \
    'has("evaluation_method") and has("cases") and (.cases | type == "array" and length >= 1)' \
    "$(basename "$layered_output_eval") must define evaluation_method and at least one case"
  assert_jq "$layered_output_eval" \
    'all(.cases[]; has("id") and has("scenario") and has("prompt") and has("locale") and has("expected"))' \
    "$(basename "$layered_output_eval") contains a case missing required fields"
  assert_jq "$layered_output_eval" \
    'all(.cases[]; .expected | type == "object" and has("verdict") and has("template") and has("scope") and has("must_include") and has("must_not_include"))' \
    "$(basename "$layered_output_eval") contains an incomplete expected block"
done

assert_jq "$routine_output_eval_file" \
  '(.cases | map(.scenario)) as $seen | ($seen | index("tiny-docs") != null) and ($seen | index("mixed-staged-unstaged") != null) and ($seen | index("breaking-api") != null) and ($seen | index("hardcoded-secret") != null) and ($seen | index("chinese-request") != null)' \
  'routine-output-eval.json must cover routine tiny, mixed state, breaking API, secret, and Chinese scenarios'

assert_jq "$routine_output_eval_file" \
  'any(.cases[]; .id == "routine-chinese-request-zh" and (.expected.must_include | index("差异来源") != null) and (.expected.must_not_include | index("**Diff source:**") != null))' \
  'routine-output-eval.json must include a Chinese localization routine case'

assert_jq "$advanced_output_eval_file" \
  '(.cases | map(.scenario)) as $seen | ($seen | index("large-generated") != null) and ($seen | index("full-review-split-reducer") != null) and ($seen | index("no-git-repo") != null)' \
  'advanced-output-eval.json must cover generated, split/reducer, and advisory fallback scenarios'

assert_jq "$advanced_output_eval_file" \
  'any(.cases[]; .id == "advanced-advisory-fallback-en" and .expected.template == "advisory" and (.expected.must_not_include | index("**VERDICT:** SAFE_TO_COMMIT") != null))' \
  'advanced-output-eval.json must ensure advisory fallback does not masquerade as a normal verdict-bearing review'

assert_jq "$visual_output_eval_file" \
  '.cases | type == "array" and length >= 5' \
  'visual-output-eval.json must contain at least 5 visual cases'

assert_jq "$visual_output_eval_file" \
  'all(.cases[]; .expected.template == "visual")' \
  'visual-output-eval.json must require the visual template for every case'

assert_jq "$visual_output_eval_file" \
  'any(.cases[]; .locale == "zh-CN" and (.expected.must_include | index("视觉审查矩阵") != null))' \
  'visual-output-eval.json must include a Chinese visual matrix case'

assert_jq "$localization_output_eval_file" \
  '.cases | type == "array" and length >= 6' \
  'localization-output-eval.json must contain at least 6 localization cases'

assert_jq "$localization_output_eval_file" \
  '([.cases[] | select(.locale == "en")] | length >= 3) and ([.cases[] | select(.locale == "zh-CN")] | length >= 3)' \
  'localization-output-eval.json must include both English and Chinese cases'

assert_jq "$localization_output_eval_file" \
  'all(.cases[]; any(.expected.must_include[]?; contains("**VERDICT:**")))' \
  'localization-output-eval.json must keep VERDICT token requirements explicit in every case'

assert_jq "$marker_eval_file" \
  'has("evaluation_method") and has("cases") and (.cases | type == "array" and length >= 7)' \
  'marker-eval.json must define evaluation_method and at least 7 marker cases'

assert_jq "$marker_eval_file" \
  'all(.cases[]; has("id") and has("scenario") and has("prompt") and has("locale") and has("expected"))' \
  'marker-eval.json contains a case missing required fields'

assert_jq "$marker_eval_file" \
  'all(.cases[]; .expected | has("verdict") and has("expected_primary_marker") and has("expected_blocking") and has("expected_tally") and has("must_include"))' \
  'marker-eval.json contains an incomplete expected taxonomy block'

required_markers='["🔒","❌","⚠️","🧪","👁️","📈","🧭"]'

jq -e --argjson required "$required_markers" \
  '(.cases | map(.expected.expected_primary_marker)) as $seen | all($required[]; . as $marker | ($seen | index($marker) != null))' \
  "$marker_eval_file" >/dev/null \
  || fail 'marker-eval.json must cover every primary marker'

assert_contains "$real_host_smoke_workflow" 'workflow_dispatch:' \
  'real-host-smoke.yml must be manually dispatchable'
assert_contains "$real_host_smoke_workflow" 'run_real_host_smoke.sh' \
  'real-host-smoke.yml must invoke evals/run_real_host_smoke.sh'

printf 'eval contract tests passed\n'
