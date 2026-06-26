#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
trigger_eval_file="$repo_root/evals/trigger-eval.json"
output_eval_file="$repo_root/evals/output-eval.json"

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

[ -f "$trigger_eval_file" ] || fail 'missing evals/trigger-eval.json'
[ -f "$output_eval_file" ] || fail 'missing evals/output-eval.json'
command -v jq >/dev/null 2>&1 || fail 'jq is required to run eval contract tests'

jq empty "$trigger_eval_file" >/dev/null \
  || fail 'trigger-eval.json must be valid JSON'
jq empty "$output_eval_file" >/dev/null \
  || fail 'output-eval.json must be valid JSON'

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
  "Review Manifest",
  "Split Suggestions",
  "Group Review Result",
  "Coverage Validation",
  "Reducer Finalization"
]'

jq -e --argjson required "$required_full_review_terms" \
  '(.cases[] | select(.scenario == "full-review-split-reducer") | .expected.must_include) as $terms | ($terms != null and all($required[]; . as $term | ($terms | index($term) != null)))' \
  "$output_eval_file" >/dev/null \
  || fail 'full-review split/reducer scenario missing required terms'

assert_jq "$output_eval_file" \
  'all(.cases[] | select(.scenario == "large-generated"); all(.expected.must_include[]?; contains("Partial review") | not))' \
  'large-generated scenario must not expect default partial review'

printf 'eval contract tests passed\n'
