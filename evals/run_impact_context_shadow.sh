#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
legacy_helper="$repo_root/scripts/collect_diff_context.sh"
context_helper="$repo_root/scripts/collect_impact_context.sh"

source_name=''
output_path=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --source)
      [ "$#" -ge 2 ] || { printf '%s\n' 'run_impact_context_shadow: --source requires a value' >&2; exit 2; }
      source_name="$2"
      shift 2
      ;;
    --source=*)
      source_name="${1#*=}"
      shift
      ;;
    --output)
      [ "$#" -ge 2 ] || { printf '%s\n' 'run_impact_context_shadow: --output requires a value' >&2; exit 2; }
      output_path="$2"
      shift 2
      ;;
    --output=*)
      output_path="${1#*=}"
      shift
      ;;
    -h|--help)
      printf '%s\n' 'Usage: run_impact_context_shadow.sh --source <staged|unstaged|branch> --output <absolute-path>'
      exit 0
      ;;
    *)
      printf 'run_impact_context_shadow: unsupported argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

case "$source_name" in
  staged|unstaged|branch) ;;
  *)
    printf '%s\n' 'run_impact_context_shadow: --source is required and must be staged, unstaged, or branch' >&2
    exit 2
    ;;
esac
case "$output_path" in
  /*) ;;
  *)
    printf '%s\n' 'run_impact_context_shadow: --output must be an absolute path' >&2
    exit 2
    ;;
esac
[ -x "$legacy_helper" ] || { printf '%s\n' 'run_impact_context_shadow: Rust report helper is unavailable' >&2; exit 2; }
[ -x "$context_helper" ] || { printf '%s\n' 'run_impact_context_shadow: impact context helper is unavailable' >&2; exit 2; }
command -v python3 >/dev/null 2>&1 \
  || { printf '%s\n' 'run_impact_context_shadow: python3 is required' >&2; exit 2; }

output_dir="$(dirname -- "$output_path")"
[ -d "$output_dir" ] || { printf '%s\n' 'run_impact_context_shadow: output directory does not exist' >&2; exit 2; }

control_output="$(mktemp)"
control_error="$(mktemp)"
legacy_output="$(mktemp)"
legacy_error="$(mktemp)"
context_output="$(mktemp)"
context_error="$(mktemp)"
metrics_tmp="$(mktemp "$output_dir/.impact-context-shadow.XXXXXX")"
trap 'rm -f "$control_output" "$control_error" "$legacy_output" "$legacy_error" "$context_output" "$context_error" "$metrics_tmp"' EXIT

started_ns="$(python3 -c 'import time; print(time.monotonic_ns())')"
PRE_COMMIT_REVIEW_HELPER_IMPL=rust PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
  "$legacy_helper" --source "$source_name" --control-plane \
  >"$control_output" 2>"$control_error"
scope_fingerprint="$(python3 - "$control_output" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
marker = lines.index('## Review Control Plane JSON')
payload = json.loads(lines[marker + 1])
if not payload.get('authoritative'):
    raise SystemExit('control plane is not authoritative')
print(payload['scope_fingerprint'])
PY
)"

PRE_COMMIT_REVIEW_HELPER_IMPL=rust PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
  "$legacy_helper" --source "$source_name" --expect-scope "$scope_fingerprint" \
  >"$legacy_output" 2>"$legacy_error"

context_exit=0
"$context_helper" --source "$source_name" --expect-scope "$scope_fingerprint" --mode fast \
  >"$context_output" 2>"$context_error" || context_exit=$?
if [ "$context_exit" -ne 0 ] && [ "$context_exit" -ne 3 ]; then
  cat "$context_error" >&2
  exit "$context_exit"
fi

python3 - "$legacy_output" "$context_output" "$scope_fingerprint" "$started_ns" "$metrics_tmp" <<'PY'
import json
import pathlib
import sys
import time

legacy_path, context_path, fingerprint, started_ns, output_path = sys.argv[1:]
legacy_lines = pathlib.Path(legacy_path).read_text(encoding='utf-8').splitlines()
context_lines = pathlib.Path(context_path).read_text(encoding='utf-8').splitlines()

def section_json(title):
    marker = legacy_lines.index(title)
    lines = []
    for line in legacy_lines[marker + 1:]:
        if line.startswith('## '):
            break
        if line:
            lines.append(line)
    return json.loads('\n'.join(lines))

review_plan = section_json('## Review Plan JSON')
impact_reference = review_plan['impact_context']

marker = context_lines.index('## Impact Context JSON')
context = json.loads(context_lines[marker + 1])
if context['scope']['fingerprint'] != fingerprint:
    raise SystemExit('impact context fingerprint mismatch')

metrics = {
    'schema_version': 1,
    'kind': 'impact_context_shadow_metrics',
    'scope_fingerprint': fingerprint,
    'report_review_plan_schema_version': review_plan['schema_version'],
    'report_impact_context_contract': impact_reference['contract'],
    'report_impact_context_retrieval': impact_reference['retrieval'],
    'report_impact_context_coverage_credit': impact_reference['coverage_credit'],
    'new_changed_symbols': len(context['changed_symbols']),
    'new_impact_edges': len(context['impact_edges']),
    'new_domain_summaries': len(context['domain_summaries']),
    'new_status': context['status'],
    'new_limitation_codes': sorted({item['code'] for item in context['limitations']}),
    'elapsed_ms': max(0, (time.monotonic_ns() - int(started_ns)) // 1_000_000),
}
pathlib.Path(output_path).write_text(
    json.dumps(metrics, separators=(',', ':')) + '\n',
    encoding='utf-8',
)
PY

mv "$metrics_tmp" "$output_path"
cat "$legacy_output"
[ -s "$legacy_error" ] && cat "$legacy_error" >&2
[ -s "$context_error" ] && cat "$context_error" >&2
exit 0
