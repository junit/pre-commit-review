#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

"$repo_root/install.sh" codex --copy --dir "$tmp_dir/codex-skills"
[ -f "$tmp_dir/codex-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/agents/openai.yaml" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/output-examples.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/visual-output.md" ]

"$repo_root/install.sh" codex --copy --dir "$tmp_dir/codex-skills"
[ -d "$tmp_dir/codex-skills/pre-commit-review" ]

AGENT_SKILLS_DIR="$tmp_dir/generic-agent-skills" CODEX_HOME="$tmp_dir/codex-home" "$repo_root/install.sh" codex --dry-run >"$tmp_dir/codex-env.out"
grep -Fq "Target: $tmp_dir/generic-agent-skills/pre-commit-review" "$tmp_dir/codex-env.out"

CODEX_HOME="$tmp_dir/codex-home" "$repo_root/install.sh" codex --dry-run >"$tmp_dir/codex-home.out"
grep -Fq "Target: $tmp_dir/codex-home/skills/pre-commit-review" "$tmp_dir/codex-home.out"

"$repo_root/install.sh" claude --link --dir "$tmp_dir/claude-skills"
[ -L "$tmp_dir/claude-skills/pre-commit-review" ]
[ "$(CDPATH='' cd -- "$tmp_dir/claude-skills/pre-commit-review" && pwd -P)" = "$repo_root" ]

"$repo_root/install.sh" gemini --dry-run --copy --dir "$tmp_dir/gemini-skills"
[ ! -e "$tmp_dir/gemini-skills/pre-commit-review" ]

KIRO_SKILLS_DIR="$tmp_dir/kiro-skills" "$repo_root/install.sh" kiro --copy
[ -f "$tmp_dir/kiro-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/kiro-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -f "$tmp_dir/kiro-skills/pre-commit-review/references/output-examples.md" ]

"$repo_root/install.sh" kiro --link --dir "$tmp_dir/workspace/.kiro/skills"
[ -L "$tmp_dir/workspace/.kiro/skills/pre-commit-review" ]
[ "$(CDPATH='' cd -- "$tmp_dir/workspace/.kiro/skills/pre-commit-review" && pwd -P)" = "$repo_root" ]

printf 'install.sh smoke tests passed\n'
