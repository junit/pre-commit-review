#!/usr/bin/env bash
# shellcheck disable=SC1091
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"

codex_bin='codex'
model=''

usage() {
  cat <<'EOF'
Usage: output_eval_codex_case.sh [--codex-bin PATH] [--model MODEL]
EOF
}

fail() {
  printf 'output eval codex case failed: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --codex-bin)
      shift
      [ "$#" -gt 0 ] || fail '--codex-bin requires a value'
      codex_bin="$1"
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

if [ ! -x "$codex_bin" ] && ! command -v "$codex_bin" >/dev/null 2>&1; then
  host_eval_taxonomy_fail 'missing-binary' "codex binary not found: $codex_bin"
fi

if git rev-parse --show-toplevel >/dev/null 2>&1; then
  mkdir -p .git/info
  {
    printf '.agents/\n'
    printf '.serena/\n'
  } >>.git/info/exclude
fi

mkdir -p .agents/skills
ln -sfn "$PRE_COMMIT_REVIEW_EVAL_SKILL_DIR" .agents/skills/pre-commit-review

cmd=(
  "$codex_bin"
  exec
  --skip-git-repo-check
  --ephemeral
  --sandbox workspace-write
  --color never
  -o "$PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE"
)

if [ -n "$model" ]; then
  cmd+=(--model "$model")
fi

"${cmd[@]}" - <"$PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE"
