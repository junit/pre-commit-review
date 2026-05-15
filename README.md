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
│   ├── output-examples.md
│   └── visual-output.md
├── scripts/
│   └── collect_diff_context.sh
└── tests/
    └── install_smoke_test.sh
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

### `references/`

Contains optional guidance loaded only when needed, such as localized output examples and visual report formatting.

### `agents/openai.yaml`

Provides lightweight agent metadata for environments that expose skills through an agent registry.

### `install.sh`

Installs this skill package into a host-specific skills directory for Codex, Claude Code, or Gemini CLI.

### `tests/install_smoke_test.sh`

Runs a small end-to-end installer smoke test against temporary directories.

## Quick Install

From a clone of this repository:

```bash
./install.sh codex
./install.sh claude
./install.sh gemini
```

Defaults:

- `codex` installs to `CODEX_SKILLS_DIR`, then `AGENT_SKILLS_DIR`, then `${CODEX_HOME}/skills`, then `~/.codex/skills`
- `claude` installs to `CLAUDE_SKILLS_DIR`, then `~/.claude/skills`
- `gemini` installs to `GEMINI_SKILLS_DIR`, then `AGENT_SKILLS_DIR`, then `~/.agents/skills`

Useful flags:

- `--copy` copies the skill into the target directory and is the default mode
- `--link` creates a symlink to this repository, which is useful for local development
- `--dir PATH` overrides the target skills directory
- `--force` replaces an existing non-managed target
- `--dry-run` prints what would happen without changing anything

Examples:

```bash
./install.sh codex --link
./install.sh codex --dir .agents/skills
./install.sh gemini --dir .agents/skills
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

## Usage

### Option 1: Use the installer

Clone this repository, then run the installer for your host:

```bash
./install.sh codex
```

or:

```bash
./install.sh claude
./install.sh gemini
```

Restart the agent or start a new session after installing so it can discover the new skill.

### Option 2: Use as a standalone repository

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

### Option 3: Merge into an existing skills collection

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
- it distinguishes staged and unstaged review scope
- it warns about untracked files not present in `git diff`
- it treats large diffs as partial-review situations unless risky files are inspected

## Limitations

- This repository does not include the runtime that loads or executes the skill.
- The included installer covers common Codex, Claude Code, and Gemini CLI locations, but some local setups may still require `--dir` overrides.
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
