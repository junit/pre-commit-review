#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
skill_file="$repo_root/SKILL.md"
output_examples_file="$repo_root/references/output-examples.md"
visual_output_file="$repo_root/references/visual-output.md"
readme_file="$repo_root/README.md"
readme_zh_file="$repo_root/README.zh-CN.md"

fail() {
  printf 'skill contract test failed: %s\n' "$*" >&2
  exit 1
}

if grep -q '<localized' "$skill_file"; then
  fail 'SKILL.md must use concrete output templates, not <localized ...> placeholders'
fi
if grep -q '<localized' "$visual_output_file"; then
  fail 'visual-output.md must use concrete visual templates, not <localized ...> placeholders'
fi

grep -Fq 'The field label `VERDICT` must remain exactly `VERDICT`.' "$skill_file" \
  || fail 'SKILL.md must explicitly forbid translating the VERDICT field label'

verdict_template_count="$(
  grep -F '**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>' "$skill_file" | wc -l | tr -d ' '
)"
[ "$verdict_template_count" -ge 2 ] \
  || fail 'SKILL.md must show concrete verdict lines in both English and Chinese templates'

grep -Fq '#### English Default Developer Review' "$skill_file" \
  || fail 'SKILL.md must include a concrete English default template'
grep -Fq '#### Chinese Default Developer Review' "$skill_file" \
  || fail 'SKILL.md must include a concrete Chinese default template'

grep -Fq 'git diff --cached -- path/to/file' "$skill_file" \
  || fail 'SKILL.md must tell reviewers to use staged file-specific diffs for staged reviews'
grep -Fq 'git diff <base>...HEAD -- path/to/file' "$skill_file" \
  || fail 'SKILL.md must tell reviewers to use branch file-specific diffs for branch-vs-base reviews'
grep -Fq 'After the title, put the verdict first' "$skill_file" \
  || fail 'SKILL.md must align the verdict-first rule with the titled output templates'

grep -Fq '## Chinese Tiny Diff Example' "$output_examples_file" \
  || fail 'output-examples.md must include a Chinese tiny diff example'
grep -Fq '## Chinese Partial Review Example' "$output_examples_file" \
  || fail 'output-examples.md must include a Chinese partial review example'
grep -Fq 'SKILL.md is authoritative; examples illustrate valid outputs only.' "$output_examples_file" \
  || fail 'output-examples.md must state that SKILL.md is authoritative'
if grep -Eq '^\*\*(结论|裁定|状态|判定)[^*]*\*\*:?[[:space:]]*(SAFE_TO_COMMIT|SAFE_TO_COMMIT_WITH_NOTES|DO_NOT_COMMIT)' "$output_examples_file"; then
  fail 'output-examples.md must not show verdict tokens under translated verdict-like labels'
fi

grep -Fq '## Visual Review Skeleton' "$visual_output_file" \
  || fail 'visual-output.md must include a complete visual review skeleton'
grep -Fq 'Follow the selected output language from `SKILL.md`.' "$visual_output_file" \
  || fail 'visual-output.md must preserve the SKILL.md localization contract'
grep -Fq 'Only calculate distribution from real `name-status`, `numstat`, or reviewed file counts.' "$visual_output_file" \
  || fail 'visual-output.md must prohibit invented change distribution percentages'

grep -Fq 'skill_contract_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include skill_contract_test.sh'
grep -Fq 'collect_diff_context_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include collect_diff_context_test.sh'
grep -Fq 'skill_contract_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include skill_contract_test.sh'
grep -Fq 'collect_diff_context_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include collect_diff_context_test.sh'

printf 'skill contract tests passed\n'
