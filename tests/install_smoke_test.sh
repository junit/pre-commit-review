#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

run_offline_install() {
  "$repo_root/install.sh" "$@" --no-download
}

static_analysis_platform() {
  local os_name arch_name suffix=''
  case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
    darwin) os_name='darwin' ;;
    linux) os_name='linux' ;;
    msys*|mingw*|cygwin*) os_name='windows'; suffix='.exe' ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch_name='arm64' ;;
    x86_64|amd64) arch_name='amd64' ;;
    *) return 1 ;;
  esac
  printf 'static_analysis-%s-%s%s\n' "$os_name" "$arch_name" "$suffix"
}

static_analysis_name="$(static_analysis_platform)"
python_suffix='py'
cargo build --release --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --bin static-analysis-cli >/dev/null

run_offline_install codex --copy --dir "$tmp_dir/codex-skills"
[ -f "$tmp_dir/codex-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/agents/openai.yaml" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_static_evidence.sh" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_static_evidence.$python_suffix" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/run_static_analysis.sh" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/scripts/run_static_analysis.$python_suffix" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/fetch_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks.version" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-assets.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-binaries.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/check_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/gitleaks_integrity.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/static_analysis_cli.sh" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/bin/$static_analysis_name" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.zh-CN.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/install.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/verdict-rules.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/risk-taxonomy.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/static-analysis-evidence.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/static-analysis-execution.md" ]
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
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-input.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-evidence.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-profile.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-execution.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/THIRD_PARTY_LICENSES/gitleaks-LICENSE" ]
(
  cd "$tmp_dir"
  python3 "$tmp_dir/codex-skills/pre-commit-review/scripts/validate_schemas.py" >/dev/null
)

isolated_source="$tmp_dir/source-without-static-checkout"
mkdir -p "$isolated_source/collect-diff-context-cli"
cp "$repo_root/install.sh" "$repo_root/SKILL.md" "$repo_root/LICENSE" "$isolated_source/"
cp -R "$repo_root/agents" "$repo_root/references" "$repo_root/scripts" \
  "$repo_root/THIRD_PARTY_LICENSES" "$isolated_source/"
cp -R "$repo_root/collect-diff-context-cli/schemas" "$isolated_source/collect-diff-context-cli/"
rm -f "$isolated_source"/scripts/bin/static_analysis-*
"$isolated_source/install.sh" codex --copy --dir "$tmp_dir/source-without-static" --no-download
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/collect_static_evidence.sh" ]
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/run_static_analysis.sh" ]
[ -f "$tmp_dir/source-without-static/pre-commit-review/scripts/lib/static_analysis_cli.sh" ]
[ ! -e "$tmp_dir/source-without-static/pre-commit-review/scripts/bin/$static_analysis_name" ]

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
