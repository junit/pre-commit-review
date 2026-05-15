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

"$repo_root/install.sh" claude --link --dir "$tmp_dir/claude-skills"
[ -L "$tmp_dir/claude-skills/pre-commit-review" ]
[ "$(CDPATH='' cd -- "$tmp_dir/claude-skills/pre-commit-review" && pwd -P)" = "$repo_root" ]

"$repo_root/install.sh" gemini --dry-run --copy --dir "$tmp_dir/gemini-skills"
[ ! -e "$tmp_dir/gemini-skills/pre-commit-review" ]

printf 'install.sh smoke tests passed\n'
