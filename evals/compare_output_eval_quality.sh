#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
output_eval_runner="$script_dir/output_eval_runner.sh"

baseline_responses=''
current_responses=''
report_json=''
custom_eval_files='no'
eval_files=()
default_eval_files=(
  "$script_dir/output/routine-output-eval.json"
  "$script_dir/output/advanced-output-eval.json"
  "$script_dir/output/visual-output-eval.json"
  "$script_dir/output/localization-output-eval.json"
)

usage() {
  cat <<'EOF'
Usage: compare_output_eval_quality.sh [options]

Grade saved baseline and current model responses against the same output-eval
cases, then write a machine-readable regression report. This command never
invokes a model.

Options:
  --baseline-responses DIR  Required directory containing baseline <case-id>.md files
  --current-responses DIR   Required directory containing current <case-id>.md files
  --report-json FILE        Required path for the comparison report
  --eval-file FILE          Compare one eval file; repeat to compare multiple files
                            Defaults to all four layered output eval files
  -h, --help                Show this help
EOF
}

fail() {
  printf 'output eval quality comparison failed: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --baseline-responses)
      shift
      [ "$#" -gt 0 ] || fail '--baseline-responses requires a value'
      baseline_responses="$1"
      ;;
    --current-responses)
      shift
      [ "$#" -gt 0 ] || fail '--current-responses requires a value'
      current_responses="$1"
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json="$1"
      ;;
    --eval-file)
      shift
      [ "$#" -gt 0 ] || fail '--eval-file requires a value'
      if [ "$custom_eval_files" = 'no' ]; then
        eval_files=()
        custom_eval_files='yes'
      fi
      eval_files+=("$1")
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

command -v jq >/dev/null 2>&1 || fail 'missing required command: jq'
[ -f "$output_eval_runner" ] || fail "missing output eval runner: $output_eval_runner"
[ -n "$baseline_responses" ] || fail '--baseline-responses is required'
[ -n "$current_responses" ] || fail '--current-responses is required'
[ -n "$report_json" ] || fail '--report-json is required'
[ -d "$baseline_responses" ] || fail "baseline responses directory does not exist: $baseline_responses"
[ -d "$current_responses" ] || fail "current responses directory does not exist: $current_responses"

if [ "$custom_eval_files" = 'no' ]; then
  eval_files=("${default_eval_files[@]}")
fi

for eval_file in "${eval_files[@]}"; do
  [ -f "$eval_file" ] || fail "eval file does not exist: $eval_file"
  jq -e '.cases | type == "array" and length > 0' "$eval_file" >/dev/null \
    || fail "eval file has no cases: $eval_file"
done

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
results_jsonl="$tmp_dir/results.jsonl"
: >"$results_jsonl"

grade_response() {
  local side="$1"
  local responses_dir="$2"
  local case_id="$3"
  local single_eval_file="$4"
  local response_file="$responses_dir/$case_id.md"
  local fixtures_dir="$tmp_dir/fixtures-$side-$case_index"

  if [ ! -s "$response_file" ]; then
    printf 'missing\n'
    return
  fi

  if bash "$output_eval_runner" \
    --eval-file "$single_eval_file" \
    --responses-dir "$responses_dir" \
    --fixtures-dir "$fixtures_dir" >"$tmp_dir/$side-$case_index.out" 2>"$tmp_dir/$side-$case_index.err"; then
    printf 'pass\n'
  else
    printf 'fail\n'
  fi
}

count_recalled_findings() {
  local response_file="$1"
  local case_json="$2"
  local finding_json term matched recalled=0

  if [ ! -s "$response_file" ]; then
    printf '0\n'
    return
  fi

  while IFS= read -r finding_json; do
    [ -n "$finding_json" ] || continue
    matched='yes'
    while IFS= read -r term; do
      [ -n "$term" ] || continue
      if ! grep -Fiq -- "$term" "$response_file"; then
        matched='no'
        break
      fi
    done < <(jq -r '.must_include[]' <<<"$finding_json")
    if [ "$matched" = 'yes' ]; then
      recalled=$((recalled + 1))
    fi
  done < <(jq -c '.expected.quality_dimensions.secret_attention.non_secret_findings[]?' <<<"$case_json")

  printf '%s\n' "$recalled"
}

build_attention_metrics() {
  local case_json="$1"
  local baseline_response="$2"
  local current_response="$3"
  local total baseline_recalled current_recalled require_full allow_drop

  total="$(jq -r '.expected.quality_dimensions.secret_attention.non_secret_findings // [] | length' <<<"$case_json")"
  if [ "$total" -eq 0 ]; then
    printf 'null\n'
    return
  fi

  baseline_recalled="$(count_recalled_findings "$baseline_response" "$case_json")"
  current_recalled="$(count_recalled_findings "$current_response" "$case_json")"
  require_full="$(jq -r '.expected.quality_dimensions.secret_attention.require_current_full_recall // false' <<<"$case_json")"
  allow_drop="$(jq -r '.expected.quality_dimensions.secret_attention.allow_recall_drop // false' <<<"$case_json")"

  jq -cn \
    --argjson total "$total" \
    --argjson baseline_recalled "$baseline_recalled" \
    --argjson current_recalled "$current_recalled" \
    --argjson require_full "$require_full" \
    --argjson allow_drop "$allow_drop" \
    '{
      non_secret_finding_count: $total,
      baseline_recalled: $baseline_recalled,
      current_recalled: $current_recalled,
      recall_delta: ($current_recalled - $baseline_recalled),
      require_current_full_recall: $require_full,
      allow_recall_drop: $allow_drop,
      regression: (
        (($allow_drop | not) and ($current_recalled < $baseline_recalled))
        or ($require_full and ($current_recalled < $total))
      )
    }'
}

case_index=0
for eval_file in "${eval_files[@]}"; do
  while IFS= read -r case_json; do
    [ -n "$case_json" ] || continue
    case_index=$((case_index + 1))
    case_id="$(jq -r '.id' <<<"$case_json")"
    scenario="$(jq -r '.scenario' <<<"$case_json")"
    [ -n "$case_id" ] && [ "$case_id" != 'null' ] || fail "case without id in $eval_file"
    [ -n "$scenario" ] && [ "$scenario" != 'null' ] || fail "case without scenario in $eval_file"

    single_eval_file="$tmp_dir/case-$case_index.json"
    jq -n --argjson case "$case_json" \
      '{evaluation_method: "single-case quality comparison", cases: [$case]}' \
      >"$single_eval_file"

    baseline_status="$(grade_response baseline "$baseline_responses" "$case_id" "$single_eval_file")"
    current_status="$(grade_response current "$current_responses" "$case_id" "$single_eval_file")"
    attention_metrics="$(build_attention_metrics \
      "$case_json" \
      "$baseline_responses/$case_id.md" \
      "$current_responses/$case_id.md")"

    if [ "$baseline_status" = 'missing' ] || [ "$current_status" = 'missing' ]; then
      change='incomplete'
    elif [ "$baseline_status" = 'pass' ] && [ "$current_status" != 'pass' ]; then
      change='regression'
    elif [ "$baseline_status" != 'pass' ] && [ "$current_status" = 'pass' ]; then
      change='improvement'
    elif [ "$baseline_status" = 'pass' ]; then
      change='unchanged-pass'
    else
      change='unchanged-fail'
    fi

    jq -cn \
      --arg case_id "$case_id" \
      --arg scenario "$scenario" \
      --arg eval_file "$(basename "$eval_file")" \
      --arg baseline "$baseline_status" \
      --arg current "$current_status" \
      --arg change "$change" \
      --argjson attention "$attention_metrics" \
      '{
        case_id: $case_id,
        scenario: $scenario,
        eval_file: $eval_file,
        baseline: $baseline,
        current: $current,
        change: $change,
        secret_attention: $attention
      }' >>"$results_jsonl"
  done < <(jq -c '.cases[]' "$eval_file")
done

[ "$case_index" -gt 0 ] || fail 'no eval cases were compared'
mkdir -p "$(dirname -- "$report_json")"
jq -s '
  . as $cases
  | {
      schema_version: "output-eval-quality-diff/v1",
      case_count: ($cases | length),
      baseline: {
        passed: ($cases | map(select(.baseline == "pass")) | length),
        failed: ($cases | map(select(.baseline == "fail")) | length),
        missing: ($cases | map(select(.baseline == "missing")) | length)
      },
      current: {
        passed: ($cases | map(select(.current == "pass")) | length),
        failed: ($cases | map(select(.current == "fail")) | length),
        missing: ($cases | map(select(.current == "missing")) | length)
      },
      regressions: ($cases | map(select(.change == "regression"))),
      secret_attention_regressions: ($cases | map(select(.secret_attention.regression == true))),
      improvements: ($cases | map(select(.change == "improvement"))),
      unchanged_failures: ($cases | map(select(.change == "unchanged-fail"))),
      incomplete_cases: ($cases | map(select(.change == "incomplete"))),
      cases: $cases
    }
  | .overall_status = (
      if (.incomplete_cases | length) > 0 then "incomplete"
      elif (.regressions | length) > 0 or (.secret_attention_regressions | length) > 0 then "regression"
      else "no-regression"
      end
    )
' "$results_jsonl" >"$report_json"

overall_status="$(jq -r '.overall_status' "$report_json")"
printf 'output eval quality comparison: %s (%s cases, %s regressions, %s secret-attention regressions, %s improvements)\n' \
  "$overall_status" \
  "$(jq -r '.case_count' "$report_json")" \
  "$(jq -r '.regressions | length' "$report_json")" \
  "$(jq -r '.secret_attention_regressions | length' "$report_json")" \
  "$(jq -r '.improvements | length' "$report_json")"

[ "$overall_status" = 'no-regression' ] \
  || fail "comparison status is $overall_status; see $report_json"
