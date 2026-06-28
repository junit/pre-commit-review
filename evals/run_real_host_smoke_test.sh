#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/evals/run_real_host_smoke.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'run real host smoke test failed: %s\n' "$*" >&2
  exit 1
}

[ -f "$wrapper" ] || fail 'missing evals/run_real_host_smoke.sh'

cat >"$tmp_dir/mock-pipeline" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
all_args="$*"
report_json=''
print_report_json='false'
while [ "$#" -gt 0 ]; do
  case "$1" in
    --report-json)
      shift
      report_json="$1"
      ;;
    --print-report-json)
      print_report_json='true'
      ;;
  esac
  shift
done
printf '%s\n' "$all_args" >"${REAL_HOST_SMOKE_LOG_FILE:?}"
cat >"$report_json" <<'JSON'
{"schema_version":"host-readiness-report/v1","host":"claude","overall_status":"passed"}
JSON
if [ "$print_report_json" = 'true' ]; then
  cat "$report_json"
fi
printf 'pipeline ok\n'
EOF
chmod +x "$tmp_dir/mock-pipeline"

artifact_dir="$tmp_dir/artifacts"
REAL_HOST_SMOKE_LOG_FILE="$tmp_dir/claude.log" \
  bash "$wrapper" \
    --host claude \
    --pipeline "$tmp_dir/mock-pipeline" \
    --artifact-dir "$artifact_dir" \
    --claude-bin /tmp/mock-claude \
    --print-report-json \
    --case mixed-staged-unstaged >"$tmp_dir/claude.out"

grep -Fq '=== Real Host Smoke: claude ===' "$tmp_dir/claude.out" \
  || fail 'claude banner changed'
grep -Fq '"schema_version":"host-readiness-report/v1"' "$tmp_dir/claude.out" \
  || fail 'wrapper must print pipeline json when requested'
grep -Fq 'pipeline ok' "$tmp_dir/claude.out" \
  || fail 'wrapper must surface pipeline output'
grep -Fq -- '--host claude --report-json' "$tmp_dir/claude.log" \
  || fail 'wrapper must pass host and report-json to pipeline'
grep -Fq -- '--claude-bin /tmp/mock-claude' "$tmp_dir/claude.log" \
  || fail 'wrapper must pass claude binary override to pipeline'
grep -Fq -- '--case mixed-staged-unstaged' "$tmp_dir/claude.log" \
  || fail 'wrapper must preserve passthrough arguments'
[ -f "$artifact_dir/claude-host-readiness-report.json" ] \
  || fail 'wrapper must materialize the claude report in the artifact dir'

REAL_HOST_SMOKE_LOG_FILE="$tmp_dir/codex.log" \
  bash "$wrapper" \
    --host codex \
    --pipeline "$tmp_dir/mock-pipeline" \
    --artifact-dir "$artifact_dir" \
    --codex-bin /tmp/mock-codex >"$tmp_dir/codex.out"

grep -Fq -- '--host codex --report-json' "$tmp_dir/codex.log" \
  || fail 'wrapper must pass codex host and report-json to pipeline'
grep -Fq -- '--codex-bin /tmp/mock-codex' "$tmp_dir/codex.log" \
  || fail 'wrapper must pass codex binary override to pipeline'
[ -f "$artifact_dir/codex-host-readiness-report.json" ] \
  || fail 'wrapper must materialize the codex report in the artifact dir'

if bash "$wrapper" --pipeline "$tmp_dir/mock-pipeline" >"$tmp_dir/missing-host.out" 2>"$tmp_dir/missing-host.err"; then
  fail 'wrapper must reject missing --host'
fi
grep -Fq 'run real host smoke failed: --host is required' "$tmp_dir/missing-host.err" \
  || fail 'missing --host error message changed'

if bash "$wrapper" --host invalid --pipeline "$tmp_dir/mock-pipeline" >"$tmp_dir/invalid-host.out" 2>"$tmp_dir/invalid-host.err"; then
  fail 'wrapper must reject invalid host values'
fi
grep -Fq 'run real host smoke failed: unsupported host: invalid' "$tmp_dir/invalid-host.err" \
  || fail 'invalid host error message changed'

printf 'run real host smoke tests passed\n'
