#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
runner="$repo_root/evals/output_eval_runner.sh"
cases_file="$repo_root/evals/output-eval.json"
routine_cases_file="$repo_root/evals/output/routine-output-eval.json"
advanced_cases_file="$repo_root/evals/output/advanced-output-eval.json"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'output eval runner test failed: %s\n' "$*" >&2
  exit 1
}

fixtures_dir="$tmp_dir/fixtures"
responses_dir="$tmp_dir/responses"
manifest_file="$tmp_dir/manifest.json"

bash "$runner" --help >"$tmp_dir/help.out"

grep -Fq -- '--eval-file FILE' "$tmp_dir/help.out" \
  || fail 'runner help must advertise --eval-file'
grep -Fq 'layered eval file such as evals/output/visual-output-eval.json' "$tmp_dir/help.out" \
  || fail 'runner help must explain layered eval-file usage'
grep -Fq -- '--skill-dir DIR' "$tmp_dir/help.out" \
  || fail 'runner help must advertise A/B skill checkout selection'

bash "$runner" --fixtures-dir "$fixtures_dir" --responses-dir "$responses_dir" --manifest "$manifest_file" >"$tmp_dir/prepare.out"

[ -d "$fixtures_dir" ] || fail 'fixtures directory not created'
[ -f "$manifest_file" ] || fail 'manifest file not created'
[ -d "$fixtures_dir/output-full-review-split-reducer/workdir" ] || fail 'full review fixture missing workdir'
[ -f "$fixtures_dir/output-pasted-diff/workdir/pasted.patch" ] || fail 'pasted diff fixture missing patch file'
[ -f "$fixtures_dir/output-static-analysis-evidence/workdir/static-results.sarif" ] \
  || fail 'static-analysis fixture missing SARIF report'
[ -f "$fixtures_dir/output-controlled-static-analysis/trusted-tools/controlled-profile.json" ] \
  || fail 'controlled static-analysis fixture missing execution profile'
[ -x "$fixtures_dir/output-controlled-static-analysis/trusted-tools/controlled-analyzer.py" ] \
  || fail 'controlled static-analysis fixture missing trusted analyzer'
grep -Fq 'Exact profile SHA256:' "$fixtures_dir/output-controlled-static-analysis/prompt.txt" \
  || fail 'controlled static-analysis prompt missing explicit profile authorization'
[ -f "$fixtures_dir/output-static-analysis-orchestration-partial/orchestration-tools/orchestration-manifest.json" ] \
  || fail 'partial orchestration fixture missing manifest'
[ -x "$fixtures_dir/output-static-analysis-orchestration-partial/orchestration-tools/security-analyzer.sh" ] \
  || fail 'partial orchestration fixture missing completed analyzer'
[ -x "$fixtures_dir/output-static-analysis-orchestration-partial/orchestration-tools/timeout-analyzer.sh" ] \
  || fail 'partial orchestration fixture missing timeout analyzer'
grep -Fq 'Exact manifest SHA256:' "$fixtures_dir/output-static-analysis-orchestration-partial/prompt.txt" \
  || fail 'partial orchestration prompt missing explicit manifest authorization'
[ -f "$fixtures_dir/output-controlled-static-analysis-unauthorized/untrusted-until-hash/profile-without-authorizing-hash.json" ] \
  || fail 'unauthorized controlled static-analysis fixture missing profile'
grep -Fq 'No expected profile SHA256 is provided.' \
  "$fixtures_dir/output-controlled-static-analysis-unauthorized/prompt.txt" \
  || fail 'unauthorized controlled static-analysis prompt must omit execution authority'
if grep -Fq 'Exact profile SHA256:' \
  "$fixtures_dir/output-controlled-static-analysis-unauthorized/prompt.txt"; then
  fail 'unauthorized controlled static-analysis prompt accidentally supplied a profile hash'
fi

controlled_workdir="$fixtures_dir/output-controlled-static-analysis/workdir"
controlled_profile="$fixtures_dir/output-controlled-static-analysis/trusted-tools/controlled-profile.json"
controlled_profile_hash="$(python3 - "$controlled_profile" <<'PY'
import hashlib
import pathlib
import sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
)"
controlled_control="$tmp_dir/controlled-control.out"
(
  cd "$controlled_workdir"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$repo_root/scripts/collect_diff_context.sh" --source staged --control-plane
) >"$controlled_control" 2>/dev/null
controlled_fingerprint="$(awk '/^## Review Control Plane JSON$/ { getline; print; exit }' "$controlled_control" | jq -r '.scope_fingerprint')"
(
  cd "$controlled_workdir"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$repo_root/scripts/run_static_analysis.sh" \
      --source staged \
      --expect-scope "$controlled_fingerprint" \
      --profile "$controlled_profile" \
      --expect-profile-sha256 "$controlled_profile_hash"
) >"$tmp_dir/controlled-execution.out" 2>"$tmp_dir/controlled-execution.err"
python3 "$repo_root/scripts/validate_schemas.py" \
  --static-execution-output "$tmp_dir/controlled-execution.out" >/dev/null \
  || fail 'controlled static-analysis eval fixture did not produce valid linked evidence'
jq -e '.counts.blocking_candidates == 1 and .reports[0].trust == "controlled-execution"' \
  < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/controlled-execution.out") >/dev/null \
  || fail 'controlled static-analysis eval fixture did not produce its expected blocking candidate'

orchestration_workdir="$fixtures_dir/output-static-analysis-orchestration-partial/workdir"
orchestration_manifest="$fixtures_dir/output-static-analysis-orchestration-partial/orchestration-tools/orchestration-manifest.json"
orchestration_manifest_hash="$(python3 - "$orchestration_manifest" <<'PY'
import hashlib
import pathlib
import sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
)"
orchestration_control="$tmp_dir/orchestration-control.out"
(
  cd "$orchestration_workdir"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$repo_root/scripts/collect_diff_context.sh" --source staged --control-plane
) >"$orchestration_control" 2>/dev/null
orchestration_fingerprint="$(awk '/^## Review Control Plane JSON$/ { getline; print; exit }' "$orchestration_control" | jq -r '.scope_fingerprint')"
(
  cd "$orchestration_workdir"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$repo_root/scripts/orchestrate_static_analysis.sh" \
      --source staged \
      --expect-scope "$orchestration_fingerprint" \
      --manifest "$orchestration_manifest" \
      --expect-manifest-sha256 "$orchestration_manifest_hash"
) >"$tmp_dir/orchestration-execution.out" 2>"$tmp_dir/orchestration-execution.err"
jq -e '
  .status == "partial"
  and (.runs | length == 2)
  and .runs[0].run_kind == "executed"
  and .runs[0].execution.execution.status == "completed"
  and .runs[0].execution.execution.result_accepted == true
  and .runs[1].run_kind == "executed"
  and .runs[1].execution.execution.status == "timeout"
  and .runs[1].execution.execution.result_accepted == false
' < <(awk '/^## Static Analysis Orchestration JSON$/ { getline; print; exit }' "$tmp_dir/orchestration-execution.out") >/dev/null \
  || fail 'partial orchestration eval fixture did not preserve completed and timeout terminal states'
jq -e '
  .counts.blocking_candidates == 1
  and (.findings | length == 1)
  and .findings[0].rule_id == "SEC-ORCH-EVAL"
  and ([.reports[].status] | index("timeout") != null)
' < <(awk '/^## Static Analysis Evidence JSON$/ { getline; print; exit }' "$tmp_dir/orchestration-execution.out") >/dev/null \
  || fail 'partial orchestration eval fixture did not limit candidates to completed evidence'

jq -e '.fixtures_root != null' "$manifest_file" >/dev/null \
  || fail 'manifest content is invalid'
jq -e '.env.PRE_COMMIT_REVIEW_GROUP_HARD_BYTES == "500"' "$fixtures_dir/output-full-review-split-reducer/metadata.json" >/dev/null \
  || fail 'full-review fixture missing split-budget env metadata'
jq -e '.env.PRE_COMMIT_REVIEW_MAX_DIFF_BYTES == "80"' "$fixtures_dir/output-large-generated/metadata.json" >/dev/null \
  || fail 'large-generated fixture missing diff budget env metadata'

if git -C "$fixtures_dir/output-no-git-repo/workdir" rev-parse --show-toplevel >/dev/null 2>&1; then
  fail 'no-git-repo fixture unexpectedly initialized a git repository'
fi

grep -Fq '```diff' "$fixtures_dir/output-pasted-diff/prompt.txt" \
  || fail 'pasted diff prompt file must embed the provided patch'

while IFS= read -r case_json; do
  [ -n "$case_json" ] || continue
  case_id="$(jq -r '.id' <<<"$case_json")"
  verdict="$(jq -r '.expected.verdict' <<<"$case_json")"
  response_file="$responses_dir/$case_id.md"

  {
    case "$verdict" in
      SAFE_TO_COMMIT|SAFE_TO_COMMIT_WITH_NOTES|DO_NOT_COMMIT)
        printf '**VERDICT:** %s\n' "$verdict"
        ;;
      NO_VERDICT)
        printf 'Diff source: unavailable\n'
        ;;
      CASE_DEPENDENT)
        printf '**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES\n'
        ;;
    esac

    jq -r '.expected.must_include[]' <<<"$case_json"
  } >"$response_file"
done < <(jq -c '.cases[]' "$cases_file")

bash "$runner" --fixtures-dir "$fixtures_dir" --responses-dir "$responses_dir" >"$tmp_dir/grade.out"

grep -Fq 'PASS full-review-split-reducer' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the full-review-split-reducer case'
grep -Fq 'PASS pasted-diff' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the pasted-diff case'
grep -Fq 'PASS static-analysis-evidence' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the static-analysis-evidence case'
grep -Fq 'PASS controlled-static-analysis' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the controlled-static-analysis case'
grep -Fq 'PASS controlled-static-analysis-unauthorized' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the unauthorized controlled-static-analysis case'
grep -Fq 'PASS static-analysis-orchestration-partial' "$tmp_dir/grade.out" \
  || fail 'runner did not grade the partial static-analysis orchestration case'
grep -Fq 'output eval runner completed' "$tmp_dir/grade.out" \
  || fail 'runner did not finish cleanly'

cat >"$responses_dir/output-tiny-docs.md" <<'EOF'
**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
No logic change
🟢 Low
EOF

if bash "$runner" --case tiny-docs --fixtures-dir "$fixtures_dir" --responses-dir "$responses_dir" >"$tmp_dir/mismatch.out" 2>&1; then
  fail 'runner accepted SAFE_TO_COMMIT_WITH_NOTES for a SAFE_TO_COMMIT expectation'
fi
grep -Fq 'expected verdict SAFE_TO_COMMIT but got SAFE_TO_COMMIT_WITH_NOTES' "$tmp_dir/mismatch.out" \
  || fail 'runner did not report the exact verdict mismatch'

cat >"$responses_dir/output-hardcoded-secret.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
redacted
rotate
environment variable

This diff adds serviceToken = "sk_live_1234567890example" directly to source.
EOF

if bash "$runner" --case hardcoded-secret --fixtures-dir "$fixtures_dir" --responses-dir "$responses_dir" >"$tmp_dir/leak.out" 2>&1; then
  fail 'runner accepted a response that leaked the full secret value'
fi
grep -Fq 'forbidden term present for hardcoded-secret' "$tmp_dir/leak.out" \
  || fail 'runner did not report the forbidden-term secret leak'

layered_fixtures_dir="$tmp_dir/layered-fixtures"
layered_responses_dir="$tmp_dir/layered-responses"
layered_manifest_file="$tmp_dir/layered-manifest.json"

bash "$runner" \
  --eval-file "$routine_cases_file" \
  --fixtures-dir "$layered_fixtures_dir" \
  --responses-dir "$layered_responses_dir" \
  --manifest "$layered_manifest_file" >"$tmp_dir/layered.out"

[ -d "$layered_fixtures_dir/routine-tiny-docs-en/workdir" ] \
  || fail 'runner did not prepare layered routine tiny-docs fixture'
[ -f "$layered_manifest_file" ] \
  || fail 'runner did not write manifest for layered eval file'
grep -Fq 'PREPARED tiny-docs' "$tmp_dir/layered.out" \
  || fail 'runner did not prepare tiny-docs from the layered eval file'

advanced_fixtures_dir="$tmp_dir/advanced-fixtures"
advanced_responses_dir="$tmp_dir/advanced-responses"

bash "$runner" \
  --eval-file "$advanced_cases_file" \
  --case auth-execution-point \
  --fixtures-dir "$advanced_fixtures_dir" \
  --responses-dir "$advanced_responses_dir" >"$tmp_dir/advanced-auth.out"

[ -f "$advanced_fixtures_dir/advanced-auth-execution-point-en/workdir/src/auth.ts" ] \
  || fail 'runner did not prepare auth execution-point fixture'
grep -Fq 'PREPARED auth-execution-point' "$tmp_dir/advanced-auth.out" \
  || fail 'runner did not report auth execution-point preparation'

bash "$runner" \
  --eval-file "$advanced_cases_file" \
  --case negative-search-cross-module \
  --fixtures-dir "$advanced_fixtures_dir" \
  --responses-dir "$advanced_responses_dir" >"$tmp_dir/advanced-negative.out"

[ -f "$advanced_fixtures_dir/advanced-negative-search-cross-module-en/workdir/src/session/create.ts" ] \
  || fail 'runner did not prepare negative-search cross-module fixture'
grep -Fq 'PREPARED negative-search-cross-module' "$tmp_dir/advanced-negative.out" \
  || fail 'runner did not report negative-search cross-module preparation'

bash "$runner" \
  --eval-file "$advanced_cases_file" \
  --case framework-behavior-source \
  --fixtures-dir "$advanced_fixtures_dir" \
  --responses-dir "$advanced_responses_dir" >"$tmp_dir/advanced-framework.out"

[ -f "$advanced_fixtures_dir/advanced-framework-behavior-source-en/workdir/vendor/acme-orm/optimistic-lock.md" ] \
  || fail 'runner did not prepare framework behavior source fixture'
grep -Fq 'PREPARED framework-behavior-source' "$tmp_dir/advanced-framework.out" \
  || fail 'runner did not report framework behavior source preparation'

attention_fixtures_dir="$tmp_dir/attention-fixtures"
attention_responses_dir="$tmp_dir/attention-responses"
mkdir -p "$attention_responses_dir"
cat >"$attention_responses_dir/advanced-independent-findings-enumeration-en.md" <<'EOF'
**VERDICT:** DO_NOT_COMMIT
The credential is redacted; rotate it.
grantAdmin lacks authorization.
email remains mentioned by getUserProfile, which is a breaking change for downstream clients.
EOF

if bash "$runner" \
  --eval-file "$advanced_cases_file" \
  --case independent-findings-enumeration \
  --fixtures-dir "$attention_fixtures_dir" \
  --responses-dir "$attention_responses_dir" >"$tmp_dir/attention-missing.out" 2>&1; then
  fail 'runner accepted a response that omitted the destructive migration finding'
fi
grep -Fq 'missing non-secret finding for independent-findings-enumeration: destructive-migration' \
  "$tmp_dir/attention-missing.out" \
  || fail 'runner did not report the missing non-secret finding dimension'

printf '%s\n' 'The migration will drop persisted email data.' \
  >>"$attention_responses_dir/advanced-independent-findings-enumeration-en.md"
bash "$runner" \
  --eval-file "$advanced_cases_file" \
  --case independent-findings-enumeration \
  --fixtures-dir "$attention_fixtures_dir" \
  --responses-dir "$attention_responses_dir" >"$tmp_dir/attention-complete.out"
grep -Fq 'PASS independent-findings-enumeration' "$tmp_dir/attention-complete.out" \
  || fail 'runner did not accept complete non-secret finding recall'

custom_skill_dir="$tmp_dir/custom-skill"
custom_skill_fixtures="$tmp_dir/custom-skill-fixtures"
mkdir -p "$custom_skill_dir"
bash "$runner" \
  --eval-file "$routine_cases_file" \
  --case tiny-docs \
  --skill-dir "$custom_skill_dir" \
  --fixtures-dir "$custom_skill_fixtures" \
  --responses-dir "$tmp_dir/custom-skill-responses" >"$tmp_dir/custom-skill.out"

jq -e --arg expected "$custom_skill_dir" '.skill_dir == $expected' \
  "$custom_skill_fixtures/routine-tiny-docs-en/metadata.json" >/dev/null \
  || fail 'runner did not persist the selected A/B skill directory'

printf 'output eval runner tests passed\n'
