#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
probe="$repo_root/evals/run_helper_gateway_probe.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'helper gateway probe test failed: %s\n' "$*" >&2
  exit 1
}

require_json_field() {
  local file="$1"
  local filter="$2"
  local message="$3"

  jq -e "$filter" "$file" >/dev/null || fail "$message"
}

[ -f "$probe" ] || fail 'missing evals/run_helper_gateway_probe.sh'

cat >"$tmp_dir/mock-codex-good" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
response_file=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      response_file="$1"
      ;;
  esac
  shift
done
cat >/dev/null
./.agents/skills/pre-commit-review/scripts/collect_diff_context.sh >/dev/null
printf '**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES\n' >"$response_file"
EOF
chmod +x "$tmp_dir/mock-codex-good"

codex_success_report="$tmp_dir/codex-success.json"
bash "$probe" \
  --host codex \
  --codex-bin "$tmp_dir/mock-codex-good" \
  --report-json "$codex_success_report" >"$tmp_dir/codex-success.out"

grep -Fq 'helper gateway probe passed [codex]' "$tmp_dir/codex-success.out" \
  || fail 'codex success output changed'
require_json_field "$codex_success_report" '.schema_version == "host-stage-report/v1"' 'codex success schema changed'
require_json_field "$codex_success_report" '.stage == "helper_gateway_probe"' 'codex success stage changed'
require_json_field "$codex_success_report" '.host == "codex"' 'codex success host changed'
require_json_field "$codex_success_report" '.status == "passed"' 'codex success status changed'
require_json_field "$codex_success_report" '.failure_taxonomy == null' 'codex success failure_taxonomy must be null'

cat >"$tmp_dir/mock-claude-good" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
./.claude/skills/pre-commit-review/scripts/collect_diff_context.sh >/dev/null
printf '**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES\n'
EOF
chmod +x "$tmp_dir/mock-claude-good"

claude_success_report="$tmp_dir/claude-success.json"
bash "$probe" \
  --host claude \
  --claude-bin "$tmp_dir/mock-claude-good" \
  --report-json "$claude_success_report" >"$tmp_dir/claude-success.out"

grep -Fq 'helper gateway probe passed [claude]' "$tmp_dir/claude-success.out" \
  || fail 'claude success output changed'
require_json_field "$claude_success_report" '.host == "claude"' 'claude success host changed'
require_json_field "$claude_success_report" '.status == "passed"' 'claude success status changed'

cat >"$tmp_dir/mock-codex-direct-first" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
response_file=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      response_file="$1"
      ;;
  esac
  shift
done
cat >/dev/null
git status --short >/dev/null
./.agents/skills/pre-commit-review/scripts/collect_diff_context.sh >/dev/null
printf '**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES\n' >"$response_file"
EOF
chmod +x "$tmp_dir/mock-codex-direct-first"

direct_first_report="$tmp_dir/direct-first.json"
if bash "$probe" \
  --host codex \
  --codex-bin "$tmp_dir/mock-codex-direct-first" \
  --report-json "$direct_first_report" >"$tmp_dir/direct-first.out" 2>"$tmp_dir/direct-first.err"; then
  fail 'probe must fail when direct Git runs before helper'
fi

grep -Fq 'direct Git source-selection command ran before helper' "$tmp_dir/direct-first.err" \
  || fail 'direct-first failure message changed'
require_json_field "$direct_first_report" '.status == "failed"' 'direct-first status changed'
require_json_field "$direct_first_report" '.failure_taxonomy == "helper-gateway-violation"' 'direct-first taxonomy changed'

cat >"$tmp_dir/mock-codex-no-helper" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
response_file=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      response_file="$1"
      ;;
  esac
  shift
done
cat >/dev/null
printf '**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES\n' >"$response_file"
EOF
chmod +x "$tmp_dir/mock-codex-no-helper"

no_helper_report="$tmp_dir/no-helper.json"
if bash "$probe" \
  --host codex \
  --codex-bin "$tmp_dir/mock-codex-no-helper" \
  --report-json "$no_helper_report" >"$tmp_dir/no-helper.out" 2>"$tmp_dir/no-helper.err"; then
  fail 'probe must fail when helper is not called'
fi

grep -Fq 'collect_diff_context helper was not called' "$tmp_dir/no-helper.err" \
  || fail 'no-helper failure message changed'
require_json_field "$no_helper_report" '.failure_taxonomy == "helper-gateway-violation"' 'no-helper taxonomy changed'

if bash "$probe" --host invalid >"$tmp_dir/invalid.out" 2>"$tmp_dir/invalid.err"; then
  fail 'probe must reject invalid host'
fi
grep -Fq 'unsupported host: invalid' "$tmp_dir/invalid.err" \
  || fail 'invalid host error changed'

printf 'helper gateway probe tests passed\n'
