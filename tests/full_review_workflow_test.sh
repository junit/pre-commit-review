#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
helper="$repo_root/scripts/collect_diff_context.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'full review workflow test failed: %s\n' "$*" >&2
  exit 1
}

extract_section() {
  local file="$1"
  local section="$2"
  local output="$3"

  awk -v section="## $section" '
    $0 == section { in_section=1; next }
    in_section && /^## / { exit }
    in_section { print }
  ' "$file" >"$output"

  [ -s "$output" ] || fail "missing section: $section"
}

json_array_from_lines() {
  local input="$1"
  local output="$2"

  if [ ! -s "$input" ]; then
    printf '[]\n' >"$output"
    return 0
  fi

  jq -R . "$input" | jq -s . >"$output"
}

init_repo() {
  local dir="$1"

  git -C "$dir" init -q
  git -C "$dir" config user.email a@example.com
  git -C "$dir" config user.name A
  printf 'baseline\n' >"$dir/base.txt"
  git -C "$dir" add base.txt
  git -C "$dir" commit -q -m init
}

fixture_repo="$tmp_dir/full-review-fixture"
mkdir -p "$fixture_repo/snapshots" "$fixture_repo/zzz_auth" "$fixture_repo/db/migrations"
init_repo "$fixture_repo"

for i in $(seq 1 80); do
  printf 'snapshot line %s\n' "$i"
done >"$fixture_repo/snapshots/large.snap"
printf 'def allow(user):\n    return True\n' >"$fixture_repo/zzz_auth/session.py"
printf 'alter table legacy_users drop column password_hash;\n' >"$fixture_repo/db/migrations/20260518_drop_legacy_users.sql"
git -C "$fixture_repo" add snapshots/large.snap zzz_auth/session.py db/migrations/20260518_drop_legacy_users.sql

helper_output="$tmp_dir/full-review.out"
(
  cd "$fixture_repo"
  PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES=200 PRE_COMMIT_REVIEW_GROUP_HARD_BYTES=500 "$helper"
) >"$helper_output" 2>&1

review_plan_json="$tmp_dir/review-plan.json"
state_template_json="$tmp_dir/reducer-state-template.json"
final_template_json="$tmp_dir/reducer-final-template.json"
split_suggestions_tsv="$tmp_dir/split-suggestions.tsv"
coverage_ledger_tsv="$tmp_dir/coverage-ledger.tsv"

extract_section "$helper_output" 'Review Plan JSON' "$review_plan_json"
extract_section "$helper_output" 'Reducer State Snapshot Template' "$state_template_json"
extract_section "$helper_output" 'Reducer Finalization Template' "$final_template_json"
extract_section "$helper_output" 'Split Suggestions' "$split_suggestions_tsv"
extract_section "$helper_output" 'Coverage Ledger Template' "$coverage_ledger_tsv"

jq -e '
  [.groups[].group_id] == ["consistency-snapshots", "high-risk-db-migrations", "high-risk-zzz_auth"]
  and .groups[0].action == "split"
  and .groups[1].budget_status == "over-target"
  and .groups[2].action == "review"
' "$review_plan_json" >/dev/null \
  || fail 'unexpected review plan order or actions'

jq -e '
  .state_kind == "reducer_state_snapshot"
  and .needs_split_units == ["file:snapshots/large.snap"]
  and ((.pending_units | sort) == (["file:snapshots/large.snap", "file:db/migrations/20260518_drop_legacy_users.sql", "file:zzz_auth/session.py"] | sort))
' "$state_template_json" >/dev/null \
  || fail 'unexpected reducer state snapshot template'

split_units_txt="$tmp_dir/split-units.txt"
awk -F "$(printf '\t')" 'NR > 1 && $1 == "consistency-snapshots" { print $2 }' "$split_suggestions_tsv" >"$split_units_txt"
[ -s "$split_units_txt" ] || fail 'expected split suggestions for consistency-snapshots'

non_split_units_txt="$tmp_dir/non-split-units.txt"
awk -F "$(printf '\t')" 'NR > 1 && $4 != "needs-split" { print $1 }' "$coverage_ledger_tsv" >"$non_split_units_txt"
[ -s "$non_split_units_txt" ] || fail 'expected non-split coverage units'

all_reviewed_units_txt="$tmp_dir/all-reviewed-units.txt"
cat "$non_split_units_txt" "$split_units_txt" | sort -u >"$all_reviewed_units_txt"

split_units_json="$tmp_dir/split-units.json"
non_split_units_json="$tmp_dir/non-split-units.json"
all_reviewed_units_json="$tmp_dir/all-reviewed-units.json"
json_array_from_lines "$split_units_txt" "$split_units_json"
json_array_from_lines "$non_split_units_txt" "$non_split_units_json"
json_array_from_lines "$all_reviewed_units_txt" "$all_reviewed_units_json"

migration_unit="$(awk 'NR == 1 { print; exit }' "$non_split_units_txt")"
auth_unit="$(awk 'NR == 2 { print; exit }' "$non_split_units_txt")"
[ "$migration_unit" = 'file:db/migrations/20260518_drop_legacy_users.sql' ] || fail 'expected migration unit first'
[ "$auth_unit" = 'file:zzz_auth/session.py' ] || fail 'expected auth unit second'

group_results_json="$tmp_dir/group-results.json"
jq -n \
  --slurpfile split_units "$split_units_json" \
  --arg migration_unit "$migration_unit" \
  --arg auth_unit "$auth_unit" \
  '
  [
    {
      "group_id": "consistency-snapshots",
      "required_units": $split_units[0],
      "reviewed_units": $split_units[0],
      "coverage": "full",
      "findings": [],
      "contract_changes": [],
      "dependencies_to_check": [],
      "tests_recommended": ["regenerate the snapshot source and verify reproducibility"]
    },
    {
      "group_id": "high-risk-db-migrations",
      "required_units": [$migration_unit],
      "reviewed_units": [$migration_unit],
      "coverage": "full",
      "findings": [
        {
          "title": "Drop column without compatibility handling",
          "impact": "Existing readers can fail after the migration removes password_hash.",
          "fix": "Gate the rollout and remove readers before dropping the column."
        }
      ],
      "contract_changes": ["schema: drop legacy_users.password_hash"],
      "dependencies_to_check": ["callers or jobs that still read password_hash"],
      "tests_recommended": ["run migration compatibility coverage before commit"]
    },
    {
      "group_id": "high-risk-zzz_auth",
      "required_units": [$auth_unit],
      "reviewed_units": [$auth_unit],
      "coverage": "full",
      "findings": [],
      "contract_changes": [],
      "dependencies_to_check": [],
      "tests_recommended": ["run auth regression coverage"]
    }
  ]
  ' >"$group_results_json"

blockers_json="$tmp_dir/blockers.json"
jq -n '
  [
    {
      "title": "Drop column without compatibility handling",
      "evidence": "alter table legacy_users drop column password_hash;",
      "impact": "The migration is destructive and can break consumers that still read password_hash.",
      "fix": "Stage a compatibility rollout before removing the column."
    }
  ]
' >"$blockers_json"

final_state_json="$tmp_dir/final-reducer-state.json"
jq \
  --slurpfile reviewed "$all_reviewed_units_json" \
  --slurpfile group_results "$group_results_json" \
  --slurpfile blockers "$blockers_json" \
  '
  .status = "coverage_validated"
  | .reviewed_units = $reviewed[0]
  | .pending_units = []
  | .needs_split_units = []
  | .group_results = $group_results[0]
  | .coverage_gaps = []
  | .finding_merge.deduplicated_findings = $blockers[0]
  | .finding_merge.blockers = $blockers[0]
  | .finding_merge.notes = ["split-required snapshot group was replaced with split hunk results before reducer finalization"]
  | .dependency_checks = ["schema drop requires compatibility rollout review"]
  | .test_recommendations = ["run migration compatibility coverage before commit", "verify snapshot regeneration is reproducible"]
  | .final_verdict = "DO_NOT_COMMIT"
  ' "$state_template_json" >"$final_state_json"

jq -e '
  .status == "coverage_validated"
  and .pending_units == []
  and .needs_split_units == []
  and .coverage_gaps == []
  and (.group_results | length) == 3
  and (.group_results[] | select(.group_id == "consistency-snapshots") | all(.required_units[]; startswith("hunk:snapshots/large.snap:")))
  and (.group_results[] | select(.group_id == "consistency-snapshots") | .required_units == .reviewed_units)
  and (.finding_merge.blockers[0].title | contains("Drop column"))
  and .final_verdict == "DO_NOT_COMMIT"
' "$final_state_json" >/dev/null \
  || fail 'final reducer state does not satisfy end-to-end workflow expectations'

reducer_final_json="$tmp_dir/reducer-final.json"
jq \
  --slurpfile blockers "$blockers_json" \
  '
  .coverage_validation = "passed"
  | .coverage_gaps = []
  | .finding_merge.deduplicated_findings = $blockers[0]
  | .finding_merge.blockers = $blockers[0]
  | .finding_merge.notes = ["coverage-led review completed after split-unit replacement"]
  | .dependency_checks = ["schema drop requires compatibility rollout review"]
  | .test_recommendations = ["run migration compatibility coverage before commit", "verify snapshot regeneration is reproducible"]
  | .residual_risks = ["Dropping password_hash remains destructive without compatibility handling"]
  | .final_verdict = "DO_NOT_COMMIT"
  ' "$final_template_json" >"$reducer_final_json"

jq -e '
  .coverage_validation == "passed"
  and .coverage_gaps == []
  and (.finding_merge.blockers | length) == 1
  and (.dependency_checks | index("schema drop requires compatibility rollout review") != null)
  and .final_verdict == "DO_NOT_COMMIT"
' "$reducer_final_json" >/dev/null \
  || fail 'reducer finalization template did not materialize into the expected blocking verdict'

printf 'full review workflow tests passed\n'
