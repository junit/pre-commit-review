#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
runner="$script_dir/output_eval_runner.sh"
case_runner="$script_dir/output_eval_codex_case.sh"

codex_bin='codex'
model=''
forward_args=()

usage() {
  cat <<'EOF'
Usage: output_eval_codex_runner.sh [output_eval_runner options] [--codex-bin PATH] [--model MODEL]

Thin Codex wrapper around tests/output_eval_runner.sh. It injects a per-case
runner that links this checkout into .agents/skills/pre-commit-review inside
each fixture workdir before calling `codex exec`.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --codex-bin)
      shift
      [ "$#" -gt 0 ] || {
        printf 'output eval codex runner failed: --codex-bin requires a value\n' >&2
        exit 1
      }
      codex_bin="$1"
      ;;
    --model)
      shift
      [ "$#" -gt 0 ] || {
        printf 'output eval codex runner failed: --model requires a value\n' >&2
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

printf -v runner_command '%q ' bash "$case_runner" --codex-bin "$codex_bin"
if [ -n "$model" ]; then
  printf -v runner_command '%s%q %q ' "$runner_command" --model "$model"
fi

exec bash "$runner" "${forward_args[@]}" --runner "$runner_command"
