#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
skill_file="$repo_root/SKILL.md"
coverage_reference_file="$repo_root/references/coverage-led-review.md"
output_examples_file="$repo_root/references/output-examples.md"
visual_output_file="$repo_root/references/visual-output.md"
readme_file="$repo_root/README.md"
readme_zh_file="$repo_root/README.zh-CN.md"

fail() {
  printf 'skill contract test failed: %s\n' "$*" >&2
  exit 1
}

contains_contract() {
  local expected="$1"
  grep -Fq "$expected" "$skill_file" "$coverage_reference_file"
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

contains_contract 'git diff --cached -- path/to/file' \
  || fail 'SKILL.md must tell reviewers to use staged file-specific diffs for staged reviews'
contains_contract 'git diff <base>...HEAD -- path/to/file' \
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
grep -Fq 'Commit-readiness reviews are coverage-led by default: start from `Review Manifest JSONL` or `Review Manifest`, account for every review unit, and treat large or truncated diffs as a reason to split or retrieve context rather than sample or skip.' "$skill_file" \
  || fail 'SKILL.md must make coverage-led review the default commit-readiness path'
grep -Fq 'Load `references/coverage-led-review.md` when the helper emits `Review Plan JSON`, when the diff is large/truncated, when any group is `split-required`, when review work is delegated, or when reducer state must survive a long multi-step review.' "$skill_file" \
  || fail 'SKILL.md must route detailed coverage-led workflow to the reference file'
contains_contract 'Risk classification controls review order and split strategy; it never authorizes omitting executable or material units from a commit-readiness review.' \
  || fail 'SKILL.md must make risk classification an ordering signal, not a skip rule'
grep -Fq 'Use advisory fallback only when repository/helper access is unavailable, the user explicitly asks for quick triage, or the user declines continuing the coverage-led review after being told that commit-readiness requires coverage-led validation; label it partial/advisory and do not provide a commit-safe verdict from sampled coverage.' "$skill_file" \
  || fail 'SKILL.md must restrict triage to advisory fallback'
grep -Fq 'When helper/repository access is available and the user asks for commit-readiness, do not self-select advisory fallback to save time; continue coverage-led review or report that commit-readiness is blocked pending coverage.' "$skill_file" \
  || fail 'SKILL.md must prevent self-selected advisory fallback when helper access is available'
grep -Fq 'Unreviewed high-risk candidates make commit-readiness `DO_NOT_COMMIT`; advisory fallback must not present a commit-safe verdict.' "$skill_file" \
  || fail 'SKILL.md must make skipped high-risk files blocking for commit-readiness'
grep -Fq 'Treat `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES` as an output budget, not a safety boundary; lower it when conversation context is crowded, and raise it or set it to `0` only when printing the larger diff is safe.' "$skill_file" \
  || fail 'SKILL.md must describe diff byte budget limits relative to LLM context'
contains_contract 'Helper candidates are not exhaustive; semantically scan the full file list, diff stat, and changed file types, then promote any ordinary-looking file to high risk when its role, imports, API surface, or changed content affects a trust boundary or irreversible behavior.' \
  || fail 'SKILL.md must require semantic risk promotion beyond helper regex candidates'
contains_contract 'The helper may read optional project-level risk hints from `.pre-commit-review/risk-paths` and `.pre-commit-review/risk-content`; each non-empty, non-comment line is an extended regular expression used only to promote matching files into high-risk ordering.' \
  || fail 'SKILL.md must document project-level risk hint files as ordering-only signals'
contains_contract 'The helper may read optional `.pre-commit-review/context-queries`; each non-empty, non-comment line is an extended regular expression executed only through bounded read-only `git grep` to provide surrounding semantic context, never as a shell command and never as a coverage substitute.' \
  || fail 'coverage-led reference must document bounded semantic context queries'
contains_contract 'Treat `Semantic Context Queries` as best-effort surrounding context for dependency and caller checks; it can promote follow-up inspection, but it cannot mark any manifest unit reviewed.' \
  || fail 'coverage-led reference must forbid context queries from satisfying coverage'
grep -Fq 'Coverage-led review requires a coverage ledger: every `Review Manifest` unit must appear in exactly one group review result before the final verdict can claim a full review.' "$skill_file" \
  || fail 'SKILL.md must require a coverage ledger for coverage-led review'
grep -Fq 'Run Coverage Validation before cross-file reduction: compute `manifest_units - reviewed_units`; any high-risk coverage gap makes the verdict `DO_NOT_COMMIT`.' "$skill_file" \
  || fail 'SKILL.md must require coverage validation before final reduction'
contains_contract 'Use `Dependency Summary` as reducer input for changed imports, exports, signatures, and schema/config signals, but treat it as best-effort rather than complete static analysis.' \
  || fail 'SKILL.md must require reducer use of Dependency Summary with best-effort limits'
contains_contract 'Treat `Dependency Summary` as TSV as well; file paths and dependency details may contain commas, so do not parse it as CSV.' \
  || fail 'SKILL.md must require TSV parsing for Dependency Summary'
contains_contract 'If a `Review Groups` row has `budget_status` of `split-required`, split that group into smaller file or hunk units before reviewing it; do not mark it covered as a single group.' \
  || fail 'SKILL.md must require splitting over-budget review groups'
contains_contract 'Use `Split Suggestions` as the starting point for replacing an over-budget group with smaller file or hunk units in the coverage ledger.' \
  || fail 'SKILL.md must require using split suggestions for over-budget groups'
contains_contract 'Start coverage-led review from the `Coverage Ledger Template`; leave units pending until a group result records the exact reviewed unit, and replace `needs-split` rows with `Split Suggestions` units before review.' \
  || fail 'SKILL.md must require using the coverage ledger template'
contains_contract 'Use the helper-provided `Group Review Result Template` for every group result; keep `required_units` intact and fill `reviewed_units` only with units actually inspected.' \
  || fail 'SKILL.md must require group result templates for reducer input'
contains_contract 'Use `Reducer State Snapshot Template` as the compact persistent state for long reviews; carry it forward after every group result and update `reviewed_units`, `pending_units`, `needs_split_units`, `group_results`, `coverage_gaps`, `finding_merge`, `dependency_checks`, and `test_recommendations`.' \
  || fail 'coverage-led reference must require reducer state snapshots for long reviews'
contains_contract 'Before every reducer pass, reconcile the current reducer state against `Review Plan JSON` and `Coverage Ledger Template`; if the state is missing a manifest unit or contains an unknown unit, treat coverage validation as failed until corrected.' \
  || fail 'coverage-led reference must require reducer state reconciliation'
contains_contract 'Before merging findings, use `Coverage Validation Checklist` as reducer preflight; full review is forbidden until `manifest_units - reviewed_units` is empty and all `needs-split` units have replacement results.' \
  || fail 'SKILL.md must require coverage validation checklist before reduction'
contains_contract 'Use `Full Review Execution Plan` as the default work order: split `split-required` groups first, then review high-risk groups, consistency groups, and medium-risk groups unless dependency evidence requires reordering.' \
  || fail 'SKILL.md must require the full review execution plan work order'
contains_contract 'Use `Split Unit Diff Preview` for hunk-level review when present; if the preview is insufficient or truncated, fall back to the listed file-specific command and hunk header.' \
  || fail 'SKILL.md must require split unit diff previews for hunk review'
contains_contract 'Use `Group Review Work Packets` as the handoff context for serial or delegated group review; each packet carries the group id, required units, review commands, and split guidance.' \
  || fail 'SKILL.md must require group review work packets as review handoff context'
contains_contract 'Use `Reducer Finalization Template` for the final synthesis; do not produce the top-level verdict until coverage validation, finding merge, dependency checks, and test recommendations are filled.' \
  || fail 'SKILL.md must require reducer finalization before top-level verdict'
contains_contract 'Use the work packet `context_command` when a group or file needs fresh context after global diff truncation; it must return only the requested group or file diff without widening review scope.' \
  || fail 'SKILL.md must require file-specific context commands after truncation'
contains_contract 'Prefer group-level `context_command` values with `--group <group_id>` for groups within the hard budget; use file-level `--path <path>` commands from the manifest only when a group needs narrower context or has been split.' \
  || fail 'SKILL.md must prefer group-level context commands for in-budget groups'
contains_contract 'Do not use `--group` to review a `split-required` group as one unit; replace it with `Split Suggestions` units first.' \
  || fail 'SKILL.md must forbid whole-group review for split-required groups'
contains_contract 'Every `context_command` must include `--source staged`, `--source unstaged`, or `--source branch` so follow-up context retrieval cannot switch diff sources when the working tree changes.' \
  || fail 'SKILL.md must require source-locked context commands'
contains_contract 'Treat `Review Manifest`, `Review Groups`, `Split Suggestions`, `Coverage Ledger Template`, and `Full Review Execution Plan` as TSV tables; do not parse their rows by comma because paths and commands may contain commas.' \
  || fail 'SKILL.md must require TSV parsing for review-planning tables'
contains_contract 'Prefer `Review Manifest JSONL` and `Review Groups JSONL` for reducer or subagent automation; keep TSV tables for human scanning only.' \
  || fail 'SKILL.md must prefer JSONL for automated reducer inputs'
contains_contract 'Use `Review Plan JSON` as the reducer-friendly aggregate plan when present; it captures group order, required units, budget status, context commands, and coverage gates without parsing Markdown tables.' \
  || fail 'SKILL.md must require Review Plan JSON for reducer-friendly automation'
if grep -Fq '10+ meaningful files' "$skill_file"; then
  fail 'SKILL.md must not use the ambiguous phrase "meaningful files"'
fi
if grep -Fq 'Large Diff Triage Protocol' "$skill_file"; then
  fail 'SKILL.md must not keep a separate large diff triage protocol'
fi
if grep -Fq 'max(5, ceil(10% of the group))' "$skill_file"; then
  fail 'SKILL.md must not use sampling quotas for commit-readiness'
fi
if grep -Fq 'Use deterministic representative sampling' "$skill_file"; then
  fail 'SKILL.md must not present representative sampling as a commit-readiness mechanism'
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
