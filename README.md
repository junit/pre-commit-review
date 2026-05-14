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
  - hygiene issues
  - intent
  - logic shifts
  - blast radius
  - regression risk
- Returns a clear verdict:
  - `PASS`
  - `PASS_WITH_NOTES`
  - `NEEDS_WORK`
- Uses a read-only helper script to collect local Git context without mutating the repository

## Why This Repository Exists

This repository is not an application or framework. It is a small, portable skill package that can be:

- published as a standalone open source repository
- copied into an existing skills collection
- adapted for local agent tooling that needs pre-commit review behavior

## Repository Structure

```text
.
├── SKILL.md
├── agents/
│   └── openai.yaml
└── scripts/
    └── collect_diff_context.sh
```

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
- truncates oversized diffs safely when needed

It does not fetch, stage, reset, install, or modify files.

### `agents/openai.yaml`

Provides lightweight agent metadata for environments that expose skills through an agent registry.

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

## Usage

### Option 1: Use as a standalone repository

Clone or copy this repository into the place where your agent runtime expects custom skills.

Example layout:

```text
your-skills/
└── pre-commit-review/
    ├── SKILL.md
    ├── agents/
    └── scripts/
```

Then register or expose the skill according to your agent platform's skill-loading mechanism.

### Option 2: Merge into an existing skills collection

If you already maintain a larger skills repository, copy this directory in as one skill package and preserve the relative paths:

- `SKILL.md`
- `scripts/collect_diff_context.sh`
- `agents/openai.yaml`

The helper script is referenced by the skill instructions, so the directory structure should remain intact unless you also update those references.

## Review Output

The expected output is a structured pre-commit review with:

- diff source
- review limits
- files changed
- reviewed and unreviewed scope
- behavior analysis
- risk assessment

Final verdicts mean:

- `PASS`: reviewed scope looks safe to commit
- `PASS_WITH_NOTES`: safe to commit, but follow-up notes or review limits exist
- `NEEDS_WORK`: blocking issue found; do not commit as-is

## Safety Characteristics

This package is intentionally conservative:

- it avoids pretending to see local changes when no repository is available
- it distinguishes staged and unstaged review scope
- it warns about untracked files not present in `git diff`
- it treats large diffs as partial-review situations unless risky files are inspected

## Limitations

- This repository does not include the runtime that loads or executes the skill.
- The exact installation method depends on your agent platform.
- The helper script expects a working `git` executable in the environment.
- The current repository itself may be used outside Git, but local diff collection only works inside a Git repository.

## Contributing

Contributions are best focused on:

- improving review heuristics
- tightening safety boundaries
- refining the output template
- making diff collection more robust across repository states

If you change script paths or repository layout, update `SKILL.md` accordingly.
If you update user-facing documentation, keep localized README files synchronized.

## License

This project is licensed under the Apache License 2.0. See [LICENSE](./LICENSE).
