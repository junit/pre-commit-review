#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
checker="$repo_root/evals/check_persisted_output_contract.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'persisted output contract test failed: %s\n' "$*" >&2
  exit 1
}

bad_transcript="$tmp_dir/bad.jsonl"
cat >"$bad_transcript" <<'JSONL'
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"<persisted-output>\nOutput too large (332.7KB). Full output saved to: /tmp/helper-output.txt\n\nPreview (first 2KB):\n# Pre-Commit Review Diff Context\n"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git status --short && git diff --cached --name-only"}}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"审查范围：完整审查全部 49 个文件的 staged diff。"}]}}
JSONL

if "$checker" "$bad_transcript" >/dev/null 2>&1; then
  fail 'checker should reject persisted helper output when the saved plan was never read'
fi

good_read_transcript="$tmp_dir/good-read.jsonl"
cat >"$good_read_transcript" <<'JSONL'
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"<persisted-output>\nOutput too large (332.7KB). Full output saved to: /tmp/helper-output.txt\n\nPreview (first 2KB):\n# Pre-Commit Review Diff Context\n"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/helper-output.txt"}}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"coverage validation passed; 审查范围：完整审查全部 manifest units。"}]}}
JSONL

"$checker" "$good_read_transcript" >/dev/null || fail 'checker should accept reading the persisted helper output'

good_plan_transcript="$tmp_dir/good-plan.jsonl"
cat >"$good_plan_transcript" <<'JSONL'
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"<persisted-output>\nOutput too large (332.7KB). Full output saved to: /tmp/helper-output.txt\n\nPreview (first 2KB):\n# Pre-Commit Review Diff Context\n"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh --plan-only"}}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"coverage validation passed; full review over all manifest units."}]}}
JSONL

"$checker" "$good_plan_transcript" >/dev/null || fail 'checker should accept explicit plan-only recovery after persisted output'

good_control_transcript="$tmp_dir/good-control.jsonl"
cat >"$good_control_transcript" <<'JSONL'
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"<persisted-output>\nOutput too large. Full output saved to: /tmp/helper-output.txt"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"bash /skill/scripts/collect_diff_context.sh --control-plane"}}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"coverage validation passed; full review over the authoritative scope fingerprint."}]}}
JSONL

"$checker" "$good_control_transcript" >/dev/null || fail 'checker should accept compact control-plane recovery after persisted output'

printf 'persisted output contract tests passed\n'
