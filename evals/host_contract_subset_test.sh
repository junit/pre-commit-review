#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
subset="$repo_root/evals/host_contract_subset.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'host contract subset test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

success_report="$tmp_dir/subset-success-report.json"
bash "$subset" --report-json "$success_report" >"$tmp_dir/success.out"
grep -Fq 'host contract subset passed' "$tmp_dir/success.out" \
  || fail 'success output changed'
require_json_field "$success_report" '.schema_version == "host-stage-report/v1"' 'subset success schema changed'
require_json_field "$success_report" '.stage == "contract_subset"' 'subset success stage changed'
require_json_field "$success_report" '.host == null' 'subset success host must be null'
require_json_field "$success_report" '.status == "passed"' 'subset success status changed'
require_json_field "$success_report" '.failure_taxonomy == null' 'subset success failure_taxonomy must be null'

copy_root="$tmp_dir/repo-copy"
mkdir -p "$copy_root"
cp -R "$repo_root/evals" "$copy_root/evals"
rm -f "$copy_root/evals/run_host_readiness_pipeline.sh"
missing_report="$tmp_dir/missing-host-file-report.json"
if bash "$copy_root/evals/host_contract_subset.sh" --report-json "$missing_report" >"$tmp_dir/missing-host-file.out" 2>"$tmp_dir/missing-host-file.err"; then
  fail 'subset must fail when a required host file is missing'
fi
grep -Fq 'host contract subset failed: missing evals/run_host_readiness_pipeline.sh' "$tmp_dir/missing-host-file.err" \
  || fail 'missing host file failure output changed'
require_json_field "$missing_report" '.schema_version == "host-stage-report/v1"' 'subset failure schema changed'
require_json_field "$missing_report" '.stage == "contract_subset"' 'subset failure stage changed'
require_json_field "$missing_report" '.host == null' 'subset failure host must be null'
require_json_field "$missing_report" '.status == "failed"' 'subset failure status changed'
require_json_field "$missing_report" '.failure_taxonomy == "missing-file"' 'subset failure taxonomy changed'

printf 'host contract subset tests passed\n'
