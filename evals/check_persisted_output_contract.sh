#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: check_persisted_output_contract.sh TRANSCRIPT.jsonl

Fail when a host transcript shows collect_diff_context output was persisted
but the agent never recovered the saved plan/manifest before claiming a full
commit-readiness review.
EOF
}

if [ "$#" -ne 1 ] || [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  [ "$#" -eq 1 ] && exit 0
  exit 2
fi

python3 - "$1" <<'PY'
import json
import re
import sys
from pathlib import PurePosixPath

transcript = sys.argv[1]

persisted_paths = []
tool_texts = []
assistant_texts = []

def walk(value):
    if isinstance(value, dict):
        if value.get("type") == "tool_use":
            name = str(value.get("name", ""))
            tool_input = value.get("input") or {}
            if isinstance(tool_input, dict):
                command = tool_input.get("command") or tool_input.get("cmd") or ""
                file_path = tool_input.get("file_path") or tool_input.get("path") or ""
                tool_texts.append(f"{name} {command} {file_path}")
        if value.get("type") == "tool_result":
            content = str(value.get("content", ""))
            tool_texts.append(content)
            for match in re.finditer(r"Full output saved to:\s*([^\s\\n]+)", content):
                persisted_paths.append(match.group(1))
        if value.get("type") == "text":
            assistant_texts.append(str(value.get("text", "")))
        for child in value.values():
            walk(child)
    elif isinstance(value, list):
        for child in value:
            walk(child)

with open(transcript, encoding="utf-8") as handle:
    for line in handle:
        if not line.strip():
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        walk(payload)

if not persisted_paths:
    sys.exit(0)

all_tool_text = "\n".join(tool_texts)
all_assistant_text = "\n".join(assistant_texts)

def path_was_recovered(path):
    path_name = str(PurePosixPath(path))
    if re.search(rf"\bRead\b[^\n]*{re.escape(path_name)}", all_tool_text):
        return True
    if re.search(rf"{re.escape(path_name)}[^\n]*(Review Plan JSON|Review Manifest|Coverage Ledger)", all_tool_text):
        return True
    return False

read_persisted_output = any(path_was_recovered(path) for path in persisted_paths)
reran_plan = any(
    marker in all_tool_text
    for marker in (
        "collect_diff_context.sh --control-plane",
        "collect_diff_context.sh --plan-only",
        "collect_diff_context.sh --include-diff never",
        "collect_diff_context.sh --plan-json",
        "collect_diff_context.sh --manifest-jsonl",
    )
)

recovered_structured_context = read_persisted_output or reran_plan
full_claim = bool(
    re.search(r"(完整审查|完整覆盖|full review|full scope|complete review|reviewed all|all manifest units)", all_assistant_text, re.I)
)
coverage_validation = bool(
    re.search(r"(coverage validation|coverage_validation|覆盖校验|覆盖验证|manifest_units\s*-\s*reviewed_units)", all_assistant_text, re.I)
)

if not recovered_structured_context:
    print(
        "persisted helper output was not recovered: rerun collect_diff_context.sh --control-plane or read the saved legacy plan/manifest before reviewing",
        file=sys.stderr,
    )
    sys.exit(1)

if full_claim and not coverage_validation:
    print(
        "full-review claim lacks coverage validation evidence after persisted helper recovery",
        file=sys.stderr,
    )
    sys.exit(1)

sys.exit(0)
PY
