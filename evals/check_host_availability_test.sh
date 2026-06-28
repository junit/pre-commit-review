#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
gate="$repo_root/evals/check_host_availability.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'host availability gate test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

assert_taxonomy_failure() {
  local err_file="$1"
  local expected_type="$2"

  grep -Fq "host eval failure [$expected_type]:" "$err_file" \
    || fail "missing taxonomy token $expected_type"
}

cat >"$tmp_dir/mock-claude-ok" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'ok from claude\n'
EOF
chmod +x "$tmp_dir/mock-claude-ok"

cat >"$tmp_dir/mock-claude-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 17
EOF
chmod +x "$tmp_dir/mock-claude-fail"

cat >"$tmp_dir/mock-claude-empty" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$tmp_dir/mock-claude-empty"

cat >"$tmp_dir/mock-codex-ok" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'ok from codex\n'
EOF
chmod +x "$tmp_dir/mock-codex-ok"

cat >"$tmp_dir/mock-codex-fail" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 21
EOF
chmod +x "$tmp_dir/mock-codex-fail"

cat >"$tmp_dir/mock-codex-empty" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
chmod +x "$tmp_dir/mock-codex-empty"

claude_success_report="$tmp_dir/claude-success-report.json"
bash "$gate" \
  --host claude \
  --claude-bin "$tmp_dir/mock-claude-ok" \
  --report-json "$claude_success_report" >"$tmp_dir/claude-ok.out"
grep -Fq 'host availability ok [claude]: binary and handshake succeeded' "$tmp_dir/claude-ok.out" \
  || fail 'claude success output changed'
require_json_field "$claude_success_report" '.schema_version == "host-stage-report/v1"' 'claude success schema changed'
require_json_field "$claude_success_report" '.stage == "availability"' 'claude success stage changed'
require_json_field "$claude_success_report" '.host == "claude"' 'claude success host changed'
require_json_field "$claude_success_report" '.status == "passed"' 'claude success status changed'
require_json_field "$claude_success_report" '.failure_taxonomy == null' 'claude success failure_taxonomy must be null'

if bash "$gate" --host claude --claude-bin "$tmp_dir/does-not-exist" >"$tmp_dir/claude-missing.out" 2>"$tmp_dir/claude-missing.err"; then
  fail 'claude missing-binary case must fail'
fi
assert_taxonomy_failure "$tmp_dir/claude-missing.err" 'missing-binary'

if bash "$gate" --host codex --codex-bin "$tmp_dir/does-not-exist" >"$tmp_dir/codex-missing.out" 2>"$tmp_dir/codex-missing.err"; then
  fail 'codex missing-binary case must fail'
fi
assert_taxonomy_failure "$tmp_dir/codex-missing.err" 'missing-binary'

if bash "$gate" --host claude --claude-bin "$tmp_dir/mock-claude-fail" >"$tmp_dir/claude-fail.out" 2>"$tmp_dir/claude-fail.err"; then
  fail 'claude handshake non-zero case must fail'
fi
assert_taxonomy_failure "$tmp_dir/claude-fail.err" 'runner-exit-nonzero'

if bash "$gate" --host codex --codex-bin "$tmp_dir/mock-codex-fail" >"$tmp_dir/codex-fail.out" 2>"$tmp_dir/codex-fail.err"; then
  fail 'codex handshake non-zero case must fail'
fi
assert_taxonomy_failure "$tmp_dir/codex-fail.err" 'runner-exit-nonzero'

if bash "$gate" --host claude --claude-bin "$tmp_dir/mock-claude-empty" >"$tmp_dir/claude-empty.out" 2>"$tmp_dir/claude-empty.err"; then
  fail 'claude empty handshake case must fail'
fi
assert_taxonomy_failure "$tmp_dir/claude-empty.err" 'protocol-mismatch'

if bash "$gate" --host codex --codex-bin "$tmp_dir/mock-codex-empty" >"$tmp_dir/codex-empty.out" 2>"$tmp_dir/codex-empty.err"; then
  fail 'codex empty handshake case must fail'
fi
assert_taxonomy_failure "$tmp_dir/codex-empty.err" 'protocol-mismatch'

codex_success_report="$tmp_dir/codex-success-report.json"
bash "$gate" \
  --host codex \
  --codex-bin "$tmp_dir/mock-codex-ok" \
  --report-json "$codex_success_report" >"$tmp_dir/codex-ok.out"
grep -Fq 'host availability ok [codex]: binary and handshake succeeded' "$tmp_dir/codex-ok.out" \
  || fail 'codex success output changed'
require_json_field "$codex_success_report" '.schema_version == "host-stage-report/v1"' 'codex success schema changed'
require_json_field "$codex_success_report" '.stage == "availability"' 'codex success stage changed'
require_json_field "$codex_success_report" '.host == "codex"' 'codex success host changed'
require_json_field "$codex_success_report" '.status == "passed"' 'codex success status changed'
require_json_field "$codex_success_report" '.failure_taxonomy == null' 'codex success failure_taxonomy must be null'

claude_failure_report="$tmp_dir/claude-failure-report.json"
if bash "$gate" \
  --host claude \
  --claude-bin "$tmp_dir/mock-claude-fail" \
  --report-json "$claude_failure_report" >"$tmp_dir/claude-report-fail.out" 2>"$tmp_dir/claude-report-fail.err"; then
  fail 'claude report-json failure case must fail'
fi
require_json_field "$claude_failure_report" '.schema_version == "host-stage-report/v1"' 'claude failure schema changed'
require_json_field "$claude_failure_report" '.stage == "availability"' 'claude failure stage changed'
require_json_field "$claude_failure_report" '.host == "claude"' 'claude failure host changed'
require_json_field "$claude_failure_report" '.status == "failed"' 'claude failure status changed'
require_json_field "$claude_failure_report" '.failure_taxonomy == "runner-exit-nonzero"' 'claude failure taxonomy changed'

printf 'host availability gate tests passed\n'
