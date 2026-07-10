#!/usr/bin/env bash
# shellcheck disable=SC2016
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

skill_file="$repo_root/SKILL.md"

decision_verdict_file="$repo_root/references/decision/verdict-rules.md"
decision_risk_file="$repo_root/references/decision/risk-taxonomy.md"
decision_finding_verification_file="$repo_root/references/decision/finding-verification.md"

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
  "$decision_finding_verification_file" \
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
grep -Fq '提交前审查' "$skill_file" \
  || fail 'SKILL.md frontmatter must include Chinese commit-review trigger wording'
grep -Fq '## Scope Guard' "$skill_file" \
  || fail 'SKILL.md must include a post-trigger scope guard'
grep -Fq 'Always preserve the field label `VERDICT` in English.' "$skill_file" \
  || fail 'SKILL.md must keep the VERDICT localization rule'
grep -Fq 'references/decision/verdict-rules.md' "$skill_file" \
  || fail 'SKILL.md must route verdict logic to references/decision/verdict-rules.md'
grep -Fq 'references/decision/risk-taxonomy.md' "$skill_file" \
  || fail 'SKILL.md must route finding taxonomy to references/decision/risk-taxonomy.md'
grep -Fq 'references/decision/finding-verification.md' "$skill_file" \
  || fail 'SKILL.md must route strong finding verification to references/decision/finding-verification.md'
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
grep -Fq 'Treat candidate risks as independent by default when they differ in affected object, trigger condition, failure mode, or required fix.' "$skill_file" \
  || fail 'SKILL.md must define independent candidate risk enumeration'
grep -Fq 'Execution summaries, commit guidance, and risk summaries cannot replace a priority finding entry.' "$skill_file" \
  || fail 'SKILL.md must forbid summary-only priority finding coverage'
grep -Fq 'Interpret priority findings as commit-relevant, high-signal findings, not as blockers only.' "$skill_file" \
  || fail 'SKILL.md must prevent treating priority findings as blockers only'
grep -Fq 'Every material candidate concern must have a visible disposition in the final report' "$skill_file" \
  || fail 'SKILL.md must require visible disposition for material candidate concerns'
grep -Fq 'Before final synthesis, harvest material candidate concerns from changed behavior categories' "$skill_file" \
  || fail 'SKILL.md must require material candidate harvesting before final synthesis'
grep -Fq 'Maintain an internal candidate disposition ledger for those harvested candidates.' "$skill_file" \
  || fail 'SKILL.md must require an internal candidate disposition ledger'
grep -Fq 'If any material candidate lacks a final report location, revise the report before emitting the verdict.' "$skill_file" \
  || fail 'SKILL.md must block final verdict until material candidates have report locations'
grep -Fq 'Boundary-condition failures, ignored validation/intent parameters, side-effect contract gaps, and security TOCTOU residuals are not clean-code smells' "$skill_file" \
  || fail 'SKILL.md must prevent material runtime/contract/security issues from being demoted as clean-code smells'
grep -Fq 'framework-wiring smells' "$skill_file" \
  || fail 'SKILL.md must use generic framework-wiring wording instead of framework-specific smells'
grep -Fq 'Before writing `Unreviewed changes: none` / `未审查变更：无`, reconcile the helper manifest, file list, and inspected content.' "$skill_file" \
  || fail 'SKILL.md must require scope honesty before claiming no unreviewed changes'
grep -Fq 'Verification recommendations must preserve the specific behavioral assertion that makes the concern meaningful.' "$skill_file" \
  || fail 'SKILL.md must preserve specific behavioral verification assertions'
grep -Fq 'If the helper emits `Test Selection Hints`, use them only as read-only guidance for verification planning.' "$skill_file" \
  || fail 'SKILL.md must treat helper test hints as read-only verification guidance'
grep -Fq 'Treat `no-known-env-heavy-marker` as "no known marker matched", not as proof that the test is a pure unit test.' "$skill_file" \
  || fail 'SKILL.md must keep no-known test hints conservative'
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
grep -Fq 'Finding verification exists to prevent false confidence in the final report.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must define the purpose of finding verification'
grep -Fq 'Negative or exhaustive claims require broader evidence than positive claims.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must guard negative and exhaustive claims'
grep -Fq 'Security, auth, authorization, privacy, and injection findings must be traced to the execution point.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require auth/security execution-point tracing'
grep -Fq 'Do not infer framework or library internals from call-site shape alone.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require framework/library behavior verification'
grep -Fq 'distinguish direct evidence from inference when claiming runtime-provided objects, generated wiring, implicit context propagation, default configuration, or auto-created resources exist' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must prevent overconfident runtime/framework existence claims'
grep -Fq '## Priority Threshold Gate' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must define a priority threshold gate'
grep -Fq 'Priority findings are not limited to blockers.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must define priority findings as more than blockers'
grep -Fq 'Do not promote pure clean-code smells into priority findings by default.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must demote pure clean-code smells by default'
grep -Fq 'a boundary fallback is missing and can produce an invalid externally observable value, resource locator, cross-boundary identifier, request descriptor, or persisted state at runtime' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must classify runtime boundary fallback failures as priority-threshold candidates'
grep -Fq 'a caller-visible parameter, flag, mode, or option is ignored' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must classify ignored validation/intent parameters as priority-threshold candidates'
grep -Fq 'SSRF, redirect, proxy, and other outbound-interaction claims must account for time-of-check/time-of-use behavior.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require TOCTOU handling for outbound security claims'
grep -Fq '## Candidate Harvest Gate' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must define a candidate harvest gate'
grep -Fq 'construction or rewriting of externally observable values or cross-boundary identifiers' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must harvest externally observable value construction candidates'
grep -Fq 'cross-boundary I/O, such as network, file-system, process, browser, IPC, plugin, database, message-bus' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must harvest language-agnostic cross-boundary I/O candidates'
grep -Fq 'Do not let a changed behavior category disappear merely because no one had already named it as a finding.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must prevent unnamed changed behavior candidates from disappearing'
grep -Fq 'Maintain a candidate disposition ledger from verification through final synthesis.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require a candidate disposition ledger'
grep -Fq 'Do not emit the final verdict until this ledger has no material candidate without a final report location.' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must block final verdict until candidate disposition is complete'
grep -Fq 'state-changing or configuration-changing code silently bypasses a validation contract' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must describe validation bypass generically'
grep -Fq 'stored or serialized field names' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must use generic stored/serialized field wording'
grep -Fq 'reconcile the material candidate concern ledger' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require final candidate concern reconciliation'
grep -Fq 'report every independently verified priority-threshold risk as its own priority finding' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require each independently verified priority risk to be reported'
grep -Fq 'when a material candidate concern is downgraded or omitted, keep the disposition visible' "$decision_finding_verification_file" \
  || fail 'finding-verification.md must require visible disposition for downgraded material concerns'
grep -Fq 'apply `references/decision/finding-verification.md` before finalizing the finding.' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must route strong claims through finding verification'
grep -Fq 'independent candidate risk points were dispositioned as a priority finding, suggested verification, follow-up/domain confirmation, review limitation, or omitted low-confidence speculation' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must require final disposition for independent candidate risks'
grep -Fq 'no independent priority-threshold risk was replaced by executive summary, commit guidance, or risk summary prose' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must forbid summary prose replacing priority findings'
grep -Fq 'the concern is only a clean-code smell with no demonstrated behavior, contract, release, performance, security, data, or testing impact' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must keep pure clean-code smells out of priority findings'
grep -Fq 'Do not treat "priority finding" as a synonym for "blocker."' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must keep non-blocking material issues eligible for priority findings'
grep -Fq 'Do not use the clean-code-smell exclusion for verified boundary or contract failures.' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must not let clean-code filtering hide boundary or contract failures'
grep -Fq 'Non-priority does not mean invisible.' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must require visible disposition for non-priority material concerns'
grep -Fq 'a safety helper leaves a reachable TOCTOU gap such as DNS rebinding, redirect target drift, or post-validation connection drift' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must preserve security TOCTOU residuals'
grep -Fq 'If the report recommends adding a missing test, missing negative-path assertion, missing integration check, or missing contract test for material changed behavior, the tally must include at least one test gap' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must keep test-gap tally consistent with material missing-test recommendations'
grep -Fq 'If the risk summary says test coverage is sufficient, its basis must not simultaneously call out missing material tests or under-verified high-risk behavior.' "$decision_risk_file" \
  || fail 'risk-taxonomy.md must prevent contradictory test coverage summaries'
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
grep -Fq '只有当所有 manifest/file-list 单元的文本内容已审查' "$render_output_zh_file" \
  || fail 'Chinese template must guard against falsely claiming no unreviewed changes'
grep -Fq 'write `Unreviewed changes: none` only when every manifest/file-list unit' "$render_output_en_file" \
  || fail 'English template must guard against falsely claiming no unreviewed changes'
grep -Fq '不要把本段文字输出给用户' "$render_output_zh_file" \
  || fail 'Chinese template must mark guardrails as internal-only'
grep -Fq 'do not output this text to the user' "$render_output_en_file" \
  || fail 'English template must mark guardrails as internal-only'
grep -Fq 'Keep the tally and Risk Summary consistent' "$render_output_en_file" \
  || fail 'English template must require tally/risk summary consistency'
grep -Fq '保持统计与风险摘要一致' "$render_output_zh_file" \
  || fail 'Chinese template must require tally/risk summary consistency'
grep -Fq '"Priority Findings" is not a blockers-only section' "$render_output_en_file" \
  || fail 'English template must prevent blockers-only priority findings'
grep -Fq '“重点发现”不是“阻断项专区”' "$render_output_zh_file" \
  || fail 'Chinese template must prevent blockers-only priority findings'
grep -Fq 'Only when there are no blocker, non-blocking risk, test-gap, or review-limit items that meet the priority-finding threshold' "$render_output_en_file" \
  || fail 'English template must limit None priority findings to no material priority-threshold items'
grep -Fq '只有在没有任何达到重点发现门槛的阻断项、非阻断风险、测试缺口或审查限制时' "$render_output_zh_file" \
  || fail 'Chinese template must limit 无 priority findings to no material priority-threshold items'
if grep -Fq '渲染守卫：' "$render_output_zh_file"; then
  fail 'Chinese template must not include literal 渲染守卫 output text'
fi
if grep -Fq 'Rendering guardrail:' "$render_output_en_file"; then
  fail 'English template must not include literal Rendering guardrail output text'
fi

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

# Long reference files must remain navigable when loaded through progressive disclosure.
for long_reference_file in \
  "$decision_verdict_file" \
  "$decision_risk_file" \
  "$decision_finding_verification_file" \
  "$advanced_coverage_file" \
  "$advanced_visual_rules_file" \
  "$advanced_grading_file"; do
  grep -Fq '## Contents' "$long_reference_file" \
    || fail "long reference file must include a Contents section: $long_reference_file"
done

# Advanced layer must remain focused on complex workflows.
grep -Fq 'Coverage-led review exists to prevent false confidence in large or fragmented reviews.' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must define the purpose of coverage-led review'
grep -Fq 'The final user-facing report does not need to expose every internal reducer detail.' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must separate internal reducer state from user-facing output'
grep -Fq 'high-impact reducer findings passed the finding verification gate or were downgraded' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must require finding verification before final reducer output'
grep -Fq 'do not mark binary, generated, minified, persisted-output-only, or otherwise unreadable units as fully reviewed by assumption' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must require honest binary/generated/unreadable coverage'
grep -Fq 'do not output the literal internal term `coverage-led` unless a grading-sensitive compatibility instruction explicitly requires that exact token' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must not leak internal workflow terms into routine reports'
grep -Fq 'Was explicit coverage accounting required for this review?' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must phrase user-facing coverage summary without leaking internal mode names'
grep -Fq 'complete commit-readiness decision' "$advanced_coverage_file" \
  || fail 'coverage-led-review.md must avoid user-facing internal coverage-led decision wording'
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
grep -Fq '.pre-commit-review/test-hints' "$readme_file" \
  || fail 'README.md must document project-specific test selection hints'
grep -Fq 'no-known-env-heavy-marker' "$readme_file" \
  || fail 'README.md must document conservative no-known test hint semantics'
grep -Fq 'Playwright/Cypress/Node e2e' "$readme_file" \
  || fail 'README.md must document popular built-in test hint ecosystems'
grep -Fq 'Go build tags' "$readme_file" \
  || fail 'README.md must document Go built-in test hint coverage'
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
grep -Fq '.pre-commit-review/test-hints' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document project-specific test selection hints'
grep -Fq 'no-known-env-heavy-marker' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document conservative no-known test hint semantics'
grep -Fq 'Playwright/Cypress/Node e2e' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document popular built-in test hint ecosystems'
grep -Fq 'Go build tags' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document Go built-in test hint coverage'
grep -Fq 'minimal runtime skill payload' "$readme_file" \
  || fail 'README.md must document minimal runtime copy installs'
grep -Fq '最小运行时 skill payload' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document minimal runtime copy installs'
grep -Fq 'By default, shadow mode does not write diff content to `/tmp`.' "$readme_file" \
  || fail 'README.md must document shadow mode diff logging as opt-in'
grep -Fq '默认 shadow mode 不会把 diff 内容写入 `/tmp`。' "$readme_zh_file" \
  || fail 'README.zh-CN.md must document shadow mode diff logging as opt-in'
grep -Fq 'default_prompt: "Use $pre-commit-review' "$repo_root/agents/openai.yaml" \
  || fail 'agents/openai.yaml default_prompt must explicitly mention $pre-commit-review'

printf 'skill contract tests passed\n'
