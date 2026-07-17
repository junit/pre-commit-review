#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

run_offline_install() {
  "$repo_root/install.sh" "$@" --no-download
}

run_offline_install codex --copy --dir "$tmp_dir/codex-skills"
[ -f "$tmp_dir/codex-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/agents/openai.yaml" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/fetch_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks.version" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-assets.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-binaries.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/check_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/gitleaks_integrity.sh" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.zh-CN.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/install.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/verdict-rules.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/risk-taxonomy.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/rendering/output-en.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/rendering/output-zh.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/rendering/visual-output.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/rendering/review-meta.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/advanced/coverage-led-review.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/advanced/visual-review-rules.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/advanced/grading-compat.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/examples/default-tiny-en.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/examples/default-tiny-zh.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/examples/complex-visual-and-coverage.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/security/gitleaks.toml" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/THIRD_PARTY_LICENSES/gitleaks-LICENSE" ]

run_offline_install codex --copy --dir "$tmp_dir/codex-skills"
[ -d "$tmp_dir/codex-skills/pre-commit-review" ]

AGENT_SKILLS_DIR="$tmp_dir/generic-agent-skills" CODEX_HOME="$tmp_dir/codex-home" "$repo_root/install.sh" codex --dry-run >"$tmp_dir/codex-env.out"
grep -Fq "Target: $tmp_dir/generic-agent-skills/pre-commit-review" "$tmp_dir/codex-env.out"

CODEX_HOME="$tmp_dir/codex-home" "$repo_root/install.sh" codex --dry-run >"$tmp_dir/codex-home.out"
grep -Fq "Target: $tmp_dir/codex-home/skills/pre-commit-review" "$tmp_dir/codex-home.out"

run_offline_install claude --link --dir "$tmp_dir/claude-skills"
[ -L "$tmp_dir/claude-skills/pre-commit-review" ]
[ "$(CDPATH='' cd -- "$tmp_dir/claude-skills/pre-commit-review" && pwd -P)" = "$repo_root" ]

"$repo_root/install.sh" gemini --dry-run --copy --dir "$tmp_dir/gemini-skills"
[ ! -e "$tmp_dir/gemini-skills/pre-commit-review" ]

KIRO_SKILLS_DIR="$tmp_dir/kiro-skills" run_offline_install kiro --copy
[ -f "$tmp_dir/kiro-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/kiro-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -f "$tmp_dir/kiro-skills/pre-commit-review/references/examples/complex-visual-and-coverage.md" ]

run_offline_install kiro --link --dir "$tmp_dir/workspace/.kiro/skills"
[ -L "$tmp_dir/workspace/.kiro/skills/pre-commit-review" ]
[ "$(CDPATH='' cd -- "$tmp_dir/workspace/.kiro/skills/pre-commit-review" && pwd -P)" = "$repo_root" ]

printf 'install.sh smoke tests passed\n'
