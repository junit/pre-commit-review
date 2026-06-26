#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
runner="$script_dir/output_eval_runner.sh"
case_runner="$script_dir/output_eval_claude_case.sh"

claude_bin='claude'
model=''
forward_args=()

usage() {
  cat <<'EOF'
Usage: output_eval_claude_runner.sh [output_eval_runner options] [--claude-bin PATH] [--model MODEL]

Thin Claude Code wrapper around evals/output_eval_runner.sh. It injects a
per-case runner that links this checkout into .claude/skills/pre-commit-review
inside each fixture workdir before calling `claude -p`.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --claude-bin)
      shift
      [ "$#" -gt 0 ] || {
        printf 'output eval claude runner failed: --claude-bin requires a value\n' >&2
        exit 1
      }
      claude_bin="$1"
      ;;
    --model)
      shift
      [ "$#" -gt 0 ] || {
        printf 'output eval claude runner failed: --model requires a value\n' >&2
        exit 1
      }
      model="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      forward_args+=("$1")
      ;;
  esac
  shift
done

printf -v runner_command '%q ' bash "$case_runner" --claude-bin "$claude_bin"
if [ -n "$model" ]; then
  printf -v runner_command '%s%q %q ' "$runner_command" --model "$model"
fi

exec bash "$runner" "${forward_args[@]}" --runner "$runner_command"
