#!/usr/bin/env bash
set -euo pipefail

current_path=''
baseline_path=''
report_json_path=''

usage() {
  cat <<'EOF'
Usage: analyze_host_readiness_diff.sh --current <path> [--baseline <path>] --report-json <path>

Analyze cross-host readiness output and optionally compare it to a baseline.

Options:
  --current <path>       Required path to the current cross-host JSON report
  --baseline <path>      Optional path to a baseline cross-host JSON report
  --report-json <path>   Required output path for the analysis JSON
  -h, --help             Show this help
EOF
}

fail() {
  printf 'analyze host readiness diff failed: %s\n' "$*" >&2
  exit 1
}

require_file() {
  local file_path="$1"
  local flag_name="$2"

  [ -n "$file_path" ] || fail "$flag_name is required"
  [ -f "$file_path" ] || fail "$flag_name file not found: $file_path"
}

require_report_schema() {
  local file_path="$1"
  local expected_schema="$2"
  local label="$3"

  jq -e --arg expected_schema "$expected_schema" '.schema_version == $expected_schema' "$file_path" >/dev/null \
    || fail "unsupported $label schema"
}

started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
started_epoch_ms="$(date -u +"%s000")"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --current)
      shift
      [ "$#" -gt 0 ] || fail '--current requires a value'
      current_path="$1"
      ;;
    --baseline)
      shift
      [ "$#" -gt 0 ] || fail '--baseline requires a value'
      baseline_path="$1"
      ;;
    --report-json)
      shift
      [ "$#" -gt 0 ] || fail '--report-json requires a value'
      report_json_path="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
  shift
done

require_file "$current_path" '--current'
[ -n "$report_json_path" ] || fail '--report-json is required'
require_report_schema "$current_path" 'cross-host-readiness-report/v1' 'current report'

if [ -n "$baseline_path" ]; then
  require_file "$baseline_path" '--baseline'
  require_report_schema "$baseline_path" 'cross-host-readiness-report/v1' 'baseline report'
fi

cross_host_diff_json="$(jq -cn --slurpfile current_doc "$current_path" '
  def effective_status_from_entry($entry):
    if $entry == null then "missing"
    elif ($entry.execution_status // "") == "skipped" then "skipped"
    elif (($entry.report // null) | type) == "object" and (($entry.report.overall_status // null) != null) then $entry.report.overall_status
    else ($entry.execution_status // "unknown")
    end;
  def host_status_pairs($doc):
    reduce (($doc.hosts // {}) | to_entries | sort_by(.key)[]) as $entry
      ({};
        .[$entry.key] = effective_status_from_entry($entry.value)
      );
  def failed_stage_diff($doc):
    reduce (($doc.hosts // {}) | to_entries | sort_by(.key)[]) as $entry
      ({};
        if (($entry.value.report.failed_stage // null) != null) then
          .[$entry.key] = $entry.value.report.failed_stage
        else
          .
        end
      );
  def failure_taxonomy_diff($doc):
    reduce (($doc.hosts // {}) | to_entries | sort_by(.key)[]) as $entry
      ({};
        if (($entry.value.report.failure_taxonomy // null) != null) then
          .[$entry.key] = $entry.value.report.failure_taxonomy
        else
          .
        end
      );
  def format_hosts($hosts):
    if ($hosts | length) == 1 then $hosts[0] else ($hosts | join(", ")) end;
  ($current_doc[0]) as $current
  | (host_status_pairs($current)) as $pairs
  | ($pairs | to_entries) as $entries
  | ($entries | map(.value) | unique | length <= 1) as $same_overall_status
  | ($entries
      | group_by(.value)
      | map({
          status: .[0].value,
          hosts: (map(.key) | sort)
        })
      | sort_by([
          if .status == "failed" then 0
          elif .status == "skipped" then 1
          elif .status == "passed" then 2
          elif .status == "completed" then 3
          else 4
          end,
          .status
        ])) as $groups
  | {
      same_overall_status: $same_overall_status,
      host_status_pairs: $pairs,
      failed_stage_diff: failed_stage_diff($current),
      failure_taxonomy_diff: failure_taxonomy_diff($current),
      summary:
        if ($entries | length) == 0 then
          "no hosts analyzed"
        elif $same_overall_status and (($groups[0].hosts | length) > 1) then
          "all hosts \($groups[0].status)"
        elif $same_overall_status then
          "\($groups[0].hosts[0]) \($groups[0].status)"
        else
          ($groups
            | map("\(format_hosts(.hosts)) \(.status)")
            | join(" while "))
        end
    }')"

baseline_compare_json='null'
analysis_scope='cross-host-only'

if [ -n "$baseline_path" ]; then
  analysis_scope='cross-host-and-baseline'
  baseline_compare_json="$(jq -cn --slurpfile current_doc "$current_path" --slurpfile baseline_doc "$baseline_path" '
    def effective_status_from_entry($entry):
      if $entry == null then "missing"
      elif ($entry.execution_status // "") == "skipped" then "skipped"
      elif (($entry.report // null) | type) == "object" and (($entry.report.overall_status // null) != null) then $entry.report.overall_status
      else ($entry.execution_status // "unknown")
      end;
    def failed_stage_from_entry($entry):
      if (($entry.report // null) | type) == "object" then ($entry.report.failed_stage // null) else null end;
    def failure_taxonomy_from_entry($entry):
      if (($entry.report // null) | type) == "object" then ($entry.report.failure_taxonomy // null) else null end;
    def status_rank($status):
      if $status == "passed" then 4
      elif $status == "completed" then 3
      elif $status == "skipped" then 2
      elif $status == "failed" then 1
      elif $status == "missing" then 0
      else -1
      end;
    def change_type($current_status; $baseline_status; $current_failed_stage; $baseline_failed_stage; $current_failure_taxonomy; $baseline_failure_taxonomy):
      if $current_status == $baseline_status
        and $current_failed_stage == $baseline_failed_stage
        and $current_failure_taxonomy == $baseline_failure_taxonomy then
        "unchanged"
      elif status_rank($current_status) > status_rank($baseline_status) then
        "improved"
      else
        "regressed"
      end;
    def format_hosts($hosts):
      if ($hosts | length) == 1 then $hosts[0] else ($hosts | join(", ")) end;
    ($current_doc[0]) as $current
    | ($baseline_doc[0]) as $baseline
    | ((($current.hosts // {}) | keys) + (($baseline.hosts // {}) | keys) | unique | sort) as $host_names
    | (reduce $host_names[] as $host
        ({};
          ($current.hosts[$host] // null) as $current_entry
          | ($baseline.hosts[$host] // null) as $baseline_entry
          | (effective_status_from_entry($current_entry)) as $current_status
          | (effective_status_from_entry($baseline_entry)) as $baseline_status
          | (failed_stage_from_entry($current_entry)) as $current_failed_stage
          | (failed_stage_from_entry($baseline_entry)) as $baseline_failed_stage
          | (failure_taxonomy_from_entry($current_entry)) as $current_failure_taxonomy
          | (failure_taxonomy_from_entry($baseline_entry)) as $baseline_failure_taxonomy
          | .[$host] = {
              change_type: change_type(
                $current_status;
                $baseline_status;
                $current_failed_stage;
                $baseline_failed_stage;
                $current_failure_taxonomy;
                $baseline_failure_taxonomy
              ),
              current_status: $current_status,
              baseline_status: $baseline_status,
              current_failed_stage: $current_failed_stage,
              baseline_failed_stage: $baseline_failed_stage,
              current_failure_taxonomy: $current_failure_taxonomy,
              baseline_failure_taxonomy: $baseline_failure_taxonomy
            }
        )) as $host_deltas
    | ($host_deltas | to_entries | map(select(.value.change_type == "regressed") | .key)) as $regressed_hosts
    | ($host_deltas | to_entries | map(select(.value.change_type == "improved") | .key)) as $improved_hosts
    | {
        overall_status_changed: (($current.overall_status // "unknown") != ($baseline.overall_status // "unknown")),
        failed_hosts_added: ((($current.failed_hosts // []) - ($baseline.failed_hosts // [])) | sort),
        failed_hosts_removed: ((($baseline.failed_hosts // []) - ($current.failed_hosts // [])) | sort),
        host_deltas: $host_deltas,
        summary:
          if ($regressed_hosts | length) > 0 then
            "\(format_hosts($regressed_hosts)) regressed relative to baseline"
          elif ($improved_hosts | length) > 0 then
            "\(format_hosts($improved_hosts)) improved relative to baseline"
          else
            "no host-level changes relative to baseline"
          end
      }')"
fi

finished_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
finished_epoch_ms="$(date -u +"%s000")"
duration_ms=$((finished_epoch_ms - started_epoch_ms))
current_overall_status="$(jq -r '.overall_status // "unknown"' "$current_path")"

report_json="$(jq -cn \
  --arg schema_version "host-readiness-diff-report/v1" \
  --arg analysis_scope "$analysis_scope" \
  --arg current_overall_status "$current_overall_status" \
  --arg started_at "$started_at" \
  --arg finished_at "$finished_at" \
  --argjson duration_ms "$duration_ms" \
  --argjson cross_host_diff "$cross_host_diff_json" \
  --argjson baseline_compare "$baseline_compare_json" \
  '{
    schema_version: $schema_version,
    analysis_scope: $analysis_scope,
    current_overall_status: $current_overall_status,
    cross_host_diff: $cross_host_diff,
    baseline_compare: $baseline_compare,
    started_at: $started_at,
    finished_at: $finished_at,
    duration_ms: $duration_ms
  }')"

printf '%s\n' "$report_json" >"$report_json_path"
