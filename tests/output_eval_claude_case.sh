#!/usr/bin/env bash
set -euo pipefail

claude_bin='claude'
model=''

usage() {
  cat <<'EOF'
Usage: output_eval_claude_case.sh [--claude-bin PATH] [--model MODEL]
EOF
}

fail() {
  printf 'output eval claude case failed: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --claude-bin)
      shift
      [ "$#" -gt 0 ] || fail '--claude-bin requires a value'
      claude_bin="$1"
      ;;
    --model)
      shift
      [ "$#" -gt 0 ] || fail '--model requires a value'
      model="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

[ -n "${PRE_COMMIT_REVIEW_EVAL_SKILL_DIR:-}" ] || fail 'PRE_COMMIT_REVIEW_EVAL_SKILL_DIR is required'
[ -n "${PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE:-}" ] || fail 'PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE is required'
[ -n "${PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE:-}" ] || fail 'PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE is required'

require_command "$claude_bin"

if git rev-parse --show-toplevel >/dev/null 2>&1; then
  mkdir -p .git/info
  {
    printf '.claude/\n'
    printf '.serena/\n'
  } >>.git/info/exclude
fi

mkdir -p .claude/skills
ln -sfn "$PRE_COMMIT_REVIEW_EVAL_SKILL_DIR" .claude/skills/pre-commit-review

prompt_text="$(
  printf '/pre-commit-review\n\nFollow the skill instructions exactly. Use the required output template and keep labels such as `VERDICT`, `差异来源`, `审查范围`, `变更规模`, and `建议验证` verbatim when they apply.\n\n%s' \
    "$(cat "$PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE")"
)"

cmd=(
  "$claude_bin"
  -p
  --output-format text
  --permission-mode dontAsk
  --bare
  --no-session-persistence
)

if [ -n "$model" ]; then
  cmd+=(--model "$model")
fi

"${cmd[@]}" "$prompt_text" >"$PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE"
