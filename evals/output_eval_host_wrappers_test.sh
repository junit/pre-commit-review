#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
codex_runner="$repo_root/evals/output_eval_codex_runner.sh"
claude_runner="$repo_root/evals/output_eval_claude_runner.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'output eval host wrapper test failed: %s\n' "$*" >&2
  exit 1
}

cat >"$tmp_dir/mock-codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

log_file="${MOCK_CLI_LOG_FILE:?}"
response_file=''
model_value=''
skip_repo='no'
ephemeral='no'
sandbox_value=''
approval_value=''
color_value=''

while [ "$#" -gt 0 ]; do
  case "$1" in
    exec) ;;
    --skip-git-repo-check) skip_repo='yes' ;;
    --ephemeral) ephemeral='yes' ;;
    --sandbox)
      shift
      sandbox_value="$1"
      ;;
    --color)
      shift
      color_value="$1"
      ;;
    --model)
      shift
      model_value="$1"
      ;;
    -o)
      shift
      response_file="$1"
      ;;
    -) ;;
    *)
      ;;
  esac
  shift
done

prompt_text="$(cat)"
[ -L .agents/skills/pre-commit-review ] || exit 11

printf 'codex|scenario=%s|model=%s|skip=%s|ephemeral=%s|sandbox=%s|approval=%s|color=%s|prompt=%s\n' \
  "${PRE_COMMIT_REVIEW_EVAL_SCENARIO:-}" "$model_value" "$skip_repo" "$ephemeral" "$sandbox_value" "$approval_value" "$color_value" "$prompt_text" >>"$log_file"

cat >"$response_file" <<'OUT'
Diff source: unavailable
No diff available
OUT
EOF
chmod +x "$tmp_dir/mock-codex"

cat >"$tmp_dir/mock-claude" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

log_file="${MOCK_CLI_LOG_FILE:?}"
model_value=''
print_mode='no'
format_value=''
permission_mode=''
bare='no'
session_persistence='yes'
prompt_text=''

while [ "$#" -gt 0 ]; do
  case "$1" in
    -p) print_mode='yes' ;;
    --output-format)
      shift
      format_value="$1"
      ;;
    --permission-mode)
      shift
      permission_mode="$1"
      ;;
    --bare) bare='yes' ;;
    --no-session-persistence) session_persistence='no' ;;
    --model)
      shift
      model_value="$1"
      ;;
    *)
      prompt_text="$1"
      ;;
  esac
  shift
done

[ -L .claude/skills/pre-commit-review ] || exit 12

printf 'claude|scenario=%s|model=%s|print=%s|format=%s|permission=%s|bare=%s|persist=%s|prompt=%s\n' \
  "${PRE_COMMIT_REVIEW_EVAL_SCENARIO:-}" "$model_value" "$print_mode" "$format_value" "$permission_mode" "$bare" "$session_persistence" "$prompt_text" >>"$log_file"

cat <<'OUT'
**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
差异来源
审查范围
变更规模
建议验证
OUT
EOF
chmod +x "$tmp_dir/mock-claude"

MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
  bash "$codex_runner" \
    --codex-bin "$tmp_dir/mock-codex" \
    --model gpt-5.5 \
    --case no-git-repo \
    --fixtures-dir "$tmp_dir/codex-fixtures" \
    --responses-dir "$tmp_dir/codex-responses" >"$tmp_dir/codex.out"

grep -Fq 'PASS no-git-repo' "$tmp_dir/codex.out" \
  || fail 'codex wrapper did not grade the no-git-repo scenario'
grep -Fq 'codex|scenario=no-git-repo|model=gpt-5.5|skip=yes|ephemeral=yes|sandbox=workspace-write|approval=|color=never|' "$tmp_dir/mock.log" \
  || fail 'codex wrapper did not invoke codex exec with the expected flags'
grep -Fq 'prompt=Prompt:' "$tmp_dir/mock.log" \
  || fail 'codex wrapper did not stream the prompt file to stdin'

MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
  bash "$claude_runner" \
    --claude-bin "$tmp_dir/mock-claude" \
    --model sonnet \
    --case chinese-request \
    --fixtures-dir "$tmp_dir/claude-fixtures" \
    --responses-dir "$tmp_dir/claude-responses" >"$tmp_dir/claude.out"

grep -Fq 'PASS chinese-request' "$tmp_dir/claude.out" \
  || fail 'claude wrapper did not grade the chinese-request scenario'
grep -Fq 'claude|scenario=chinese-request|model=sonnet|print=yes|format=text|permission=dontAsk|bare=yes|persist=no|' "$tmp_dir/mock.log" \
  || fail 'claude wrapper did not invoke claude -p with the expected flags'
grep -Fq 'prompt=/pre-commit-review' "$tmp_dir/mock.log" \
  || fail 'claude wrapper did not pass the rendered prompt text'

printf 'output eval host wrapper tests passed\n'
