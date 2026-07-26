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

repository_context_platform() {
  local static_name
  static_name="$(static_analysis_platform)"
  printf 'repository_context-%s\n' "${static_name#static_analysis-}"
}

static_analysis_name="$(static_analysis_platform)"
repository_context_name="$(repository_context_platform)"
python_suffix='py'
cargo build --release --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" \
  --bin static-analysis-cli --bin repository-context-cli >/dev/null

run_offline_install codex --copy --dir "$tmp_dir/codex-skills"
[ -f "$tmp_dir/codex-skills/pre-commit-review/SKILL.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/agents/openai.yaml" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_diff_context.sh" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_impact_context.sh" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_static_evidence.sh" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/scripts/collect_static_evidence.$python_suffix" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/run_static_analysis.sh" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/scripts/run_static_analysis.$python_suffix" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/orchestrate_static_analysis.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/fetch_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks.version" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-assets.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/gitleaks-binaries.sha256" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/check_gitleaks.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/gitleaks_integrity.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/static_analysis_cli.sh" ]
[ -r "$tmp_dir/codex-skills/pre-commit-review/scripts/lib/repository_context_cli.sh" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/bin/$static_analysis_name" ]
[ -x "$tmp_dir/codex-skills/pre-commit-review/scripts/bin/$repository_context_name" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/README.zh-CN.md" ]
[ ! -e "$tmp_dir/codex-skills/pre-commit-review/install.sh" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/verdict-rules.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/risk-taxonomy.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/static-analysis-evidence.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/static-analysis-execution.md" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/references/decision/static-analysis-orchestration.md" ]
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
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/static-analysis-orchestration.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/collect-diff-context-cli/schemas/impact-context.schema.json" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/THIRD_PARTY_LICENSES/gitleaks-LICENSE" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/THIRD_PARTY_LICENSES/tree-sitter-LICENSE" ]
[ -f "$tmp_dir/codex-skills/pre-commit-review/THIRD_PARTY_LICENSES/tree-sitter-rust-LICENSE" ]
(
  cd "$tmp_dir"
  python3 "$tmp_dir/codex-skills/pre-commit-review/scripts/validate_schemas.py" >/dev/null
)
python3 "$tmp_dir/codex-skills/pre-commit-review/scripts/validate_schemas.py" --help >"$tmp_dir/schema-help.out"
grep -Fq -- '--static-orchestration-manifest' "$tmp_dir/schema-help.out"
grep -Fq -- '--static-orchestration-output' "$tmp_dir/schema-help.out"

isolated_source="$tmp_dir/source-without-static-checkout"
mkdir -p "$isolated_source/collect-diff-context-cli"
cp "$repo_root/install.sh" "$repo_root/SKILL.md" "$repo_root/LICENSE" "$isolated_source/"
cp -R "$repo_root/agents" "$repo_root/references" "$repo_root/scripts" \
  "$repo_root/THIRD_PARTY_LICENSES" "$isolated_source/"
cp -R "$repo_root/collect-diff-context-cli/schemas" "$isolated_source/collect-diff-context-cli/"
rm -f "$isolated_source"/scripts/bin/static_analysis-* \
  "$isolated_source"/scripts/bin/repository_context-*
"$isolated_source/install.sh" codex --copy --dir "$tmp_dir/source-without-static" --no-download
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/collect_impact_context.sh" ]
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/collect_static_evidence.sh" ]
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/run_static_analysis.sh" ]
[ -x "$tmp_dir/source-without-static/pre-commit-review/scripts/orchestrate_static_analysis.sh" ]
[ -f "$tmp_dir/source-without-static/pre-commit-review/scripts/lib/static_analysis_cli.sh" ]
[ -r "$tmp_dir/source-without-static/pre-commit-review/scripts/lib/repository_context_cli.sh" ]
[ ! -e "$tmp_dir/source-without-static/pre-commit-review/scripts/bin/$static_analysis_name" ]
[ ! -e "$tmp_dir/source-without-static/pre-commit-review/scripts/bin/$repository_context_name" ]

grep -Fq "\"\$static_binary\" orchestrate --help" "$repo_root/.github/workflows/lint.yml"
grep -Fq "\"\$repository_binary\" collect --help" "$repo_root/.github/workflows/lint.yml"
grep -Fq './tests/static_analysis_orchestration_test.sh' "$repo_root/.github/workflows/lint.yml"
grep -Fq "\"\$static_binary\" orchestrate --help" "$repo_root/.github/workflows/release.yml"
grep -Fq "\"\$repository_binary\" collect --help" "$repo_root/.github/workflows/release.yml"
grep -Fq 'chmod +x dist/pre-commit-review/scripts/orchestrate_static_analysis.sh' "$repo_root/.github/workflows/release.yml"
grep -Fq 'chmod +x dist/pre-commit-review/scripts/collect_impact_context.sh' "$repo_root/.github/workflows/release.yml"
grep -Fq "find artifacts -type f -name 'repository_context-*'" "$repo_root/.github/workflows/release.yml"
grep -Fq 'dist/pre-commit-review.cdx.json' "$repo_root/.github/workflows/release.yml"
grep -Fq 'tree-sitter@0.26.11' "$repo_root/.github/workflows/release.yml"
grep -Fq 'tree-sitter-rust@0.24.2' "$repo_root/.github/workflows/release.yml"

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
