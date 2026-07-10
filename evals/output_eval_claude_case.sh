#!/usr/bin/env bash
# shellcheck disable=SC1091,SC2016
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"

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

if [ ! -x "$claude_bin" ] && ! command -v "$claude_bin" >/dev/null 2>&1; then
  host_eval_taxonomy_fail 'missing-binary' "claude binary not found: $claude_bin"
fi

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
  printf '/pre-commit-review\n\nFollow the skill instructions exactly. Start your output IMMEDIATELY with `# Pre-Commit Review` or `**VERDICT:**`. Do NOT output any preamble, rule explanations, or `★ Insight` blocks. Use the required output template and keep labels such as `VERDICT`, `差异来源`, `审查范围`, `变更规模`, and `建议验证` verbatim when they apply.\n\n%s' \
    "$(cat "$PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE")"
)"

cmd=(
  "$claude_bin"
  -p
  --output-format text
  --permission-mode dontAsk
  --no-session-persistence
)

if [ -n "$model" ]; then
  cmd+=(--model "$model")
fi

"${cmd[@]}" "$prompt_text" >"$PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE"
