# pre-commit-review

[English](./README.md) | [简体中文](./README.zh-CN.md)

`pre-commit-review` is a reusable skill package for reviewing Git diffs before committing, pushing, or opening a pull request.

It is designed for agent workflows such as Codex- or Claude-style skill systems, where you want a structured, repeatable pre-commit quality gate instead of an ad hoc diff summary.

## Available Languages

- English: `README.md`
- Simplified Chinese: `README.zh-CN.md`

Translations should stay functionally aligned. If you update one version, update the others in the same change when possible.

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
├── references/
├── scripts/
│   └── collect_diff_context.sh
├── tests/
│   ├── collect_diff_context_test.sh
│   ├── full_review_workflow_test.sh
│   ├── install_agent_matrix_test.sh
│   ├── install_smoke_test.sh
│   └── skill_contract_test.sh
└── evals/
    ├── eval_contract_test.sh
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

Loaded on demand by `SKILL.md`. Each file has a single responsibility:

| File | Loaded when | Purpose |
|------|-------------|---------|
| `coverage-led-review.md` | Large/truncated diffs, delegated review, or reducer state | Coverage ledger, review groups, split suggestions, reducer templates |
| `output-en.md` | English review output | English Default, Tiny Diff, and Visual Review templates |
| `output-zh.md` | Chinese review output | Chinese Default, Tiny Diff, and Visual Review templates |
| `output-examples.md` | Visual Reviews or complex structural alignment | Concrete worked examples per language |
| `review-risk-taxonomy.md` | Writing Priority Findings | Severity levels, finding structure, evidence rules |
| `review-verdict-rules.md` | Selecting a verdict | Blocker/non-blocking matrices, output quality gate |
| `visual-output.md` | Full Visual Mode | Visual report formatting and skeletons |
| `visual-review-rules.md` | Full Visual Mode | Detailed visual-mode rules |

Daily Default/Tiny reviews intentionally do not load `output-examples.md` to avoid token bloat.

### `SKILL.md`

Defines the skill itself:

- when it should be triggered
- how the diff source is resolved
- how large diffs are handled
- what review dimensions must be covered
- the required output template and verdict rules

### `scripts/collect_diff_context.sh`

A read-only helper script that gathers local repository context for the review workflow. It:

- detects whether the current directory is a Git repository
- prefers staged changes when present
- falls back to unstaged changes or branch-vs-base comparison
- reports diff stats, file lists, and status
- identifies truncation, path/content high-risk candidates, generated-like files, lock files, and top-churn files
- emits a Review Manifest and Review Groups for coverage-led commit-readiness workflows
- records rename, delete, binary, mode-only, and submodule pointer changes as manifest units
- emits Review Plan JSON for reducer-friendly automation without Markdown table parsing
- emits Split Suggestions for review groups that exceed the hard budget
- emits Split Unit Diff Preview blocks for hunk-level review
- emits a Coverage Ledger Template with pending review units
- emits Group Review Result templates for reducer-ready group findings
- emits a Reducer State Snapshot Template for long multi-step reviews
- emits a Coverage Validation Checklist for reducer preflight
- emits a Full Review Execution Plan with ordered split/review steps
- emits Group Review Work Packets for serial or delegated group review
- emits a Reducer Finalization Template for final synthesis gates
- emits a best-effort Dependency Summary for cross-file reduction
- emits bounded Semantic Context Queries from project-provided read-only grep patterns
- emits a suggested review queue for large or truncated diffs
- truncates oversized diffs safely when needed

It does not fetch, stage, reset, install, or modify files.

The default diff output budget is 200KB. Override it with `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES`; use a lower value when the surrounding conversation is already large, and use `0` only when printing the full diff is safe.

Review group budgets default to 120KB target and 160KB hard limit. Override them with `PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES` and `PRE_COMMIT_REVIEW_GROUP_HARD_BYTES`; groups over the hard limit are marked `split-required`.

Use `scripts/collect_diff_context.sh --source <staged|unstaged|branch> --group <group_id>` to retrieve one in-budget review group's diff after a global diff is truncated. Use `--path <path>` for file-level follow-up when a group needs narrower context or has been split. Helper-emitted `context_command` values include `--source` so follow-up retrieval stays pinned to the original diff source; `split-required` groups must be reviewed through split suggestions instead of as one group.

Project-specific risk hints can live in `.pre-commit-review/risk-paths` and `.pre-commit-review/risk-content`. Each non-empty, non-comment line is an extended regular expression; matches promote files into high-risk ordering but do not change coverage requirements.

Project-specific semantic context hints can live in `.pre-commit-review/context-queries`. Each non-empty, non-comment line is an extended regular expression executed only through bounded read-only `git grep`; these matches can guide dependency or caller checks but never satisfy review coverage.

Review-planning tables and `Dependency Summary` use TSV because paths, commands, and dependency details may contain commas.

Reducer and subagent automation should prefer `Review Plan JSON`, `Reducer State Snapshot Template`, and JSONL sections when present; TSV tables are primarily for human scanning.

### `tests/`

Deterministic shell tests with no model dependency. `skill_contract_test.sh` pins the cross-document contract between `SKILL.md` and `references/` (forbidden placeholders, required labels, the untranslatable `VERDICT` field). `collect_diff_context_test.sh` and `full_review_workflow_test.sh` exercise the helper script against temporary real Git repositories. `install_smoke_test.sh` and `install_agent_matrix_test.sh` verify the installer across copy/link/dry-run modes and the supported agent matrix. All of them run with plain `bash` and `jq`, never call a model, and are safe in CI.

### `evals/`

The LLM-backed output evaluation harness. `output-eval.json` and `trigger-eval.json` define eval cases (expected verdicts and required phrases). `output_eval_runner.sh` prepares real local fixtures for every case, can optionally invoke an external model runner, and grades saved responses against the expected verdict and required phrases. `output_eval_runner_test.sh` is the deterministic self-test: it synthesizes mock responses and verifies the grading logic without a model. `output_eval_codex_runner.sh` and `output_eval_claude_runner.sh` are host-specific thin wrappers that link this checkout into the fixture's project-local skill directory (`.agents/skills` for Codex, `.claude/skills` for Claude Code) and delegate to `output_eval_runner.sh` with host-appropriate non-interactive commands. `output_eval_codex_case.sh` and `output_eval_claude_case.sh` run a single case per host. `output_eval_host_wrappers_test.sh` verifies the wrappers with mock Codex and Claude binaries so host command templates regress without spending model calls. `eval_contract_test.sh` validates the structure of both eval JSON files.

### `agents/openai.yaml`

Provides lightweight agent metadata for environments that expose skills through an agent registry.

### `install.sh`

Installs this skill package into host-specific skills directories for supported AI coding agents.

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

- `--copy` copies the skill into the target directory and is the default mode
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

## How It Works

The skill resolves review input in this order:

1. A diff explicitly provided by the user
2. Staged changes in the current repository
3. Unstaged changes if nothing is staged
4. Current branch compared with a detected base branch
5. If no diff is available, the skill asks for staged changes or a provided diff

When local repository access is available, the workflow prefers using `scripts/collect_diff_context.sh` as the source of truth for:

- diff source
- review boundaries
- changed file counts
- staged vs. unstaged notes
- untracked file warnings

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

Contributions are best focused on:

- improving review heuristics
- tightening safety boundaries
- refining the output template
- making diff collection more robust across repository states

If you change script paths or repository layout, update `SKILL.md` accordingly.
If you update user-facing documentation, keep localized README files synchronized.

### Development

Shell scripts (`scripts/*.sh`, `install.sh`, `tests/*.sh`, `evals/*.sh`) are linted by [shellcheck](https://www.shellcheck.net/) in CI (`.github/workflows/lint.yml`). Install it locally (`brew install shellcheck` on macOS) and run `shellcheck -s bash scripts/*.sh install.sh tests/*.sh evals/*.sh` before submitting changes.

The deterministic test suite is `bash tests/*_test.sh`. The eval harness also ships deterministic self-tests that do not call a model: `bash evals/eval_contract_test.sh`, `bash evals/output_eval_runner_test.sh`, and `bash evals/output_eval_host_wrappers_test.sh`. The model-backed runners (`evals/output_eval_codex_runner.sh`, `evals/output_eval_claude_runner.sh`) require a real Codex or Claude CLI and are not part of CI.

## License

This project is licensed under the Apache License 2.0. See [LICENSE](./LICENSE).
