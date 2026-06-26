#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
runner="$repo_root/evals/output_eval_runner.sh"
cases_file="$repo_root/evals/output-eval.json"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'output eval runner test failed: %s\n' "$*" >&2
  exit 1
}

fixtures_dir="$tmp_dir/fixtures"
responses_dir="$tmp_dir/responses"
manifest_file="$tmp_dir/manifest.json"

bash "$runner" --fixtures-dir "$fixtures_dir" --responses-dir "$responses_dir" --manifest "$manifest_file" >"$tmp_dir/prepare.out"

[ -d "$fixtures_dir" ] || fail 'fixtures directory not created'
[ -f "$manifest_file" ] || fail 'manifest file not created'
[ -d "$fixtures_dir/output-full-review-split-reducer/workdir" ] || fail 'full review fixture missing workdir'
[ -f "$fixtures_dir/output-pasted-diff/workdir/pasted.patch" ] || fail 'pasted diff fixture missing patch file'

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

printf 'output eval runner tests passed\n'
