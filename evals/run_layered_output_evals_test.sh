#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/evals/run_layered_output_evals.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'run layered output evals test failed: %s\n' "$*" >&2
  exit 1
}

cat >"$tmp_dir/mock-output-eval-runner" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

log_file="${MOCK_LAYERED_LOG_FILE:?}"
printf '%s\n' "$*" >>"$log_file"

eval_file=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --eval-file)
      shift
      eval_file="$1"
      ;;
  esac
  shift
done

case "$eval_file" in
  */routine-output-eval.json|*/advanced-output-eval.json|*/visual-output-eval.json|*/localization-output-eval.json)
    ;;
  *)
    exit 17
    ;;
esac
EOF
chmod +x "$tmp_dir/mock-output-eval-runner"

MOCK_LAYERED_LOG_FILE="$tmp_dir/layered.log" \
  bash "$wrapper" \
    --output-eval-runner "$tmp_dir/mock-output-eval-runner" \
    --runner "echo mock-runner" \
    --case mixed-staged-unstaged \
    --keep-fixtures >"$tmp_dir/out.txt"

[ -f "$tmp_dir/layered.log" ] || fail 'wrapper did not invoke the underlying output eval runner'
[ "$(wc -l <"$tmp_dir/layered.log" | tr -d ' ')" = "4" ] \
  || fail 'wrapper must invoke the output eval runner four times'

grep -Fq 'routine-output-eval.json' "$tmp_dir/layered.log" \
  || fail 'wrapper did not run the routine output eval file'
grep -Fq 'advanced-output-eval.json' "$tmp_dir/layered.log" \
  || fail 'wrapper did not run the advanced output eval file'
grep -Fq 'visual-output-eval.json' "$tmp_dir/layered.log" \
  || fail 'wrapper did not run the visual output eval file'
grep -Fq 'localization-output-eval.json' "$tmp_dir/layered.log" \
  || fail 'wrapper did not run the localization output eval file'
grep -Fq -- '--runner echo mock-runner' "$tmp_dir/layered.log" \
  || fail 'wrapper did not forward the external runner command'
grep -Fq -- '--case mixed-staged-unstaged' "$tmp_dir/layered.log" \
  || fail 'wrapper did not forward the case filter'
grep -Fq -- '--keep-fixtures' "$tmp_dir/layered.log" \
  || fail 'wrapper did not forward additional output-eval-runner flags'

grep -Fq 'routine-output-eval.json' "$tmp_dir/out.txt" \
  || fail 'wrapper output must mention the routine eval file'
grep -Fq 'localization-output-eval.json' "$tmp_dir/out.txt" \
  || fail 'wrapper output must mention the localization eval file'

printf 'run layered output evals tests passed\n'
