#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

skill_file="$repo_root/SKILL.md"

decision_verdict_file="$repo_root/references/decision/verdict-rules.md"
decision_risk_file="$repo_root/references/decision/risk-taxonomy.md"

render_output_en_file="$repo_root/references/rendering/output-en.md"
render_output_zh_file="$repo_root/references/rendering/output-zh.md"
render_visual_file="$repo_root/references/rendering/visual-output.md"
render_meta_file="$repo_root/references/rendering/review-meta.md"

advanced_coverage_file="$repo_root/references/advanced/coverage-led-review.md"
advanced_visual_rules_file="$repo_root/references/advanced/visual-review-rules.md"
advanced_grading_file="$repo_root/references/advanced/grading-compat.md"

examples_en_file="$repo_root/references/examples/default-tiny-en.md"
examples_zh_file="$repo_root/references/examples/default-tiny-zh.md"
examples_complex_file="$repo_root/references/examples/complex-visual-and-coverage.md"

readme_file="$repo_root/README.md"
readme_zh_file="$repo_root/README.zh-CN.md"

fail() {
  printf 'skill contract test failed: %s\n' "$*" >&2
  exit 1
}

for required_file in \
  "$skill_file" \
  "$decision_verdict_file" \
  "$decision_risk_file" \
  "$render_output_en_file" \
  "$render_output_zh_file" \
  "$render_visual_file" \
  "$render_meta_file" \
  "$advanced_coverage_file" \
  "$advanced_visual_rules_file" \
  "$advanced_grading_file" \
  "$examples_en_file" \
  "$examples_zh_file" \
  "$examples_complex_file" \
  "$readme_file" \
  "$readme_zh_file"; do
  [ -f "$required_file" ] || fail "missing required file: $required_file"
done

for concrete_file in \
  "$skill_file" \
  "$render_output_en_file" \
  "$render_output_zh_file" \
  "$render_visual_file"; do
  if grep -q '<localized' "$concrete_file"; then
    fail "$(basename "$concrete_file") must use concrete content, not <localized ...> placeholders"
  fi
done

# SKILL.md must route through the new layered structure.
grep -Fq 'Always preserve the field label `VERDICT` in English.' "$skill_file" \
  || fail 'SKILL.md must keep the VERDICT localization rule'
grep -Fq 'references/decision/verdict-rules.md' "$skill_file" \
  || fail 'SKILL.md must route verdict logic to references/decision/verdict-rules.md'
grep -Fq 'references/decision/risk-taxonomy.md' "$skill_file" \
  || fail 'SKILL.md must route finding taxonomy to references/decision/risk-taxonomy.md'
grep -Fq 'references/rendering/output-en.md' "$skill_file" \
  || fail 'SKILL.md must route English output through references/rendering/output-en.md'
grep -Fq 'references/rendering/output-zh.md' "$skill_file" \
  || fail 'SKILL.md must route Chinese output through references/rendering/output-zh.md'
grep -Fq 'references/advanced/visual-review-rules.md' "$skill_file" \
  || fail 'SKILL.md must route visual review rules through references/advanced/visual-review-rules.md'
grep -Fq 'references/rendering/visual-output.md' "$skill_file" \
  || fail 'SKILL.md must route visual rendering through references/rendering/visual-output.md'
grep -Fq 'references/advanced/coverage-led-review.md' "$skill_file" \
  || fail 'SKILL.md must route coverage-led review through references/advanced/coverage-led-review.md'
grep -Fq 'references/advanced/grading-compat.md' "$skill_file" \
  || fail 'SKILL.md must route grading compatibility through references/advanced/grading-compat.md'
grep -Fq 'Examples are optional calibration aids only.' "$skill_file" \
  || fail 'SKILL.md must keep examples optional rather than required'
grep -Fq '### Local Repository Gateway' "$skill_file" \
  || fail 'SKILL.md must define the local repository helper gateway'
grep -Fq 'Resolve this path relative to the skill package directory containing this `SKILL.md`. Do not assume `scripts/` exists in the user'\''s project root.' "$skill_file" \
  || fail 'SKILL.md must disambiguate the helper path relative to the skill package'
grep -Fq 'This is a mandatory gateway. Attempt the helper before any direct `git status`, `git diff`, `git diff --cached`, or branch comparison command.' "$skill_file" \
  || fail 'SKILL.md must require the helper before direct Git inspection'
grep -Fq 'Only fall back to direct Git inspection when the helper is unavailable at that resolved path, exits non-zero, cannot be executed in the current host, or the user already provided the review material explicitly.' "$skill_file" \
  || fail 'SKILL.md must bound direct Git fallback to explicit helper-unavailable scenarios'
if grep -Fq 'prefer `scripts/collect_diff_context.sh`' "$skill_file"; then
  fail 'SKILL.md must not describe helper-first collection as a soft preference'
fi

if grep -Fq 'references/coverage-led-review.md' "$skill_file"; then
  fail 'SKILL.md must not reference the deprecated flat coverage-led-review path'
fi
if grep -Fq 'references/output-examples.md' "$skill_file"; then
  fail 'SKILL.md must not reference the deprecated flat output-examples path'
fi
if grep -Fq 'references/output-en.md' "$skill_file"; then
  fail 'SKILL.md must not reference the deprecated flat output-en path'
fi
if grep -Fq 'references/output-zh.md' "$skill_file"; then
  fail 'SKILL.md must not reference the deprecated flat output-zh path'
fi

# Decision layer must stay narrowly scoped.
grep -Fq 'Use exactly one top-level verdict token:' "$decision_verdict_file" \
  || fail 'verdict-rules.md must define the verdict token contract'
grep -Fq 'Each priority finding must use exactly one primary marker.' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must define primary finding markers'
grep -Fq 'redacted' "$advanced_grading_file" \
  || fail 'grading-compat.md must retain secret-handling compatibility wording'
grep -Fq 'downstream clients' "$advanced_grading_file" \
  || fail 'grading-compat.md must retain downstream-clients compatibility wording'
grep -Fq 'coverage-led' "$advanced_grading_file" \
  || fail 'grading-compat.md must retain coverage-led compatibility wording'

# Rendering layer must keep concrete templates and localized labels.
verdict_en_count="$(grep -Fc '**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>' "$render_output_en_file")"
verdict_zh_count="$(grep -Fc '**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>' "$render_output_zh_file")"
[ "$verdict_en_count" -ge 1 ] && [ "$verdict_zh_count" -ge 1 ] \
  || fail 'rendering output templates must each show concrete verdict lines'

grep -Fq '## Default Developer Review' "$render_output_en_file" \
  || fail 'output-en.md must include the English default template'
grep -Fq '## Default Developer Review' "$render_output_zh_file" \
  || fail 'output-zh.md must include the Chinese default template section heading'
grep -Fq '**Tally:**' "$render_output_en_file" \
  || fail 'English default template must include a Tally line'
grep -Fq '**统计：**' "$render_output_zh_file" \
  || fail 'Chinese default template must include a 统计 line'
grep -Fq '## Tiny Diff Review' "$render_output_en_file" \
  || fail 'output-en.md must include the tiny template'
grep -Fq '## Tiny Diff Review' "$render_output_zh_file" \
  || fail 'output-zh.md must include the tiny template section heading'
grep -Fq '## Visual Review' "$render_output_en_file" \
  || fail 'output-en.md must include the visual template'
grep -Fq '## Visual Review' "$render_output_zh_file" \
  || fail 'output-zh.md must include the visual template section heading'
grep -Fq '## Visual Review Matrix' "$render_output_en_file" \
  || fail 'English visual template must include the visual review matrix'
grep -Fq '## 视觉审查矩阵' "$render_output_zh_file" \
  || fail 'Chinese visual template must include the visual review matrix'
grep -Fq 'Blocking reason: <include only for blockers>' "$render_output_en_file" \
  || fail 'English finding template must make blocking reason conditional'
grep -Fq '阻塞原因：<仅阻塞项包含此行>' "$render_output_zh_file" \
  || fail 'Chinese finding template must make 阻塞原因 conditional'
grep -Eq '\*\*变更规模：\*\* .*个文件.*\+[0-9<]+.*行.*/.*-[0-9<]+.*行' "$render_output_zh_file" \
  || fail 'Chinese templates must use localized file and line count units'

if grep -Fq '## Supporting Analysis' "$render_output_en_file"; then
  fail 'English default template must not include Supporting Analysis by default'
fi
if grep -Fq '## 补充分析' "$render_output_zh_file"; then
  fail 'Chinese default template must not include 补充分析 by default'
fi

grep -Fq '<!-- review-meta' "$render_meta_file" \
  || fail 'review-meta.md must define the machine-readable review-meta block'

grep -Fq 'references/rendering/output-en.md' "$render_visual_file" \
  || fail 'visual-output.md must route visual mode to the English rendering template'
grep -Fq 'references/rendering/output-zh.md' "$render_visual_file" \
  || fail 'visual-output.md must route visual mode to the Chinese rendering template'
grep -Fq 'Do not invent precision.' "$render_visual_file" \
  || fail 'visual-output.md must forbid invented precision'
grep -Fq 'Only calculate distribution from real reviewed counts.' "$render_visual_file" \
  || fail 'visual-output.md must forbid invented distribution percentages'

# Advanced layer must remain focused on complex workflows.
grep -Fq 'Coverage-led review exists to prevent false confidence in large or fragmented reviews.' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must define the purpose of coverage-led review'
grep -Fq 'The final user-facing report does not need to expose every internal reducer detail.' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must separate internal reducer state from user-facing output'
grep -Fq 'Use visual review when the change meaningfully affects any of the following:' "$advanced_visual_rules_file" \
  || fail 'visual-review-rules.md must define when visual review is justified'
grep -Fq 'Treat accessibility as a real correctness dimension, not cosmetic polish.' "$advanced_visual_rules_file" \
  || fail 'visual-review-rules.md must treat accessibility as a correctness concern'

# Example layer must be explicitly non-authoritative and split by concern.
grep -Fq 'These examples are optional calibration aids only.' "$examples_en_file" \
  || fail 'English examples file must declare itself optional calibration only'
grep -Fq '这些示例只是可选校准材料。' "$examples_zh_file" \
  || fail 'Chinese examples file must declare itself optional calibration only'
grep -Fq 'Do not use this file as a second rules source.' "$examples_complex_file" \
  || fail 'Complex examples file must not become a second rules source'
grep -Fq '## Example 1: Default Review' "$examples_en_file" \
  || fail 'English examples file must include a default review example'
grep -Fq '## 示例 1：Default Review' "$examples_zh_file" \
  || fail 'Chinese examples file must include a default review example'
grep -Fq '## Example 3: Visual Review' "$examples_complex_file" \
  || fail 'Complex examples file must include a visual review example'

# README files must document the new layered layout.
for readme in "$readme_file" "$readme_zh_file"; do
  grep -Fq 'decision/' "$readme" \
    || fail "$(basename "$readme") must document the decision/ reference layer"
  grep -Fq 'rendering/' "$readme" \
    || fail "$(basename "$readme") must document the rendering/ reference layer"
  grep -Fq 'advanced/' "$readme" \
    || fail "$(basename "$readme") must document the advanced/ reference layer"
  grep -Fq 'examples/' "$readme" \
    || fail "$(basename "$readme") must document the examples/ reference layer"
done

grep -Fq 'User-provided code without before/after diff' "$readme_file" \
  || fail 'README.md must document the user-provided code input path'
grep -Fq 'perform a static pre-commit-style review' "$readme_file" \
  || fail 'README.md must explain the static review fallback for user-provided code'
grep -Fq '`--eval-file` lets `output_eval_runner.sh` target one layered output eval JSON such as `evals/output/visual-output-eval.json`.' "$readme_file" \
  || fail 'README.md must document output_eval_runner.sh layered eval-file usage'
grep -Fq 'readme_surface_test.sh' "$readme_file" \
  || fail 'README.md must document the README surface test'
grep -Fq 'readme_host_entrypoints_test.sh' "$readme_file" \
  || fail 'README.md must document the README host entrypoints surface test'
grep -Fq '用户提供代码但没有 before/after diff' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document the user-provided code input path'
grep -Fq '执行静态的提交前风格审查' "$readme_zh_file" \
  || fail 'README.zh-CN.md must explain the static review fallback for user-provided code'
grep -Fq '`--eval-file` 可让 `output_eval_runner.sh` 指向任意单个分层 output eval JSON，例如 `evals/output/visual-output-eval.json`。' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document output_eval_runner.sh layered eval-file usage'
grep -Fq 'readme_surface_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document the README surface test'
grep -Fq 'readme_host_entrypoints_test.sh' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document the README host entrypoints surface test'

printf 'skill contract tests passed\n'
