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
grep -Fq 'MUST use the Tiny Diff format' "$skill_file" \
  || fail 'SKILL.md must make tiny diff formatting a hard rule for tiny low-risk diffs'
grep -Fq 'Visual mode is justified only when' "$skill_file" \
  || fail 'SKILL.md must define a concrete visual mode threshold'
grep -Fq '300+ changed lines' "$skill_file" \
  || fail 'SKILL.md visual mode threshold must reuse the large-diff line-count signal'
grep -Fq '10+ changed files excluding generated, vendored, minified, and lockfile-only files' "$skill_file" \
  || fail 'SKILL.md must define meaningful files in measurable terms'
if grep -Fq '10+ meaningful files' "$skill_file"; then
  fail 'SKILL.md must not use the ambiguous phrase "meaningful files"'
fi
grep -Fq 'Append supporting analysis only when it adds decision value' "$skill_file" \
  || fail 'SKILL.md must make Supporting Analysis optional and decision-value gated'
if grep -Fq '## Supporting Analysis' "$skill_file"; then
  fail 'SKILL.md default templates must not include Supporting Analysis by default'
fi
if grep -Fq '## 补充分析' "$skill_file"; then
  fail 'SKILL.md default templates must not include Chinese Supporting Analysis by default'
fi
if grep -Fq '**变更规模：** <count> files' "$skill_file"; then
  fail 'Chinese default template must not use English count/files placeholders'
fi
if grep -Fq '**未审查变更：** <无 | unstaged/generated/too-large files' "$skill_file"; then
  fail 'Chinese default template must not use English unreviewed-change placeholders'
fi
if grep -Fq '**变更规模：** <files and lines>' "$skill_file"; then
  fail 'Chinese tiny template must not use English files/lines placeholders'
fi
grep -Fq '**变更规模：** <文件数> 个文件, +<新增行数> 行 / -<删除行数> 行' "$skill_file" \
  || fail 'Chinese templates must show localized line-count units'
if grep -Fq '**差异来源：** <来源>' "$skill_file"; then
  fail 'Chinese Tiny Diff template must use the same diff-source placeholder style as the default template'
fi
if grep -Fq '**审查范围：** <完整 | 部分>' "$skill_file"; then
  fail 'Chinese Tiny Diff template must use explicit review-scope placeholder wording'
fi
chinese_tiny_template="$(
  awk '
    /#### Chinese Tiny Diff Review/ { in_section=1 }
    in_section { print }
    in_section && /### Full Visual Mode/ { exit }
  ' "$skill_file"
)"
for label in '**结论：**' '**差异来源：**' '**审查范围：**' '**变更规模：**' '- **变更：**' '- **代码卫生：**' '- **逻辑：**' '- **影响范围：**' '- **风险：**' '- **测试：**'; do
  printf '%s\n' "$chinese_tiny_template" | grep -Fq -- "$label" \
    || fail "Chinese Tiny Diff template missing localized label: $label"
done

grep -Fq '## Chinese Tiny Diff Example' "$output_examples_file" \
  || fail 'output-examples.md must include a Chinese tiny diff example'
grep -Fq '## Chinese Partial Review Example' "$output_examples_file" \
  || fail 'output-examples.md must include a Chinese partial review example'
grep -Fq 'SKILL.md is authoritative; examples illustrate valid outputs only.' "$output_examples_file" \
  || fail 'output-examples.md must state that SKILL.md is authoritative'
if grep -Eq '^\*\*(结论|裁定|状态|判定)[^*]*\*\*:?[[:space:]]*(SAFE_TO_COMMIT|SAFE_TO_COMMIT_WITH_NOTES|DO_NOT_COMMIT)' "$output_examples_file"; then
  fail 'output-examples.md must not show verdict tokens under translated verdict-like labels'
fi
if grep -Fq '**差异来源：** staged diff' "$output_examples_file"; then
  fail 'Chinese examples must not use English diff-source labels as prose values'
fi
if grep -Eq '^\*\*变更规模：\*\* [0-9]+ files' "$output_examples_file"; then
  fail 'Chinese examples must not use English files counts'
fi
if grep -Eq '^\*\*变更规模：\*\* .* \+[0-9]+ / -[0-9]+' "$output_examples_file"; then
  fail 'Chinese examples must include localized line-count units'
fi

grep -Fq '## Visual Review Skeleton' "$visual_output_file" \
  || fail 'visual-output.md must include a complete visual review skeleton'
grep -Fq 'Follow the selected output language from `SKILL.md`.' "$visual_output_file" \
  || fail 'visual-output.md must preserve the SKILL.md localization contract'
grep -Fq 'Only calculate distribution from real `name-status`, `numstat`, or reviewed file counts.' "$visual_output_file" \
  || fail 'visual-output.md must prohibit invented change distribution percentages'
if grep -Fq '**变更规模：** <files and lines>' "$visual_output_file"; then
  fail 'Chinese visual skeleton must not use English files/lines placeholders'
fi

grep -Fq 'skill_contract_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include skill_contract_test.sh'
grep -Fq 'collect_diff_context_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include collect_diff_context_test.sh'
grep -Fq 'eval_contract_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include eval_contract_test.sh'
grep -Fq 'install_agent_matrix_test.sh' "$readme_file" \
  || fail 'README.md repository tree must include install_agent_matrix_test.sh'
grep -Fq 'trigger-eval.json' "$readme_file" \
  || fail 'README.md repository tree must include trigger-eval.json'
grep -Fq 'output-eval.json' "$readme_file" \
  || fail 'README.md repository tree must include output-eval.json'
grep -Fq 'skill_contract_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include skill_contract_test.sh'
grep -Fq 'collect_diff_context_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include collect_diff_context_test.sh'
grep -Fq 'eval_contract_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include eval_contract_test.sh'
grep -Fq 'install_agent_matrix_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include install_agent_matrix_test.sh'
grep -Fq 'trigger-eval.json' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include trigger-eval.json'
grep -Fq 'output-eval.json' "$readme_zh_file" \
  || fail 'README.zh-CN.md repository tree must include output-eval.json'

printf 'skill contract tests passed\n'
