#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
output_eval_runner="$script_dir/output_eval_runner.sh"

eval_files=(
  "$script_dir/output/routine-output-eval.json"
  "$script_dir/output/advanced-output-eval.json"
  "$script_dir/output/visual-output-eval.json"
  "$script_dir/output/localization-output-eval.json"
)

forward_args=()

usage() {
  cat <<'EOF'
Usage: run_layered_output_evals.sh [options passed through to output_eval_runner.sh]

Run the layered output eval matrix by invoking evals/output_eval_runner.sh once for
each of:
  - evals/output/routine-output-eval.json
  - evals/output/advanced-output-eval.json
  - evals/output/visual-output-eval.json
  - evals/output/localization-output-eval.json

Options:
  --output-eval-runner PATH   Override the underlying output eval runner script
  -h, --help                  Show this help

All other arguments are forwarded to evals/output_eval_runner.sh unchanged.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --output-eval-runner)
      shift
      [ "$#" -gt 0 ] || {
        printf 'run layered output evals failed: --output-eval-runner requires a value\n' >&2
        exit 1
      }
      output_eval_runner="$1"
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

[ -f "$output_eval_runner" ] || {
  printf 'run layered output evals failed: missing output eval runner: %s\n' "$output_eval_runner" >&2
  exit 1
}

for eval_file in "${eval_files[@]}"; do
  [ -f "$eval_file" ] || {
    printf 'run layered output evals failed: missing eval file: %s\n' "$eval_file" >&2
    exit 1
  }

  printf '=== Running %s ===\n' "$(basename "$eval_file")"
  bash "$output_eval_runner" --eval-file "$eval_file" "${forward_args[@]}"
done
