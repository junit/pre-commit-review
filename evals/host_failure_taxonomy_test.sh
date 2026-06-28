#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
claude_case="$repo_root/evals/output_eval_claude_case.sh"
codex_case="$repo_root/evals/output_eval_codex_case.sh"
layered_host_runner="$repo_root/evals/run_layered_host_evals.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'host failure taxonomy test failed: %s\n' "$*" >&2
  exit 1
}

assert_taxonomy_failure() {
  local err_file="$1"
  local expected_type="$2"

  grep -Fq 'host eval failure [' "$err_file" \
    || fail "missing stable failure prefix in $err_file"
  grep -Fq "host eval failure [$expected_type]:" "$err_file" \
    || fail "missing taxonomy token $expected_type"
}

prepare_case_env() {
  local response_file="$1"
  local prompt_file="$2"
  local workdir="$3"

  mkdir -p "$workdir"
  printf 'Prompt\n' >"$prompt_file"
  export PRE_COMMIT_REVIEW_EVAL_SKILL_DIR="$repo_root"
  export PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE="$prompt_file"
  export PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE="$response_file"
}

prepare_case_env "$tmp_dir/missing-binary.md" "$tmp_dir/prompt.txt" "$tmp_dir/workdir"
if (
  cd "$tmp_dir/workdir"
  bash "$claude_case" --claude-bin "$tmp_dir/does-not-exist"
) >"$tmp_dir/missing-binary.out" 2>"$tmp_dir/missing-binary.err"; then
  fail 'missing-binary case must fail'
fi
assert_taxonomy_failure "$tmp_dir/missing-binary.err" 'missing-binary'

if (
  cd "$tmp_dir/workdir"
  bash "$codex_case" --codex-bin "$tmp_dir/does-not-exist"
) >"$tmp_dir/codex-missing-binary.out" 2>"$tmp_dir/codex-missing-binary.err"; then
  fail 'codex missing-binary case must fail'
fi
assert_taxonomy_failure "$tmp_dir/codex-missing-binary.err" 'missing-binary'

if bash "$layered_host_runner" \
  --host claude \
  --claude-runner "$tmp_dir/no-runner.sh" >"$tmp_dir/runner-missing.out" 2>"$tmp_dir/runner-missing.err"; then
  fail 'runner-missing case must fail'
fi
assert_taxonomy_failure "$tmp_dir/runner-missing.err" 'runner-missing'

cat >"$tmp_dir/mock-runner-exit.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 23
EOF
chmod +x "$tmp_dir/mock-runner-exit.sh"

if bash "$repo_root/evals/output_eval_runner.sh" \
  --eval-file "$repo_root/evals/output/routine-output-eval.json" \
  --case tiny-docs \
  --runner "bash $tmp_dir/mock-runner-exit.sh" \
  --fixtures-dir "$tmp_dir/runner-exit-fixtures" \
  --responses-dir "$tmp_dir/runner-exit-responses" >"$tmp_dir/runner-exit.out" 2>"$tmp_dir/runner-exit.err"; then
  fail 'runner-exit-nonzero case must fail'
fi
assert_taxonomy_failure "$tmp_dir/runner-exit.err" 'runner-exit-nonzero'

cat >"$tmp_dir/mock-no-response.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$tmp_dir/mock-no-response.sh"

if bash "$repo_root/evals/output_eval_runner.sh" \
  --eval-file "$repo_root/evals/output/advanced-output-eval.json" \
  --case no-git-repo \
  --runner "bash $tmp_dir/mock-no-response.sh" \
  --fixtures-dir "$tmp_dir/no-response-fixtures" \
  --responses-dir "$tmp_dir/no-response-responses" >"$tmp_dir/no-response.out" 2>"$tmp_dir/no-response.err"; then
  fail 'response-missing case must fail'
fi
assert_taxonomy_failure "$tmp_dir/no-response.err" 'response-missing'

cat >"$tmp_dir/mock-empty-response.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: >"${PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE:?}"
EOF
chmod +x "$tmp_dir/mock-empty-response.sh"

if bash "$repo_root/evals/output_eval_runner.sh" \
  --eval-file "$repo_root/evals/output/advanced-output-eval.json" \
  --case no-git-repo \
  --runner "bash $tmp_dir/mock-empty-response.sh" \
  --fixtures-dir "$tmp_dir/empty-response-fixtures" \
  --responses-dir "$tmp_dir/empty-response-responses" >"$tmp_dir/empty-response.out" 2>"$tmp_dir/empty-response.err"; then
  fail 'response-empty case must fail'
fi
assert_taxonomy_failure "$tmp_dir/empty-response.err" 'response-empty'

cat >"$tmp_dir/mock-protocol-mismatch" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cat <<'OUT'
plain text without required host wrapper framing
OUT
EOF
chmod +x "$tmp_dir/mock-protocol-mismatch"

if MOCK_CLI_LOG_FILE="$tmp_dir/mock.log" \
  bash "$repo_root/evals/output_eval_host_wrappers_test.sh" \
    --claude-bin "$tmp_dir/mock-protocol-mismatch" >"$tmp_dir/protocol-mismatch.out" 2>"$tmp_dir/protocol-mismatch.err"; then
  fail 'protocol-mismatch case must fail'
fi
assert_taxonomy_failure "$tmp_dir/protocol-mismatch.err" 'protocol-mismatch'

printf 'host failure taxonomy tests passed\n'
