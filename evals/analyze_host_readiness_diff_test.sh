#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
analysis_script="$repo_root/evals/analyze_host_readiness_diff.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'host readiness diff analysis test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

cat >"$tmp_dir/current-cross-host.json" <<'JSON'
{
  "schema_version": "cross-host-readiness-report/v1",
  "mode": "complete-matrix",
  "overall_status": "failed",
  "failed_hosts": ["codex"],
  "skipped_hosts": [],
  "hosts": {
    "claude": {
      "execution_status": "completed",
      "report": {
        "schema_version": "host-readiness-report/v1",
        "host": "claude",
        "overall_status": "passed",
        "failed_stage": null,
        "failure_taxonomy": null
      }
    },
    "codex": {
      "execution_status": "completed",
      "report": {
        "schema_version": "host-readiness-report/v1",
        "host": "codex",
        "overall_status": "failed",
        "failed_stage": "layered_host_smoke",
        "failure_taxonomy": "runner-exit-nonzero"
      }
    }
  }
}
JSON

cat >"$tmp_dir/baseline-cross-host.json" <<'JSON'
{
  "schema_version": "cross-host-readiness-report/v1",
  "mode": "complete-matrix",
  "overall_status": "passed",
  "failed_hosts": [],
  "skipped_hosts": [],
  "hosts": {
    "claude": {
      "execution_status": "completed",
      "report": {
        "schema_version": "host-readiness-report/v1",
        "host": "claude",
        "overall_status": "passed",
        "failed_stage": null,
        "failure_taxonomy": null
      }
    },
    "codex": {
      "execution_status": "completed",
      "report": {
        "schema_version": "host-readiness-report/v1",
        "host": "codex",
        "overall_status": "passed",
        "failed_stage": null,
        "failure_taxonomy": null
      }
    }
  }
}
JSON

cross_host_only_report="$tmp_dir/cross-host-only-analysis.json"
bash "$analysis_script" \
  --current "$tmp_dir/current-cross-host.json" \
  --report-json "$cross_host_only_report"

require_json_field "$cross_host_only_report" '.schema_version == "host-readiness-diff-report/v1"' 'cross-host-only analysis schema changed'
require_json_field "$cross_host_only_report" '.analysis_scope == "cross-host-only"' 'cross-host-only analysis_scope changed'
require_json_field "$cross_host_only_report" '.current_overall_status == "failed"' 'cross-host-only current_overall_status changed'
require_json_field "$cross_host_only_report" '.baseline_compare == null' 'cross-host-only baseline_compare must be null'
require_json_field "$cross_host_only_report" '.cross_host_diff.same_overall_status == false' 'cross-host-only same_overall_status changed'
require_json_field "$cross_host_only_report" '.cross_host_diff.host_status_pairs.claude == "passed"' 'cross-host-only claude status changed'
require_json_field "$cross_host_only_report" '.cross_host_diff.host_status_pairs.codex == "failed"' 'cross-host-only codex status changed'
require_json_field "$cross_host_only_report" '.cross_host_diff.failed_stage_diff.codex == "layered_host_smoke"' 'cross-host-only failed_stage_diff changed'
require_json_field "$cross_host_only_report" '.cross_host_diff.failure_taxonomy_diff.codex == "runner-exit-nonzero"' 'cross-host-only taxonomy diff changed'
require_json_field "$cross_host_only_report" '.cross_host_diff.summary == "codex failed while claude passed"' 'cross-host-only summary changed'
require_json_field "$cross_host_only_report" 'has("started_at") and has("finished_at") and has("duration_ms")' 'cross-host-only timing fields missing'

baseline_report="$tmp_dir/baseline-analysis.json"
bash "$analysis_script" \
  --current "$tmp_dir/current-cross-host.json" \
  --baseline "$tmp_dir/baseline-cross-host.json" \
  --report-json "$baseline_report"

require_json_field "$baseline_report" '.schema_version == "host-readiness-diff-report/v1"' 'baseline analysis schema changed'
require_json_field "$baseline_report" '.analysis_scope == "cross-host-and-baseline"' 'baseline analysis_scope changed'
require_json_field "$baseline_report" '.baseline_compare.overall_status_changed == true' 'baseline overall_status_changed changed'
require_json_field "$baseline_report" '.baseline_compare.failed_hosts_added == ["codex"]' 'baseline failed_hosts_added changed'
require_json_field "$baseline_report" '.baseline_compare.failed_hosts_removed == []' 'baseline failed_hosts_removed changed'
require_json_field "$baseline_report" '.baseline_compare.host_deltas.claude.change_type == "unchanged"' 'baseline claude delta changed'
require_json_field "$baseline_report" '.baseline_compare.host_deltas.codex.change_type == "regressed"' 'baseline codex delta changed'
require_json_field "$baseline_report" '.baseline_compare.host_deltas.codex.current_failed_stage == "layered_host_smoke"' 'baseline codex current_failed_stage changed'
require_json_field "$baseline_report" '.baseline_compare.host_deltas.codex.baseline_failed_stage == null' 'baseline codex baseline_failed_stage changed'
require_json_field "$baseline_report" '.baseline_compare.host_deltas.codex.current_failure_taxonomy == "runner-exit-nonzero"' 'baseline codex current taxonomy changed'
require_json_field "$baseline_report" '.baseline_compare.summary == "codex regressed relative to baseline"' 'baseline summary changed'

printf 'host readiness diff analysis tests passed\n'
