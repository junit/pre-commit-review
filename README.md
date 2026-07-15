# pre-commit-review

[![Lint](https://img.shields.io/github/actions/workflow/status/wifibaby4u/pre-commit-review/lint.yml?branch=main&label=lint&logo=github)](https://github.com/wifibaby4u/pre-commit-review/actions/workflows/lint.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![Shell](https://img.shields.io/badge/shell-bash-4EAA25?logo=gnubash&logoColor=white)](https://www.shellcheck.net/)

[English](./README.md) | [简体中文](./README.zh-CN.md)

`pre-commit-review` is a reusable skill package for reviewing Git diffs before committing, pushing, or opening a pull request. In plain terms: **a drop-in pre-commit review step you add to your AI coding agent** (Codex, Claude Code, Gemini CLI, Kiro) so it gives you a structured, repeatable quality gate instead of an ad hoc diff summary.

It is designed for agent workflows such as Codex- or Claude-style skill systems, where you want a structured, repeatable pre-commit quality gate instead of an ad hoc diff summary.

## Available Languages

- English: `README.md`
- Simplified Chinese: `README.zh-CN.md`

Translations should stay functionally aligned. If you update one version, update the others in the same change when possible.

## Table of Contents

- [What It Does](#what-it-does)
- [Requirements](#requirements)
- [Example Output](#example-output)
- [Quick Install](#quick-install)
- [Why This Repository Exists](#why-this-repository-exists)
- [Repository Structure](#repository-structure)
- [How It Works](#how-it-works)
- [Conversation Prompts](#conversation-prompts)
- [Other Integration Modes](#other-integration-modes)
- [Review Output](#review-output)
- [Safety Characteristics](#safety-characteristics)
- [Limitations](#limitations)
- [Contributing](#contributing)
- [License](#license)

## What It Does

- Reviews the most relevant diff source in priority order:
  - user-provided diff
  - staged changes
  - unstaged changes
  - branch vs. base branch
- Produces a consistent review format focused on:
  - what changed
  - code quality issues
  - intent
  - logic shifts
  - blast radius
  - regression risk
  - performance & cost impact (only on hot paths, queries, loops, or network/IO calls)
- Returns a clear verdict:
  - `SAFE_TO_COMMIT`
  - `SAFE_TO_COMMIT_WITH_NOTES`
  - `DO_NOT_COMMIT`
- Uses a read-only helper script to collect local Git context without mutating the repository

## Requirements

- A supported AI coding agent runtime that can load skills (Codex, Claude Code, Gemini CLI, or Kiro). The skill package ships no runtime of its own.
- `git` on `PATH` for local diff collection. The review still works without it when you paste a diff or code directly.
- A Unix-compatible shell to run `install.sh` and the helper. On Windows use Git Bash, MSYS2, or WSL.

## Example Output

A real review ends with a verdict and a one-line conclusion. This is what the skill produces for a one-line README typo fix (Tiny review):

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT
**Conclusion:** Safe to commit; documentation update in README has no runtime risk.
**Diff source:** staged diff via `git diff --cached`
**Review scope:** full review - inspected the single modified hunk in `README.md`
**Change scale:** 1 file, +2 / -2

- **Change:** Corrected installation commands in the README quickstart
- **Logic:** No runtime behavior change
- **Impact scope:** Affects document readers only
- **Risk:** 🟢 Low - prose update only; no impact on code or build
- **Suggested verification:** No tests needed
- **Before commit:** None
```

For larger diffs the skill adds a risk summary table, regression-risk level, and commit guidance with required/suggested fixes. See [`references/examples/`](./references/examples/) for full default and complex examples.

## Quick Install

From a clone of this repository, install globally for any supported agent:

```bash
./install.sh --agent codex
./install.sh --agent claude-code
./install.sh --agent gemini-cli
./install.sh --agent kiro-cli
```

List every supported agent id and its project/global paths:

```bash
./install.sh --list-agents
```

Defaults:

- Global installs use the agent-specific global path shown by `--list-agents`
- Project installs use the agent-specific project path shown by `--list-agents`
- `--dir PATH` overrides both defaults
- `AGENT_SKILLS_DIR` overrides the global default for all agents
- Dedicated overrides are also supported for existing integrations: `CODEX_SKILLS_DIR`, `CLAUDE_SKILLS_DIR`, `GEMINI_SKILLS_DIR`, `KIRO_SKILLS_DIR`, and `CODEX_HOME`
- Backward-compatible aliases are supported: `claude`, `gemini`, and `kiro`

Useful flags:

- `--copy` copies the minimal runtime skill payload into the target directory and is the default mode
- `--link` creates a symlink to this repository, which is useful for local development
- `--project` installs into the agent's project-local skills directory
- `--dir PATH` overrides the target skills directory
- `--force` replaces an existing non-managed target
- `--dry-run` prints what would happen without changing anything

Examples:

```bash
./install.sh --agent cursor --project
./install.sh --agent windsurf --link --project
./install.sh --agent github-copilot --dry-run
./install.sh kiro --dir .kiro/skills
```

## Why This Repository Exists

This repository is not an application or framework. It is a small, portable skill package that can be:

- published as a standalone open source repository
- copied into an existing skills collection
- adapted for local agent tooling that needs pre-commit review behavior

## Repository Structure

```text
.
├── install.sh
├── SKILL.md
├── agents/
│   └── openai.yaml
├── collect-diff-context-cli/
│   ├── Cargo.toml
│   └── src/
├── docs/
│   └── superpowers/
├── references/
├── scripts/
│   ├── bin/
│   ├── build_all_binaries.sh
│   ├── build_with_docker.sh
│   ├── collect_diff_context.sh
│   ├── collect_diff_context.legacy.sh
│   └── validate_schemas.py
├── tests/
│   ├── lib/
│   ├── collect_diff_context_test.sh
│   ├── full_review_workflow_test.sh
│   ├── helper_shadow_mode_test.sh
│   ├── install_agent_matrix_test.sh
│   ├── install_smoke_test.sh
│   ├── parity_assets_test.sh
│   ├── parity_golden_test.sh
│   └── skill_contract_test.sh
└── evals/
    ├── output/
    ├── taxonomy/
    ├── eval_contract_test.sh
    ├── readme_surface_test.sh
    ├── readme_host_entrypoints_test.sh
    ├── output-eval.json
    ├── trigger-eval.json
    ├── output_eval_runner.sh
    ├── output_eval_runner_test.sh
    ├── output_eval_codex_runner.sh
    ├── output_eval_claude_runner.sh
    ├── output_eval_codex_case.sh
    ├── output_eval_claude_case.sh
    └── output_eval_host_wrappers_test.sh
```

### `references/`

Loaded on demand by `SKILL.md`. References are now layered by responsibility:

| Layer | Files | Loaded when | Purpose |
|------|-------|-------------|---------|
| `decision/` | `verdict-rules.md`, `risk-taxonomy.md`, `finding-verification.md` | Every routine review, plus finding verification when strong claims are surfaced | Verdict selection, blocker thresholds, finding markers, tally rules, evidence discipline, and high-impact claim verification |
| `rendering/` | `output-en.md`, `output-zh.md`, `visual-output.md`, `review-meta.md` | When rendering the response | Per-language review skeletons, optional visual presentation guidance, and machine-readable metadata |
| `advanced/` | `coverage-led-review.md`, `visual-review-rules.md`, `grading-compat.md` | Only for complex workflows | Coverage-led review flow, UI/visual review rules, and grading-sensitive exact phrases |
| `examples/` | `default-tiny-en.md`, `default-tiny-zh.md`, `complex-visual-and-coverage.md` | Optional calibration only | Concrete examples for aligning structure and tone without redefining the rules |

Daily Default/Tiny reviews intentionally avoid loading the `examples/` layer unless structure calibration is needed, which keeps routine runs small and stable.

### `SKILL.md`

Defines the skill itself:

- when it should be triggered
- how the diff source is resolved
- how large diffs are handled
- what review dimensions must be covered
- the required output template and verdict rules

### `scripts/collect_diff_context.sh`

A read-only helper script that gathers local repository context for the review workflow. It does three jobs:

1. **Diff source resolution** — detects whether the cwd is a Git repository, prefers staged changes, falls back to unstaged or branch-vs-base, and reports diff stats, file lists, status, truncation, high-risk candidates, generated-like/lock files, and top-churn files. Rename, delete, binary, mode-only, and submodule pointer changes are recorded as manifest units.
2. **A bounded control plane** — emits a compact `--control-plane` JSON gateway with an authoritative full-scope content fingerprint, per-unit fingerprints, bounded units/groups, work order, and reusable command templates; supports `--expect-scope <fingerprint>` on follow-up retrieval so stale output fails closed; and disables external diff/textconv drivers so snapshot identity and inspected content stay aligned.
3. **Coverage-led + test-selection hints** — emits a Review Manifest/Groups and reducer-friendly structured sections (Review Plan JSON, split suggestions, ledgers, work packets, finalization templates), bounded read-only Semantic Context Queries, and Test Selection Hints for changed test files that look environment-dependent, including common JVM/Spring/Quarkus/Micronaut, Maven/Gradle integration naming, JUnit tags, Testcontainers, Docker Compose, WireMock/MockServer, pytest markers, Playwright/Cypress/Node e2e, Go build tags, Rust ignored/integration tests, and database/cache/broker/search service configuration.

The full list of emitted sections (Coverage Ledger Template, Group Review Work Packets, Reducer State Snapshot, etc.) is documented in [`docs/helper-capabilities.md`](./docs/helper-capabilities.md) for integrators building reducer/subagent automation.

It does not fetch, stage, reset, install, or modify files.
It does not run, rewrite, or skip tests. Test Selection Hints are read-only guidance for choosing focused verification commands and for distinguishing sandbox failures from code failures. A `no-known-env-heavy-marker` hint is not proof that a test is isolated; it only means the helper did not match a known environment-heavy marker.

The review workflow starts with `scripts/collect_diff_context.sh --control-plane`. This bounded gateway emits no raw diff and is authoritative only when its collection-start and collection-end fingerprints match. The legacy default output remains plan-first and may omit the global raw diff. `PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES` (default `60000`) controls when that default output inlines the global diff. `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES` (default `200000`) controls truncation for a diff that is actually emitted; use `0` only when printing the full diff is safe.

The default budgets are intentionally conservative even when the selected model advertises a 200K+ context window. CLI hosts can persist or preview large tool stdout before it ever reaches the model, long raw diffs increase latency and multi-turn token cost, and broad diffs can reduce review focus. Treat the defaults as a stable cross-host baseline rather than a model-context maximum.

Advanced gateway budget tuning:
- `PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES`: Default `60000`. Raise it for private deployments with larger model context windows, for example `150000`; lower it for smaller models, for example `30000`.
- `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES`: Default `200000`. Caps any diff that is explicitly emitted through the gateway or follow-up context commands.
- Prompt caching and adaptive inline budgets are deployment-specific optimizations. Enable higher inline budgets only after confirming the host does not hide large stdout behind a preview and that latency/cost remain acceptable.

Review group budgets default to 120KB target and 160KB hard limit. Override them with `PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES` and `PRE_COMMIT_REVIEW_GROUP_HARD_BYTES`; groups over the hard limit are marked `split-required`.

### Rollout and Multi-Implementation Controls
The entrypoint wrapper `scripts/collect_diff_context.sh` supports multiple execution modes for transition and safety:
- `PRE_COMMIT_REVIEW_HELPER_IMPL`: Specifies the helper implementation mode.
  - `rust` (default): Executes the compiled Rust CLI binary. If it fails, prints a warning to `stderr` and gracefully falls back to the legacy script `collect_diff_context.legacy.sh`.
  - `legacy` or `shell`: Forces execution of the legacy Shell script.
  - `shadow`: Runs both the legacy Shell script and the Rust binary, compares their stdout, warns on mismatches, and returns the legacy script's stdout to ensure safety.
- `PRE_COMMIT_REVIEW_SHADOW_MODE`: If set to `1`, forces Shadow Mode comparison even when `PRE_COMMIT_REVIEW_HELPER_IMPL` is explicitly set to `legacy` or `shell`.
- `PRE_COMMIT_REVIEW_SHADOW_DIFF_LOG`: Optional path for writing shadow mismatch diffs. By default, shadow mode does not write diff content to `/tmp`.
- `PRE_COMMIT_REVIEW_DISABLE_FALLBACK`: If set to `1`, disables the legacy script fallback, strictly propagating Rust CLI process failures.

Use `scripts/collect_diff_context.sh --plan-only` or `--include-diff never` to recover only the structured control plane when a host persisted the original helper output. Use `--include-diff always` only when you explicitly want the global raw diff, still bounded by `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES`.

Use `scripts/collect_diff_context.sh --source <staged|unstaged|branch> --group <group_id> --expect-scope <fingerprint>` to retrieve one in-budget review group's diff after opening the control plane. Use `--path <path>` with the same fingerprint for file-level follow-up when a group needs narrower context or has been split. Rerun `--control-plane` before the verdict; snapshot drift invalidates the old ledger instead of being merged into a false complete review. `split-required` groups must be reviewed through bounded replacements instead of as one group.

Project-specific risk hints can live in `.pre-commit-review/risk-paths` and `.pre-commit-review/risk-content`. Each non-empty, non-comment line is an extended regular expression; matches promote files into high-risk ordering but do not change coverage requirements.

Project-specific semantic context hints can live in `.pre-commit-review/context-queries`. Each non-empty, non-comment line is an extended regular expression executed only through bounded read-only `git grep`; these matches can guide dependency or caller checks but never satisfy review coverage.

Project-specific test selection hints can live in `.pre-commit-review/test-hints`. Each non-comment line is a TSV row:

```text
rule_id<TAB>path_regex<TAB>content_regex<TAB>test_kind<TAB>environment_dependency<TAB>confidence<TAB>hint
```

The helper emits the first custom hint whose path or content regex matches a changed test file, ahead of built-in hints. Built-ins cover popular cross-ecosystem conventions, but project-specific config should still be used for local profiles, naming schemes, proprietary test harnesses, and service-backed suites that are not visible from path/content markers alone.

Review-planning tables and `Dependency Summary` use TSV because paths, commands, and dependency details may contain commas.

Reducer and subagent automation should prefer authoritative `Review Control Plane JSON`; the older Review Plan/Manifest/Ledger sections remain compatibility output. TSV tables are primarily for human scanning. Automation must not reconstruct scope from direct `git status` or `git diff --name-only` after the helper has emitted a manifest.

### `tests/`

Deterministic shell tests with no model dependency. `skill_contract_test.sh` pins the cross-document contract between `SKILL.md` and `references/` (forbidden placeholders, required labels, the untranslatable `VERDICT` field). `collect_diff_context_test.sh`, `control_plane_test.sh`, and `full_review_workflow_test.sh` exercise normal output, authoritative snapshot pinning/drift failure, schemas, and full reduction against temporary real Git repositories. `parity_golden_test.sh` reuses shared parity fixtures plus a dedicated normalizer to keep legacy-vs-Rust comparisons stable. `install_smoke_test.sh` and `install_agent_matrix_test.sh` verify the installer across copy/link/dry-run modes and the supported agent matrix. All of them avoid model calls and are safe in CI.

### `evals/`

The LLM-backed evaluation harness is now layered by responsibility:

- `trigger-eval.json` covers skill triggering behavior
- `output-eval.json` remains the compatibility umbrella for core output scenarios
- `evals/output/routine-output-eval.json`, `advanced-output-eval.json`, `visual-output-eval.json`, and `localization-output-eval.json` split output grading into routine, complex, visual, and localization-specific matrices
- `evals/taxonomy/marker-eval.json` isolates finding-marker and tally expectations for `🔒`, `❌`, `⚠️`, `🧪`, `👁️`, `📈`, and `🧭`

Execution entrypoints are layered too:

- `output_eval_runner.sh` prepares real local fixtures for any one eval file, can optionally invoke an external model runner, and grades saved responses against expected verdicts and required phrases
- `--eval-file` lets `output_eval_runner.sh` target one layered output eval JSON such as `evals/output/visual-output-eval.json`.
- `run_layered_output_evals.sh` runs the layered output eval matrix end-to-end across the routine, advanced, visual, and localization eval files
- `run_marker_eval_checks.sh` validates marker-taxonomy coverage and summarizes blocking vs non-blocking case counts
- `output_eval_codex_case.sh` and `output_eval_claude_case.sh` run a single eval case per host
- `output_eval_codex_runner.sh` and `output_eval_claude_runner.sh` are host-specific thin wrappers that link this checkout into the fixture's project-local skill directory (`.agents/skills` for Codex, `.claude/skills` for Claude Code) and delegate to `output_eval_runner.sh` with host-appropriate non-interactive commands
- `output_eval_runner_test.sh` is the deterministic self-test for fixture preparation and grading logic
- `output_eval_host_wrappers_test.sh` verifies the wrappers with mock Codex and Claude binaries so host command templates regress without spending model calls
- `run_helper_gateway_probe.sh` runs a real-host stage that instruments the bundled helper and selected direct Git commands, then fails if a host inspects Git diff source before attempting `scripts/collect_diff_context.sh`
- `check_persisted_output_contract.sh` scans host transcripts for persisted helper output and fails if the saved plan/manifest was never recovered before a full-review claim
- `readme_surface_test.sh` keeps the README-facing public surface aligned with the documented contract gates and entrypoint inventory
- `readme_host_entrypoints_test.sh` pins the tiered `Host Entrypoints` section so the README keeps exposing the host-lane surface by `Primary`, `Analysis`, `Stage`, and `Internal / Repo-wide`
- `eval_contract_test.sh` is the repo-wide gate for trigger evals, layered output evals, marker taxonomy assets, and host-lane contract surfaces

### Host Entrypoints

For the host-lane workflow, use these scripts by tier:

- `Primary`: `evals/run_host_readiness_pipeline.sh`, `evals/run_cross_host_readiness.sh`
- Default entrypoints for end-to-end single-host or cross-host verification
- `Primary / Real Host Smoke`: `evals/run_real_host_smoke.sh`, `.github/workflows/real-host-smoke.yml`
- Use these when you want one stable entrypoint for real authenticated host smoke runs and artifact collection
- `Primary / Output Matrix`: `evals/run_layered_output_evals.sh`, `evals/run_marker_eval_checks.sh`
- Use these to run the layered output-eval surface and marker-taxonomy checks without hand-selecting individual eval assets
- `Analysis`: `evals/analyze_host_readiness_diff.sh`
- Use this to compare cross-host readiness outputs without rerunning each stage
- `Stage`: `evals/check_host_availability.sh`, `evals/run_helper_gateway_probe.sh`, `evals/check_persisted_output_contract.sh`, `evals/run_layered_host_evals.sh`, `evals/host_contract_subset.sh`
- Use these when debugging or running one host-lane boundary directly
- `Internal / Repo-wide`: `evals/eval_contract_test.sh`, host `*_test.sh`, `evals/host_failure_taxonomy.sh`
- Important support surfaces, but not normal user-facing entrypoints
- `Stage reports`: `check_host_availability.sh`, `run_helper_gateway_probe.sh`, `run_layered_host_evals.sh`, and `host_contract_subset.sh` can emit `host-stage-report/v1`
- `Pipeline report`: `run_host_readiness_pipeline.sh` emits `host-readiness-report/v1`
- `Cross-host and diff reports`: `run_cross_host_readiness.sh` emits `cross-host-readiness-report/v1`, and `analyze_host_readiness_diff.sh` emits `host-readiness-diff-report/v1`

### `agents/openai.yaml`

Provides lightweight agent metadata for environments that expose skills through an agent registry.

### `install.sh`

Installs this skill package into host-specific skills directories for supported AI coding agents. See [Quick Install](#quick-install) for usage.

## How It Works

The skill resolves review input in this order:

1. A diff explicitly provided by the user
2. Staged changes in the current repository
3. Unstaged changes if nothing is staged
4. Current branch compared with a detected base branch
5. User-provided code without before/after diff
6. If no diff or code is available, the skill asks for staged changes or a provided diff

If the user provides code without a before/after diff, the skill:

- perform a static pre-commit-style review
- labels the review source as user-provided code
- treats the review as partial
- avoids inferring prior behavior unless the user explicitly showed it

When local repository access is available and the user has not explicitly provided review material, the workflow first attempts the helper at `scripts/collect_diff_context.sh`. Resolve that path relative to the installed `pre-commit-review` skill package containing `SKILL.md`, not relative to the user's project root.

The helper is the source of truth for:

- diff source
- review boundaries
- changed file counts
- staged vs. unstaged notes
- untracked file warnings

Only fall back to direct Git inspection when the helper is unavailable at that resolved path, exits non-zero, cannot be executed in the current host, or the user already provided the review material explicitly.

## Conversation Prompts

Depending on your review scenario, you can trigger and guide the AI using these prompt examples in your conversation. Below are the 5 primary scenarios, their purposes, and typical prompts:

1. **Staged/Unstaged Changes Review (Routine Pre-Commit Check)**
   * **Scenario**: Developers modify code locally and want to assess if the changes are safe before running `git commit`.
   * **Purpose**: Inspect the diff for high-risk issues such as syntax errors, deadlocks, sensitive credential leaks, and missing unit tests.
   * **Prompts**:
     * *“Help me perform a pre-commit review.”*
     * *“Check my staged changes to see if they are safe to commit.”*
     * *“Review my unstaged changes for potential issues or credential leaks.”*
     * *“Check the current modifications for credential leaks or missing tests.”*
2. **Branch vs. Base Merge Review (PR Gateway)**
   * **Scenario**: A branch is developed and ready to be merged into a target branch (e.g., `main`, `develop`) via Pull Request, requiring a review of the cumulative differences.
   * **Purpose**: Perform a static code review on branch changes relative to a specific base ref (e.g. `develop`) as a pre-merging gate.
   * **Prompts**:
     * *“Please review my current branch changes against the `develop` branch (PR review).”*
     * *“Review cumulative differences between the current branch and `main` to see if it is safe to merge.”*
     * *“Run a branch-level merge review against base branch origin/develop.”*
3. **User-Provided Patch/Diff Review (Text-only Diff)**
   * **Scenario**: The agent lacks local Git repository access (e.g., in restricted sandboxes), or you want to review a patch file by copy-pasting the diff text directly.
   * **Purpose**: Evaluate the quality and risks of a pasted patch.
   * **Prompts**:
     * *“I have a git diff patch, please perform a pre-commit review on it: [paste diff here]”*
     * *“Analyze this patch for regression risks: `[paste diff here]`”*
4. **Static Code Review (Single File/No Diff)**
   * **Scenario**: You paste raw source code directly without any before/after diff history, requesting an audit.
   * **Purpose**: Run a static pre-commit style audit. Note that the review will be marked as a "partial review" since no historical diff context is present.
   * **Prompts**:
     * *“I wrote some new code and want a static pre-commit security review: [paste code here]”*
     * *“Review this single file as a pre-commit readiness audit: `[paste code here]`”*
5. **Complex/Large Diff Review (Coverage-Led)**
   * **Scenario**: Large or highly fragmented changes (e.g., major refactoring or version upgrades) where direct end-to-end diff review is unreliable or truncated.
   * **Purpose**: Automatically split changes into manageable groups using `collect_diff_context.sh`, track coverage with a ledger, and synthesize results via a reducer to ensure no modified line goes unreviewed.
   * **Prompts**:
     * *“This branch has a huge diff, please perform a coverage-led review.”*
     * *“Please start a coverage-led pre-commit review, split the changes into groups, and review them step-by-step.”*
     * *“Analyze the large amount of changes on the current branch, generate a Review Plan, and audit them group by group.”*

## Other Integration Modes

### Use as a standalone repository

Clone or copy this repository into the place where your agent runtime expects custom skills.

Example layout:

```text
your-skills/
└── pre-commit-review/
    ├── SKILL.md
    ├── agents/
    ├── references/
    └── scripts/
```

Then register or expose the skill according to your agent platform's skill-loading mechanism.

### Merge into an existing skills collection

If you already maintain a larger skills repository, copy this directory in as one skill package and preserve the relative paths:

- `SKILL.md`
- `scripts/collect_diff_context.sh`
- `references/`
- `agents/openai.yaml`

The helper script is referenced by the skill instructions, so the directory structure should remain intact unless you also update those references.

## Review Output

The expected output is an action-first, fast-scanning pre-commit review with:

- a verdict plus a one-line conclusion
- diff source
- review scope
- change scale
- priority findings with concrete fixes
- the minimum risk and test guidance needed to make a commit decision

The default review should answer three questions first:

- can this be committed now
- what must be fixed before commit
- what should be tested next

Only include deeper intent analysis, before/after logic detail, or extra supporting notes when they materially improve the review.

Final verdicts mean:

- `SAFE_TO_COMMIT`: reviewed scope looks safe to commit now
- `SAFE_TO_COMMIT_WITH_NOTES`: safe to commit now, but follow-up notes or review limits exist
- `DO_NOT_COMMIT`: blocking issue found; do not commit as-is

## Safety Characteristics

This package is intentionally conservative:

- it avoids pretending to see local changes when no repository is available
- it distinguishes staged and unstaged review scope, and flags when unstaged changes touch files also staged
- it warns about untracked files not present in `git diff`
- it never reproduces secret values; flagged credentials are shown as a redacted preview with a rotate suggestion
- it treats large or truncated diffs as a reason to split work and retrieve smaller context, not as permission to skip material units
- it reserves partial triage for advisory fallback and blocks commit-readiness when high-risk units are unreviewed
- it supports coverage-led commit-readiness by requiring every manifest unit to be accounted for before claiming full scope
- it keeps long-review reducer state compact and explicit instead of relying on implicit conversation memory
- it treats semantic context queries as bounded read-only hints, not arbitrary shell commands or coverage substitutes

## Limitations

- This repository does not include the runtime that loads or executes the skill.
- The included installer covers common Codex, Claude Code, and Gemini CLI locations, but some local setups may still require `--dir` overrides.
- The helper script expects a working `git` executable in the environment.
- On Windows, the helper script and installer require a Unix-compatible environment (such as Git Bash, MSYS2, or WSL) to run correctly.
- The current repository itself may be used outside Git, but local diff collection only works inside a Git repository.

## Contributing

Contributions are welcome. Good focus areas: review heuristics, safety boundaries, the output template, and diff collection robustness across repository states.

See **[CONTRIBUTING.md](./CONTRIBUTING.md)** for the development setup (shellcheck, the Rust CLI build, and the deterministic test suites) and PR checklist.

> Note: `README.md` and `README.zh-CN.md` are contract files — several tests assert specific phrases appear in them. When editing, keep those exact strings intact or update the assertions together.

## License

This project is licensed under the Apache License 2.0. See [LICENSE](./LICENSE).
