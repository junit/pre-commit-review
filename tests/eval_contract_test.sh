#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
trigger_eval_file="$repo_root/tests/trigger-eval.json"
output_eval_file="$repo_root/tests/output-eval.json"

fail() {
  printf 'eval contract test failed: %s\n' "$*" >&2
  exit 1
}

[ -f "$trigger_eval_file" ] || fail 'missing tests/trigger-eval.json'
[ -f "$output_eval_file" ] || fail 'missing tests/output-eval.json'

python3 -m json.tool "$trigger_eval_file" >/dev/null \
  || fail 'trigger-eval.json must be valid JSON'
python3 -m json.tool "$output_eval_file" >/dev/null \
  || fail 'output-eval.json must be valid JSON'

python3 - "$trigger_eval_file" "$output_eval_file" <<'PY'
import json
import sys

trigger_path, output_path = sys.argv[1:3]

with open(trigger_path, encoding="utf-8") as f:
    trigger = json.load(f)
with open(output_path, encoding="utf-8") as f:
    output = json.load(f)


def fail(message):
    print(f"eval contract test failed: {message}", file=sys.stderr)
    sys.exit(1)


trigger_cases = trigger.get("cases")
if not isinstance(trigger_cases, list) or len(trigger_cases) < 8:
    fail("trigger-eval.json must contain at least 8 cases")

positive = [case for case in trigger_cases if case.get("expected_trigger") is True]
negative = [case for case in trigger_cases if case.get("expected_trigger") is False]
if len(positive) < 4 or len(negative) < 4:
    fail("trigger-eval.json must contain at least 4 positive and 4 negative cases")

for case in trigger_cases:
    for field in ("id", "prompt", "expected_trigger", "rationale"):
        if field not in case:
            fail(f"trigger case missing {field}: {case}")

if not any("提交" in case.get("prompt", "") for case in positive):
    fail("trigger positives must include a Chinese commit-readiness prompt")
if not any("staged" in case.get("prompt", "").lower() for case in positive):
    fail("trigger positives must include a staged-changes prompt")
if not any("debug" in case.get("prompt", "").lower() for case in negative):
    fail("trigger negatives must include a debugging prompt")
if not any("function" in case.get("prompt", "").lower() for case in negative):
    fail("trigger negatives must include a single-function review prompt")

output_cases = output.get("cases")
if not isinstance(output_cases, list) or len(output_cases) < 8:
    fail("output-eval.json must contain at least 8 cases")

required_scenarios = {
    "tiny-docs",
    "mixed-staged-unstaged",
    "hardcoded-secret",
    "breaking-api",
    "large-generated",
    "no-git-repo",
    "chinese-request",
    "pasted-diff",
}
seen = {case.get("scenario") for case in output_cases}
missing = sorted(required_scenarios - seen)
if missing:
    fail(f"output-eval.json missing scenarios: {', '.join(missing)}")

for case in output_cases:
    for field in ("id", "scenario", "prompt", "locale", "expected"):
        if field not in case:
            fail(f"output case missing {field}: {case}")
    expected = case["expected"]
    if "verdict" not in expected or "must_include" not in expected:
        fail(f"output case expected block is incomplete: {case['id']}")

print("eval contract tests passed")
PY
