#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
comparator="$script_dir/compare_output_eval_quality.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'output eval quality comparison test failed: %s\n' "$*" >&2
  exit 1
}

baseline_dir="$tmp_dir/baseline"
current_dir="$tmp_dir/current"
mkdir -p "$baseline_dir" "$current_dir"

jq -s '{
  evaluation_method: "quality comparison fixture",
  cases: (
    [.[0].cases[] | select(.id == "routine-tiny-docs-en" or .id == "routine-hardcoded-secret-en")]
    + [.[1].cases[] | select(.id == "advanced-independent-findings-enumeration-en")]
  )
}' \
  "$script_dir/output/routine-output-eval.json" \
  "$script_dir/output/advanced-output-eval.json" >"$tmp_dir/eval.json"

cat >"$baseline_dir/routine-tiny-docs-en.md" <<'EOF'
**VERDICT:** SAFE_TO_COMMIT
No logic change
🟢 Low
EOF

cat >"$baseline_dir/routine-hardcoded-secret-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
redacted
rotate
environment variable
The diff contains sk_live_1234567890example.
EOF

cat >"$current_dir/routine-tiny-docs-en.md" <<'EOF'
**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
No logic change
🟢 Low
EOF

cat >"$current_dir/routine-hardcoded-secret-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
redacted
rotate
environment variable
EOF

cat >"$baseline_dir/advanced-independent-findings-enumeration-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
serviceToken is redacted; rotate it.
grantAdmin lacks authorization.
The migration executes drop column email.
getUserProfile is a breaking change for downstream clients.
EOF

cat >"$current_dir/advanced-independent-findings-enumeration-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
serviceToken is redacted; rotate it.
grantAdmin lacks authorization.
The migration executes drop column email.
EOF

regression_report="$tmp_dir/regression.json"
if bash "$comparator" \
  --baseline-responses "$baseline_dir" \
  --current-responses "$current_dir" \
  --eval-file "$tmp_dir/eval.json" \
  --report-json "$regression_report" >"$tmp_dir/regression.out" 2>"$tmp_dir/regression.err"; then
  fail 'comparator accepted a current regression'
fi

jq -e '
  .schema_version == "output-eval-quality-diff/v1"
  and .overall_status == "regression"
  and (.regressions | map(.case_id) == ["routine-tiny-docs-en", "advanced-independent-findings-enumeration-en"])
  and (.secret_attention_regressions | map(.case_id) == ["advanced-independent-findings-enumeration-en"])
  and (.secret_attention_regressions[0].secret_attention.baseline_recalled == 3)
  and (.secret_attention_regressions[0].secret_attention.current_recalled == 2)
  and (.improvements | map(.case_id) == ["routine-hardcoded-secret-en"])
' "$regression_report" >/dev/null || fail 'regression report changed'

cat >"$current_dir/routine-tiny-docs-en.md" <<'EOF'
**VERDICT:** SAFE_TO_COMMIT
No logic change
🟢 Low
EOF

cat >"$current_dir/advanced-independent-findings-enumeration-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
serviceToken is redacted; rotate it.
grantAdmin lacks authorization.
The migration executes drop column email.
getUserProfile is a breaking change for downstream clients.
EOF

passing_report="$tmp_dir/passing.json"
bash "$comparator" \
  --baseline-responses "$baseline_dir" \
  --current-responses "$current_dir" \
  --eval-file "$tmp_dir/eval.json" \
  --report-json "$passing_report" >"$tmp_dir/passing.out"

jq -e '
  .overall_status == "no-regression"
  and .baseline.passed == 2
  and .current.passed == 3
  and (.regressions | length) == 0
  and (.secret_attention_regressions | length) == 0
  and (.cases[] | select(.case_id == "advanced-independent-findings-enumeration-en") | .secret_attention.current_recalled == 3)
  and (.improvements | map(.case_id) == ["routine-hardcoded-secret-en"])
' "$passing_report" >/dev/null || fail 'no-regression report changed'

rm "$current_dir/routine-hardcoded-secret-en.md"
incomplete_report="$tmp_dir/incomplete.json"
if bash "$comparator" \
  --baseline-responses "$baseline_dir" \
  --current-responses "$current_dir" \
  --eval-file "$tmp_dir/eval.json" \
  --report-json "$incomplete_report" >"$tmp_dir/incomplete.out" 2>"$tmp_dir/incomplete.err"; then
  fail 'comparator accepted an incomplete current response set'
fi

jq -e '
  .overall_status == "incomplete"
  and (.incomplete_cases | map(.case_id) == ["routine-hardcoded-secret-en"])
' "$incomplete_report" >/dev/null || fail 'incomplete report changed'

printf 'output eval quality comparison tests passed\n'
