#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
codex_runner="$repo_root/evals/output_eval_codex_runner.sh"
claude_runner="$repo_root/evals/output_eval_claude_runner.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
codex_bin=''
claude_bin=''
protocol_mode='no'

fail() {
  printf 'output eval host wrapper test failed: %s\n' "$*" >&2
  exit 1
}

host_protocol_fail() {
  printf 'host eval failure [protocol-mismatch]: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --codex-bin)
      shift
      [ "$#" -gt 0 ] || fail '--codex-bin requires a value'
      codex_bin="$1"
      ;;
    --claude-bin)
      shift
      [ "$#" -gt 0 ] || fail '--claude-bin requires a value'
      claude_bin="$1"
      protocol_mode='yes'
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

if [ -z "$codex_bin" ]; then
  codex_bin="$tmp_dir/mock-codex"
  cat >"$codex_bin" <<'EOF'
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
  chmod +x "$codex_bin"
fi

cat >"$tmp_dir/mock-claude-good" <<'EOF'
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
chmod +x "$tmp_dir/mock-claude-good"

cat >"$tmp_dir/mock-claude-bad-output" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cat <<'OUT'
plain text without required review framing
OUT
EOF
chmod +x "$tmp_dir/mock-claude-bad-output"

if [ -z "$claude_bin" ]; then
  claude_bin="$tmp_dir/mock-claude-good"
fi

MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
  bash "$codex_runner" \
    --codex-bin "$codex_bin" \
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

if [ "$claude_bin" = "$tmp_dir/mock-claude-good" ]; then
  MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
    bash "$claude_runner" \
      --claude-bin "$claude_bin" \
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

  claude_bin="$tmp_dir/mock-claude-bad-output"
fi

if MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
  bash "$claude_runner" \
    --claude-bin "$claude_bin" \
    --model sonnet \
    --case chinese-request \
    --fixtures-dir "$tmp_dir/claude-bad-fixtures" \
    --responses-dir "$tmp_dir/claude-bad-responses" >"$tmp_dir/claude-bad.out" 2>"$tmp_dir/claude-bad.err"; then
  host_protocol_fail 'claude wrapper accepted shape-incompatible output'
fi

grep -Eq 'missing expected verdict token|expected verdict .* but got <none>|failed must_include checks|failed must_not_include check' "$tmp_dir/claude-bad.err" \
  || {
    if [ -f "$tmp_dir/claude-bad.err" ]; then
      cat "$tmp_dir/claude-bad.err" >&2
    fi
    host_protocol_fail 'claude wrapper rejected output for an unexpected reason'
  }

if [ "$protocol_mode" = 'yes' ]; then
  host_protocol_fail "claude wrapper rejected shape-incompatible output from $claude_bin"
fi

printf 'output eval host wrapper tests passed\n'
