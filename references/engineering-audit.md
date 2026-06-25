# Engineering Quality Audit (2026-06-25)

> Engineering reference, not review instructions. This file documents a one-time quality assessment of the skill package itself. It is **not** loaded during reviews and does not change review behavior. SKILL.md remains authoritative for all review logic.

Date: 2026-06-25
Scope: design, implementation, testing, portability, and security of the `pre-commit-review` skill package as of commit `66fb621`.

## Verdict

**Industrial-grade.** This is a portable, dependency-free, tested safety tool — not a prompt wrapper. It sits in the top tier of skill-class projects. The assessment below is deliberately critical so it can drive future hardening, not to qualify the verdict.

## Scorecard

| Dimension | Grade | Notes |
|---|---|---|
| Architecture & progressive disclosure | A | Correct three-level loading (SKILL.md 384 lines → 3 refs on demand) |
| Determinism / correctness | A | Read-only helper, `set -euo pipefail` universal, verified end-to-end on a real diff |
| Security posture | A− | No mutation, no fetch, no push; one minor surface (see Issue 2) |
| Portability | A+ | Bash + Git + AWK only. 40+ agent install matrix |
| Test engineering | A | 8 suites, all passing; contract + workflow + install matrix |
| Prompt engineering | A**** | Principle-first, why-explanations, non-rigid; localization contract is strict |
| Documentation | A | Bilingual README, tree synced to reality, design rationale present |
| Maintainability | B+ | Single 1935-line script is the only real concern (see Issue 1) |

## Strengths (industrial-grade patterns)

1. **The helper is a real tool, not a script toy.** `set -euo pipefail` + `trap ... EXIT` for temp cleanup, `-c color.ui=false` + `--no-ext-diff` + `--find-renames` for reproducible diffs. Risk classification is split into path signals (auth/crypto/migration/deploy…) and content signals (`ALTER TABLE`, `eval(`, `process.env.*secret`) matching real vulnerability classes. JSON is built by hand in AWK with correct escaping, so no `jq` dependency.

2. **The safety contract is explicit and tested.** The helper cannot mutate the repo by construction: no fetch, no stage, no commit, no branch switch (SKILL.md line 19 rule + helper code). The installer uses a managed-mark check so it refuses to overwrite directories it does not own unless `--force`, and an atomic staging dir means an interrupted install leaves no broken skill directory.

3. **Coverage-led, no-skip is the core IP.** The skill deliberately rejects the common LLM-code-review failure mode of sampling part of a diff and declaring it safe. Contract tests forbid `max(5, ceil(10%))` quotas, `deterministic representative sampling`, and the old `Large Diff Triage Protocol` wording. Any unreviewed high-risk unit forces `DO_NOT_COMMIT`.

4. **Injection-safe self-injection.** The helper emits `context_command` strings it will later re-execute (`--group` / `--path` recall). Safety comes from `shell_quote` (`printf '%q'`) plus **source-locking**: every recall must carry `--source staged|unstaged|branch` so a working-tree change cannot silently switch which diff is reviewed.

## Issues (refinement backlog)

These are non-blocking. Ordered by realism.

### Issue 1 — Single 1935-line script (maintainability; most real)
`scripts/collect_diff_context.sh` is ~5x the size of every other file and mixes arg parsing, six AWK risk classifiers, JSON emission, group splitting, and orchestration. The repo coding-style rule is "800 max." Readable today, but no symbol index navigates it and test coverage for small logic changes is indirect. **Defensible as-is** because single-file maximizes portability (no `source` paths). Least-risk mitigation: add a table-of-contents comment at the top. Future growth could extract AWK classifiers into a `scripts/lib/` loader.

### Issue 2 — `rm -rf "$path"` in installer (security; low)
`install.sh:228` removes the target skill dir. Path is well-validated (home expand, managed-mark check, refuses non-managed dirs, `--dry-run` no-ops). `--dir` requires non-empty, `staging_dir` is always `.tmp.$$`-suffixed. The only gap: an extreme `--dir /` would still recurse into a `pre-commit-review/` child (bounded), but a one-line root guard (`[ "${target_dir#/}" != "/" ] || die`) would close it. Very minor.

### Issue 3 — README tree is a brittle contract (maintainability)
`skill_contract_test.sh` asserts every test filename appears in the README tree (lines 269–324). Strong drift guard, but adding a test now also requires editing the README or CI fails. Consider relaxing to "tests dir non-empty" or auto-generating the tree.

### Issue 4 — Description trigger could be pushier (recall)
Per skill-creator best practice, skill descriptions under-trigger. This one is well-formed and bilingual, but reads restrained. A short "Use this whenever…" clause would raise recall on vague "is this safe?" prompts. Optional.

### Issue 5 — shellcheck not enforced (CI gap)
Code is clean but unverifiable without shellcheck installed. Industrial fix: add a CI step `shellcheck scripts/*.sh install.sh tests/*.sh`, or document it as a dev prerequisite.

## How this audit was produced

- Ran all 8 test suites (all PASS) with `ECC_GATEGUARD=off`.
- Static audit: strict-mode presence, `rm -rf` call sites, eval/injection surface, ref quoting.
- Dynamic probe on a real temp repo: weird filenames, secret-bearing auth path detection, file-specific context recall.
- Read SKILL.md, coverage-led-review.md, the full helper, installer, contract test, READMEs.

Re-run the test suites before treating any future change as safe; the 250 contract assertions are the regression net for prompt-level behavior.
