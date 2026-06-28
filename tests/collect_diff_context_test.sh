#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
helper="$repo_root/scripts/collect_diff_context.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'collect_diff_context test failed: %s\n' "$*" >&2
  exit 1
}

run_helper() {
  local workdir="$1"
  local output_file="$2"

  (
    cd "$workdir"
    "$helper"
  ) >"$output_file" 2>&1
}

assert_contains() {
  local file="$1"
  local expected="$2"

  grep -Fq -- "$expected" "$file" || {
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected output to contain: $expected"
  }
}

assert_not_contains() {
  local file="$1"
  local unexpected="$2"

  if grep -Fq -- "$unexpected" "$file"; then
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected output not to contain: $unexpected"
  fi
}

assert_work_packets_start_with_group_id() {
  local file="$1"

  awk '
    $0 == "## Group Review Work Packets" { in_section=1; next }
    in_section && /^## / { exit }
    in_section && $0 == "---" {
      if ((getline next_line) <= 0 || next_line !~ /^group_id: /) {
        exit 1
      }
      packet_count += 1
    }
    END {
      if (packet_count == 0) exit 1
    }
  ' "$file" || {
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail 'expected every group review work packet to start with group_id'
  }
}

assert_jsonl_section_valid() {
  local file="$1"
  local section="$2"

  python3 - "$file" "$section" <<'PY' || {
import json
import sys

path = sys.argv[1]
section = "## " + sys.argv[2]
in_section = False
count = 0

with open(path, encoding="utf-8") as handle:
    for raw_line in handle:
        line = raw_line.rstrip("\n")
        if line == section:
            in_section = True
            continue
        if in_section and line.startswith("## "):
            break
        if in_section and line.strip():
            json.loads(line)
            count += 1

if count == 0:
    raise SystemExit(f"missing or empty JSONL section: {section}")
PY
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected valid JSONL section: $section"
  }
}

assert_json_section_valid() {
  local file="$1"
  local section="$2"

  python3 - "$file" "$section" <<'PY' || {
import json
import sys

path = sys.argv[1]
section = "## " + sys.argv[2]
in_section = False
lines = []

with open(path, encoding="utf-8") as handle:
    for raw_line in handle:
        line = raw_line.rstrip("\n")
        if line == section:
            in_section = True
            continue
        if in_section and line.startswith("## "):
            break
        if in_section:
            lines.append(raw_line)

payload = "".join(lines).strip()
if not payload:
    raise SystemExit(f"missing or empty JSON section: {section}")
json.loads(payload)
PY
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected valid JSON section: $section"
  }
}

assert_review_plan_group_order() {
  local file="$1"
  shift

  python3 - "$file" "$@" <<'PY' || {
import json
import sys

path = sys.argv[1]
expected = sys.argv[2:]
section = "## Review Plan JSON"
in_section = False
lines = []

with open(path, encoding="utf-8") as handle:
    for raw_line in handle:
        line = raw_line.rstrip("\n")
        if line == section:
            in_section = True
            continue
        if in_section and line.startswith("## "):
            break
        if in_section:
            lines.append(raw_line)

payload = "".join(lines).strip()
plan = json.loads(payload)
actual = [group["group_id"] for group in plan["groups"][: len(expected)]]
if actual != expected:
    raise SystemExit(f"expected first groups {expected}, got {actual}")
PY
    printf '%s\n' '--- output ---' >&2
    cat "$file" >&2
    printf '%s\n' '--------------' >&2
    fail "expected Review Plan JSON group order: $*"
  }
}

init_repo() {
  local dir="$1"

  git -C "$dir" init -q
  git -C "$dir" config user.email a@example.com
  git -C "$dir" config user.name A
  printf 'one\n' >"$dir/f.txt"
  git -C "$dir" add f.txt
  git -C "$dir" commit -q -m init
}

non_repo="$tmp_dir/non-repo"
mkdir -p "$non_repo"
non_repo_output="$tmp_dir/non-repo.out"
run_helper "$non_repo" "$non_repo_output" || fail 'non-git repository output should not use a failing exit status'
assert_contains "$non_repo_output" 'repository: not a git repository'
assert_contains "$non_repo_output" 'diff_source: unavailable'

staged_repo="$tmp_dir/staged"
mkdir -p "$staged_repo"
init_repo "$staged_repo"
printf 'two\n' >>"$staged_repo/f.txt"
git -C "$staged_repo" add f.txt
staged_output="$tmp_dir/staged.out"
run_helper "$staged_repo" "$staged_output"
assert_contains "$staged_output" 'diff_source: staged changes via git diff --cached'
assert_contains "$staged_output" 'files_changed: 1 files, 1 insertions(+), 0 deletions(-)'

unstaged_repo="$tmp_dir/unstaged"
mkdir -p "$unstaged_repo"
init_repo "$unstaged_repo"
printf 'two\n' >>"$unstaged_repo/f.txt"
unstaged_output="$tmp_dir/unstaged.out"
run_helper "$unstaged_repo" "$unstaged_output"
assert_contains "$unstaged_output" 'diff_source: unstaged changes via git diff'
assert_contains "$unstaged_output" 'files_changed: 1 files, 1 insertions(+), 0 deletions(-)'

mixed_repo="$tmp_dir/mixed"
mkdir -p "$mixed_repo"
init_repo "$mixed_repo"
printf 'two\n' >>"$mixed_repo/f.txt"
git -C "$mixed_repo" add f.txt
printf 'three\n' >>"$mixed_repo/f.txt"
mixed_output="$tmp_dir/mixed.out"
run_helper "$mixed_repo" "$mixed_output"
assert_contains "$mixed_output" 'unstaged changes touch files also staged for commit'
assert_contains "$mixed_output" '## Staged Files With Unstaged Changes Too'

untracked_repo="$tmp_dir/untracked"
mkdir -p "$untracked_repo"
init_repo "$untracked_repo"
printf 'new\n' >"$untracked_repo/new.txt"
untracked_output="$tmp_dir/untracked.out"
run_helper "$untracked_repo" "$untracked_output"
assert_contains "$untracked_output" 'untracked_files: yes'
assert_contains "$untracked_output" 'untracked files exist but are not part of git diff'

truncated_repo="$tmp_dir/truncated"
mkdir -p "$truncated_repo"
init_repo "$truncated_repo"
printf 'two\nthree\nfour\n' >>"$truncated_repo/f.txt"
git -C "$truncated_repo" add f.txt
truncated_output="$tmp_dir/truncated.out"
(
  cd "$truncated_repo"
  PRE_COMMIT_REVIEW_MAX_DIFF_BYTES=20 "$helper"
) >"$truncated_output" 2>&1
assert_contains "$truncated_output" 'review_limits: partial diff output'
assert_contains "$truncated_output" '[diff truncated after 20 bytes; inspect high-risk files with file-specific git diff commands before making safety claims]'

risk_queue_repo="$tmp_dir/risk-queue"
mkdir -p "$risk_queue_repo"
init_repo "$risk_queue_repo"
mkdir -p "$risk_queue_repo/snapshots" "$risk_queue_repo/zzz_auth" "$risk_queue_repo/db/migrations"
for i in $(seq 1 80); do
  printf 'snapshot line %s\n' "$i"
done >"$risk_queue_repo/snapshots/large.snap"
printf 'def allow(user):\n    return True\n' >"$risk_queue_repo/zzz_auth/session.py"
printf 'alter table users add column admin boolean default false;\n' >"$risk_queue_repo/db/migrations/20260518_add_admin.sql"
git -C "$risk_queue_repo" add snapshots/large.snap zzz_auth/session.py db/migrations/20260518_add_admin.sql
risk_queue_output="$tmp_dir/risk-queue.out"
(
  cd "$risk_queue_repo"
  PRE_COMMIT_REVIEW_MAX_DIFF_BYTES=80 "$helper"
) >"$risk_queue_output" 2>&1
assert_contains "$risk_queue_output" 'diff_truncated: yes'
assert_contains "$risk_queue_output" 'high_risk_candidates: db/migrations/20260518_add_admin.sql, zzz_auth/session.py'
assert_contains "$risk_queue_output" 'generated_like_files: snapshots/large.snap'
assert_contains "$risk_queue_output" '## Suggested Review Queue'
assert_contains "$risk_queue_output" 'high-risk: db/migrations/20260518_add_admin.sql'
assert_contains "$risk_queue_output" 'high-risk: zzz_auth/session.py'

content_risk_repo="$tmp_dir/content-risk"
mkdir -p "$content_risk_repo/src"
init_repo "$content_risk_repo"
printf 'def allowed(request):\n    return request.headers.get("Authorization") == "admin"\n' >"$content_risk_repo/src/service.py"
git -C "$content_risk_repo" add src/service.py
content_risk_output="$tmp_dir/content-risk.out"
run_helper "$content_risk_repo" "$content_risk_output"
assert_contains "$content_risk_output" 'content_risk_candidates: src/service.py'
assert_contains "$content_risk_output" 'high_risk_candidates: src/service.py'
assert_contains "$content_risk_output" 'high-risk: src/service.py'
assert_contains "$content_risk_output" '## Review Manifest'
assert_contains "$content_risk_output" $'unit_id\tpath\tstatus\tadditions\tdeletions\tdiff_bytes\trisk_tags\tgroup_id\treview_command\tcontext_command'
assert_contains "$content_risk_output" $'file:src/service.py\tsrc/service.py\tA\t2\t0\t'
assert_contains "$content_risk_output" $'high-risk\thigh-risk-src'
assert_contains "$content_risk_output" 'git diff --cached -- src/service.py'
assert_contains "$content_risk_output" '## Review Manifest JSONL'
assert_contains "$content_risk_output" '"unit_id":"file:src/service.py"'
assert_contains "$content_risk_output" '"risk_tags":["high-risk"]'
assert_contains "$content_risk_output" '## Review Groups JSONL'
assert_contains "$content_risk_output" '"group_id":"high-risk-src"'
assert_contains "$content_risk_output" '"files":["src/service.py"]'
assert_jsonl_section_valid "$content_risk_output" 'Review Manifest JSONL'
assert_jsonl_section_valid "$content_risk_output" 'Review Groups JSONL'
assert_contains "$content_risk_output" '## Review Plan JSON'
assert_contains "$content_risk_output" '"schema_version":1'
assert_contains "$content_risk_output" '"context_mode":"group"'
assert_contains "$content_risk_output" '"state_snapshot_section":"Reducer State Snapshot Template"'
assert_contains "$content_risk_output" '"semantic_context_section":"Semantic Context Queries"'
assert_contains "$content_risk_output" '"context_command":"'"$repo_root"'/scripts/collect_diff_context.sh --source staged --group high-risk-src"'
assert_json_section_valid "$content_risk_output" 'Review Plan JSON'
assert_contains "$content_risk_output" '## Coverage Ledger Template'
assert_contains "$content_risk_output" $'unit_id\tgroup_id\tpath\tcoverage_status\tcoverage_mode\tnotes'
assert_contains "$content_risk_output" $'file:src/service.py\thigh-risk-src\tsrc/service.py\tpending\tfile-review\trecord group result before final verdict'
assert_contains "$content_risk_output" '## Group Review Result Template'
assert_contains "$content_risk_output" '"group_id":"high-risk-src"'
assert_contains "$content_risk_output" '"required_units":["file:src/service.py"]'
assert_contains "$content_risk_output" '"coverage":"pending"'
assert_contains "$content_risk_output" '"findings":[]'
assert_contains "$content_risk_output" '## Reducer State Snapshot Template'
assert_contains "$content_risk_output" '"state_kind":"reducer_state_snapshot"'
assert_contains "$content_risk_output" '"pending_units":["file:src/service.py"]'
assert_contains "$content_risk_output" '"final_verdict":"blocked_until_coverage_validation_passes"'
assert_contains "$content_risk_output" '"persistence_rule":"carry this compact state forward after each group result'
assert_json_section_valid "$content_risk_output" 'Reducer State Snapshot Template'
assert_contains "$content_risk_output" '## Coverage Validation Checklist'
assert_contains "$content_risk_output" 'manifest_units: 1'
assert_contains "$content_risk_output" 'review_groups: 1'
assert_contains "$content_risk_output" 'high_risk_units: 1'
assert_contains "$content_risk_output" 'validation_rule: manifest_units - reviewed_units must be empty before claiming full review'
assert_contains "$content_risk_output" '## Full Review Execution Plan'
assert_contains "$content_risk_output" $'step\taction\tgroup_id\trisk\tbudget_status\tunits\tnotes'
assert_contains "$content_risk_output" $'1\treview\thigh-risk-src\thigh\tok\tfile:src/service.py\treview-complete-group-before-coverage-validation'
assert_contains "$content_risk_output" '## Group Review Work Packets'
assert_contains "$content_risk_output" 'group_id: high-risk-src'
assert_contains "$content_risk_output" 'required_units: file:src/service.py'
assert_contains "$content_risk_output" 'review_commands: git diff --cached -- src/service.py'
assert_contains "$content_risk_output" '--source staged --group high-risk-src'
assert_contains "$content_risk_output" '--source staged --path src/service.py'
assert_contains "$content_risk_output" '## Reducer Finalization Template'
assert_contains "$content_risk_output" '"coverage_validation":"required"'
assert_contains "$content_risk_output" '"cross_file_reduction":"required_after_coverage_validation"'
assert_contains "$content_risk_output" '"final_verdict":"blocked_until_coverage_validation_passes"'
assert_contains "$content_risk_output" '"residual_risks":[]'
assert_contains "$content_risk_output" '## Semantic Context Queries'
assert_contains "$content_risk_output" $'none\tnone\t0\tno context queries configured'

space_path_repo="$tmp_dir/space-path"
mkdir -p "$space_path_repo/docs"
init_repo "$space_path_repo"
printf 'hello\n' >"$space_path_repo/docs/file with space.md"
git -C "$space_path_repo" add 'docs/file with space.md'
space_path_output="$tmp_dir/space-path.out"
run_helper "$space_path_repo" "$space_path_output"
assert_contains "$space_path_output" 'review_commands: git diff --cached -- docs/file\ with\ space.md'
assert_contains "$space_path_output" 'context_command: '
assert_contains "$space_path_output" '--source staged --path docs/file\ with\ space.md'
assert_contains "$space_path_output" 'top-churn: docs/file with space.md (+1/-0)'

comma_path_repo="$tmp_dir/comma-path"
mkdir -p "$comma_path_repo/docs"
init_repo "$comma_path_repo"
printf 'hello\n' >"$comma_path_repo/docs/file,with,comma.md"
git -C "$comma_path_repo" add 'docs/file,with,comma.md'
comma_path_output="$tmp_dir/comma-path.out"
run_helper "$comma_path_repo" "$comma_path_output"
assert_contains "$comma_path_output" $'file:docs/file,with,comma.md\tdocs/file,with,comma.md\tA\t1\t0\t'
assert_contains "$comma_path_output" $'module-docs\tmedium\tmodule\t'
assert_contains "$comma_path_output" '## Review Manifest JSONL'
assert_contains "$comma_path_output" '"path":"docs/file,with,comma.md"'
assert_contains "$comma_path_output" '"review_command":"git diff --cached -- docs/file\\,with\\,comma.md"'
assert_contains "$comma_path_output" '## Review Groups JSONL'
assert_contains "$comma_path_output" '"files":["docs/file,with,comma.md"]'
assert_jsonl_section_valid "$comma_path_output" 'Review Manifest JSONL'
assert_jsonl_section_valid "$comma_path_output" 'Review Groups JSONL'
assert_json_section_valid "$comma_path_output" 'Review Plan JSON'
assert_contains "$comma_path_output" 'group_id: module-docs'
assert_contains "$comma_path_output" 'required_units: file:docs/file,with,comma.md'
assert_contains "$comma_path_output" 'review_commands: git diff --cached -- docs/file\,with\,comma.md'

comma_risk_repo="$tmp_dir/comma-risk-path"
mkdir -p "$comma_risk_repo/src" "$comma_risk_repo/snapshots"
init_repo "$comma_risk_repo"
printf 'def allowed(request):\n    return request.headers.get("Authorization") == "admin"\n' >"$comma_risk_repo/src/needs,review.py"
printf 'snapshot\n' >"$comma_risk_repo/snapshots/value,with,comma.snap"
git -C "$comma_risk_repo" add 'src/needs,review.py' 'snapshots/value,with,comma.snap'
comma_risk_output="$tmp_dir/comma-risk-path.out"
run_helper "$comma_risk_repo" "$comma_risk_output"
assert_contains "$comma_risk_output" $'file:src/needs,review.py\tsrc/needs,review.py\tA\t2\t0\t'
assert_contains "$comma_risk_output" $'file:snapshots/value,with,comma.snap\tsnapshots/value,with,comma.snap\tA\t1\t0\t'
assert_contains "$comma_risk_output" $'high-risk\thigh-risk-src'
assert_contains "$comma_risk_output" $'generated-like\tconsistency-snapshots'
assert_contains "$comma_risk_output" '"path":"src/needs,review.py"'
assert_contains "$comma_risk_output" '"path":"snapshots/value,with,comma.snap"'
assert_contains "$comma_risk_output" '## Dependency Summary'
assert_contains "$comma_risk_output" $'src/needs,review.py\tadded\tsignature\tdef allowed(request):'
assert_not_contains "$comma_risk_output" 'src/needs,review.py,added,signature'
assert_jsonl_section_valid "$comma_risk_output" 'Review Manifest JSONL'
assert_json_section_valid "$comma_risk_output" 'Review Plan JSON'

expanded_risk_repo="$tmp_dir/expanded-risk"
mkdir -p "$expanded_risk_repo/lib/crypto" "$expanded_risk_repo/src"
init_repo "$expanded_risk_repo"
printf 'package crypto\nfunc Hash(value string) string { return value }\n' >"$expanded_risk_repo/lib/crypto/hash.go"
printf 'export function run(userInput: string) {\n  return eval(userInput);\n}\n' >"$expanded_risk_repo/src/runner.ts"
git -C "$expanded_risk_repo" add lib/crypto/hash.go src/runner.ts
expanded_risk_output="$tmp_dir/expanded-risk.out"
run_helper "$expanded_risk_repo" "$expanded_risk_output"
assert_contains "$expanded_risk_output" $'file:lib/crypto/hash.go\tlib/crypto/hash.go\tA\t2\t0\t'
assert_contains "$expanded_risk_output" $'file:src/runner.ts\tsrc/runner.ts\tA\t3\t0\t'
assert_contains "$expanded_risk_output" $'high-risk\thigh-risk-lib'
assert_contains "$expanded_risk_output" $'high-risk\thigh-risk-src'
assert_contains "$expanded_risk_output" 'high-risk: lib/crypto/hash.go'
assert_contains "$expanded_risk_output" 'high-risk: src/runner.ts'

configured_risk_repo="$tmp_dir/configured-risk"
mkdir -p "$configured_risk_repo/.pre-commit-review" "$configured_risk_repo/internal/service" "$configured_risk_repo/src"
init_repo "$configured_risk_repo"
printf '# project-specific path risk\n^internal/service/\n' >"$configured_risk_repo/.pre-commit-review/risk-paths"
printf '# project-specific content risk\nDangerousThing\n' >"$configured_risk_repo/.pre-commit-review/risk-content"
git -C "$configured_risk_repo" add .pre-commit-review/risk-paths .pre-commit-review/risk-content
git -C "$configured_risk_repo" commit -q -m risk-config
printf 'def ordinary():\n    return True\n' >"$configured_risk_repo/internal/service/ordinary.py"
printf 'export const marker = "DangerousThing";\n' >"$configured_risk_repo/src/plain.ts"
git -C "$configured_risk_repo" add internal/service/ordinary.py src/plain.ts
configured_risk_output="$tmp_dir/configured-risk.out"
run_helper "$configured_risk_repo" "$configured_risk_output"
assert_contains "$configured_risk_output" $'file:internal/service/ordinary.py\tinternal/service/ordinary.py\tA\t2\t0\t'
assert_contains "$configured_risk_output" $'file:src/plain.ts\tsrc/plain.ts\tA\t1\t0\t'
assert_contains "$configured_risk_output" $'high-risk\thigh-risk-internal'
assert_contains "$configured_risk_output" $'high-risk\thigh-risk-src'
assert_contains "$configured_risk_output" 'high-risk: internal/service/ordinary.py'
assert_contains "$configured_risk_output" 'high-risk: src/plain.ts'

context_query_repo="$tmp_dir/context-query"
mkdir -p "$context_query_repo/.pre-commit-review" "$context_query_repo/src"
init_repo "$context_query_repo"
printf '# read-only semantic context query\nvalidate_token\n' >"$context_query_repo/.pre-commit-review/context-queries"
git -C "$context_query_repo" add .pre-commit-review/context-queries
git -C "$context_query_repo" commit -q -m context-queries
printf 'def validate_token(token):\n    return token\n' >"$context_query_repo/src/auth.py"
git -C "$context_query_repo" add src/auth.py
context_query_output="$tmp_dir/context-query.out"
run_helper "$context_query_repo" "$context_query_output"
assert_contains "$context_query_output" '## Semantic Context Queries'
assert_contains "$context_query_output" $'query\tfile\tline\tmatch'
assert_contains "$context_query_output" $'validate_token\tsrc/auth.py\t1\tdef validate_token(token):'

space_path_specific_output="$tmp_dir/space-path-specific.out"
(
  cd "$space_path_repo"
  "$helper" --source staged --path 'docs/file with space.md'
) >"$space_path_specific_output" 2>&1
assert_contains "$space_path_specific_output" 'requested_path: docs/file with space.md'
assert_contains "$space_path_specific_output" 'requested_source: staged'
assert_contains "$space_path_specific_output" 'review_limits: file-specific diff for requested path; no other files included'
assert_contains "$space_path_specific_output" '## Requested File Diff'
assert_contains "$space_path_specific_output" 'review_command: git diff --cached -- docs/file\ with\ space.md'
assert_contains "$space_path_specific_output" '+hello'
if grep -Fq '## Review Manifest' "$space_path_specific_output"; then
  fail 'file-specific context mode must not emit the full review manifest'
fi

group_context_repo="$tmp_dir/group-context"
mkdir -p "$group_context_repo/src"
init_repo "$group_context_repo"
printf 'alpha\n' >"$group_context_repo/src/a.py"
printf 'beta\n' >"$group_context_repo/src/b.py"
git -C "$group_context_repo" add src/a.py src/b.py
group_context_output="$tmp_dir/group-context.out"
run_helper "$group_context_repo" "$group_context_output"
assert_contains "$group_context_output" '--source staged --group module-src'
group_context_specific_output="$tmp_dir/group-context-specific.out"
(
  cd "$group_context_repo"
  "$helper" --source staged --group module-src
) >"$group_context_specific_output" 2>&1
assert_contains "$group_context_specific_output" 'requested_group: module-src'
assert_contains "$group_context_specific_output" 'requested_source: staged'
assert_contains "$group_context_specific_output" 'review_limits: group-specific diff for requested group; no other groups included'
assert_contains "$group_context_specific_output" '## Requested Group Files'
assert_contains "$group_context_specific_output" $'A\tsrc/a.py\tfile:src/a.py'
assert_contains "$group_context_specific_output" $'A\tsrc/b.py\tfile:src/b.py'
assert_contains "$group_context_specific_output" '## Requested Group Diff'
assert_contains "$group_context_specific_output" 'group_id: module-src'
assert_contains "$group_context_specific_output" 'required_units: file:src/a.py;file:src/b.py'
assert_contains "$group_context_specific_output" '+alpha'
assert_contains "$group_context_specific_output" '+beta'
if grep -Fq '## Review Manifest' "$group_context_specific_output"; then
  fail 'group-specific context mode must not emit the full review manifest'
fi

source_lock_repo="$tmp_dir/source-lock"
mkdir -p "$source_lock_repo"
init_repo "$source_lock_repo"
printf 'staged\n' >"$source_lock_repo/f.txt"
git -C "$source_lock_repo" add f.txt
printf 'unstaged\n' >"$source_lock_repo/f.txt"
source_lock_output="$tmp_dir/source-lock.out"
run_helper "$source_lock_repo" "$source_lock_output"
assert_contains "$source_lock_output" 'diff_source: staged changes via git diff --cached'
assert_contains "$source_lock_output" '--source staged --path f.txt'

source_lock_staged_output="$tmp_dir/source-lock-staged.out"
(
  cd "$source_lock_repo"
  "$helper" --source staged --path f.txt
) >"$source_lock_staged_output" 2>&1
assert_contains "$source_lock_staged_output" 'requested_source: staged'
assert_contains "$source_lock_staged_output" 'diff_source: staged changes via git diff --cached'
assert_contains "$source_lock_staged_output" '+staged'
assert_not_contains "$source_lock_staged_output" '+unstaged'

source_lock_unstaged_output="$tmp_dir/source-lock-unstaged.out"
(
  cd "$source_lock_repo"
  "$helper" --source unstaged --path f.txt
) >"$source_lock_unstaged_output" 2>&1
assert_contains "$source_lock_unstaged_output" 'requested_source: unstaged'
assert_contains "$source_lock_unstaged_output" 'diff_source: unstaged changes via git diff'
assert_contains "$source_lock_unstaged_output" '+unstaged'

branch_source_repo="$tmp_dir/branch-source"
mkdir -p "$branch_source_repo"
init_repo "$branch_source_repo"
git -C "$branch_source_repo" checkout -q -b feature
printf 'branch\n' >"$branch_source_repo/f.txt"
git -C "$branch_source_repo" add f.txt
git -C "$branch_source_repo" commit -q -m branch-change
branch_source_output="$tmp_dir/branch-source.out"
run_helper "$branch_source_repo" "$branch_source_output"
assert_contains "$branch_source_output" 'diff_source: branch vs local base via git diff'
assert_contains "$branch_source_output" '--source branch --path f.txt'

branch_source_specific_output="$tmp_dir/branch-source-specific.out"
(
  cd "$branch_source_repo"
  "$helper" --source branch --path f.txt
) >"$branch_source_specific_output" 2>&1
assert_contains "$branch_source_specific_output" 'requested_source: branch'
assert_contains "$branch_source_specific_output" 'diff_source: branch vs local base via git diff'
assert_contains "$branch_source_specific_output" '+branch'

special_change_repo="$tmp_dir/special-change"
mkdir -p "$special_change_repo"
init_repo "$special_change_repo"
printf 'remove me\n' >"$special_change_repo/delete.txt"
printf 'rename me\n' >"$special_change_repo/old.txt"
printf '#!/bin/sh\necho hi\n' >"$special_change_repo/mode.sh"
printf '\001\002base\000\n' >"$special_change_repo/binary.bin"
git -C "$special_change_repo" add delete.txt old.txt mode.sh binary.bin
git -C "$special_change_repo" commit -q -m special-baseline
git -C "$special_change_repo" mv old.txt new.txt
rm "$special_change_repo/delete.txt"
chmod +x "$special_change_repo/mode.sh"
printf '\003\004changed\000\n' >"$special_change_repo/binary.bin"
git -C "$special_change_repo" add -A
special_change_output="$tmp_dir/special-change.out"
run_helper "$special_change_repo" "$special_change_output"
assert_contains "$special_change_output" $'file:new.txt\tnew.txt\tR100\t0\t0\t'
assert_contains "$special_change_output" '"status":"R100"'
assert_contains "$special_change_output" $'file:delete.txt\tdelete.txt\tD\t0\t1\t'
assert_contains "$special_change_output" $'file:mode.sh\tmode.sh\tM\t0\t0\t'
assert_contains "$special_change_output" $'file:binary.bin\tbinary.bin\tM\t0\t0\t'
assert_contains "$special_change_output" 'required_units: file:mode.sh'
assert_contains "$special_change_output" 'required_units: file:binary.bin'
assert_jsonl_section_valid "$special_change_output" 'Review Manifest JSONL'

rename_risk_repo="$tmp_dir/rename-risk"
mkdir -p "$rename_risk_repo/auth"
init_repo "$rename_risk_repo"
printf 'def allow(user):\n    return True\n' >"$rename_risk_repo/session_old.py"
git -C "$rename_risk_repo" add session_old.py
git -C "$rename_risk_repo" commit -q -m rename-risk-baseline
git -C "$rename_risk_repo" mv session_old.py auth/session.py
rename_risk_output="$tmp_dir/rename-risk.out"
run_helper "$rename_risk_repo" "$rename_risk_output"
assert_contains "$rename_risk_output" $'file:auth/session.py\tauth/session.py\tR100\t0\t0\t'
assert_contains "$rename_risk_output" $'high-risk\thigh-risk-auth'
assert_contains "$rename_risk_output" '"group_id":"high-risk-auth"'
assert_jsonl_section_valid "$rename_risk_output" 'Review Manifest JSONL'
assert_json_section_valid "$rename_risk_output" 'Review Plan JSON'

submodule_remote_repo="$tmp_dir/submodule-remote"
mkdir -p "$submodule_remote_repo"
init_repo "$submodule_remote_repo"
submodule_parent_repo="$tmp_dir/submodule-parent"
mkdir -p "$submodule_parent_repo"
init_repo "$submodule_parent_repo"
git -C "$submodule_parent_repo" -c protocol.file.allow=always submodule add "$submodule_remote_repo" vendor/sub >/dev/null
git -C "$submodule_parent_repo" commit -q -m add-submodule
git -C "$submodule_parent_repo/vendor/sub" config user.email a@example.com
git -C "$submodule_parent_repo/vendor/sub" config user.name A
printf 'submodule update\n' >>"$submodule_parent_repo/vendor/sub/f.txt"
git -C "$submodule_parent_repo/vendor/sub" add f.txt
git -C "$submodule_parent_repo/vendor/sub" commit -q -m submodule-update
git -C "$submodule_parent_repo" add vendor/sub
submodule_output="$tmp_dir/submodule.out"
run_helper "$submodule_parent_repo" "$submodule_output"
assert_contains "$submodule_output" $'file:vendor/sub\tvendor/sub\tM\t'
assert_contains "$submodule_output" '"path":"vendor/sub"'
assert_contains "$submodule_output" 'required_units: file:vendor/sub'
assert_jsonl_section_valid "$submodule_output" 'Review Manifest JSONL'

assert_contains "$risk_queue_output" '## Review Groups'
assert_contains "$risk_queue_output" $'group_id\trisk\treason\tdiff_bytes\tfiles\tbudget_status'
assert_contains "$risk_queue_output" $'high-risk-db-migrations\thigh\tpath-or-content-risk\t'
assert_contains "$risk_queue_output" $'high-risk-zzz_auth\thigh\tpath-or-content-risk\t'
assert_contains "$risk_queue_output" $'consistency-snapshots\tconsistency\tgenerated-like\t'

group_budget_output="$tmp_dir/group-budget.out"
(
  cd "$risk_queue_repo"
  PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES=200 PRE_COMMIT_REVIEW_GROUP_HARD_BYTES=500 "$helper"
) >"$group_budget_output" 2>&1
assert_contains "$group_budget_output" 'group_target_bytes: 200'
assert_contains "$group_budget_output" 'group_hard_bytes: 500'
assert_contains "$group_budget_output" $'group_id\trisk\treason\tdiff_bytes\tfiles\tbudget_status'
assert_contains "$group_budget_output" $'consistency-snapshots\tconsistency\tgenerated-like\t1590\tsnapshots/large.snap\tsplit-required'
assert_contains "$group_budget_output" $'high-risk-db-migrations\thigh\tpath-or-content-risk\t263\tdb/migrations/20260518_add_admin.sql\tover-target'
assert_contains "$group_budget_output" '## Split Suggestions'
assert_contains "$group_budget_output" $'parent_group_id\tunit_id\tpath\tsplit_kind\tdiff_bytes\thunk_header\treview_command'
assert_contains "$group_budget_output" $'consistency-snapshots\thunk:snapshots/large.snap:1\tsnapshots/large.snap\thunk\t'
assert_contains "$group_budget_output" '## Split Unit Diff Preview'
assert_contains "$group_budget_output" 'unit_id: hunk:snapshots/large.snap:1'
assert_contains "$group_budget_output" $'file:snapshots/large.snap\tconsistency-snapshots\tsnapshots/large.snap\tneeds-split\treplace-with-split-suggestions\tsplit-required group'
assert_contains "$group_budget_output" '"group_id":"consistency-snapshots"'
assert_contains "$group_budget_output" '"coverage":"needs-split"'
assert_contains "$group_budget_output" 'split_required_groups: 1'
assert_contains "$group_budget_output" 'needs_split_units: 1'
assert_contains "$group_budget_output" 'blocking_rule: high-risk or needs-split coverage gaps force DO_NOT_COMMIT'
assert_contains "$group_budget_output" $'1\tsplit\tconsistency-snapshots\tconsistency\tsplit-required\tfile:snapshots/large.snap\treplace-with-split-suggestions-before-review'
assert_contains "$group_budget_output" $'2\treview\thigh-risk-db-migrations\thigh\tover-target\tfile:db/migrations/20260518_add_admin.sql\treview-complete-group-before-coverage-validation'
assert_contains "$group_budget_output" 'group_id: consistency-snapshots'
assert_contains "$group_budget_output" 'budget_status: split-required'
assert_contains "$group_budget_output" 'split_source: Split Suggestions and Split Unit Diff Preview'
assert_contains "$group_budget_output" 'context_command: '
assert_contains "$group_budget_output" '--source staged --group high-risk-db-migrations'
assert_review_plan_group_order "$group_budget_output" consistency-snapshots high-risk-db-migrations high-risk-zzz_auth
assert_work_packets_start_with_group_id "$group_budget_output"

split_group_specific_output="$tmp_dir/split-group-specific.out"
(
  cd "$risk_queue_repo"
  PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES=200 PRE_COMMIT_REVIEW_GROUP_HARD_BYTES=500 "$helper" --source staged --group consistency-snapshots
) >"$split_group_specific_output" 2>&1
assert_contains "$split_group_specific_output" 'requested_group: consistency-snapshots'
assert_contains "$split_group_specific_output" '## Requested Group Files'
assert_contains "$split_group_specific_output" $'A\tsnapshots/large.snap\tfile:snapshots/large.snap'
assert_contains "$split_group_specific_output" 'budget_status: split-required'
assert_contains "$split_group_specific_output" 'Group exceeds hard review budget; use split suggestions instead of reviewing it as one group.'
assert_contains "$split_group_specific_output" '## Split Suggestions'
assert_contains "$split_group_specific_output" $'consistency-snapshots\thunk:snapshots/large.snap:1\tsnapshots/large.snap\thunk\t'
assert_not_contains "$split_group_specific_output" '## Diff'

split_repo="$tmp_dir/split-hunks"
mkdir -p "$split_repo"
init_repo "$split_repo"
for i in $(seq 1 40); do
  printf 'line %s\n' "$i"
done >"$split_repo/f.txt"
git -C "$split_repo" add f.txt
git -C "$split_repo" commit -q -m baseline
sed -i.bak '5s/.*/changed five/' "$split_repo/f.txt"
sed -i.bak '35s/.*/changed thirty five/' "$split_repo/f.txt"
rm -f "$split_repo/f.txt.bak"
git -C "$split_repo" add f.txt
split_output="$tmp_dir/split-hunks.out"
(
  cd "$split_repo"
  PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES=80 PRE_COMMIT_REVIEW_GROUP_HARD_BYTES=120 "$helper"
) >"$split_output" 2>&1
assert_contains "$split_output" $'module-f.txt\tmedium\tmodule\t'
assert_contains "$split_output" 'split-required'
assert_contains "$split_output" $'module-f.txt\thunk:f.txt:1\tf.txt\thunk\t'
assert_contains "$split_output" $'module-f.txt\thunk:f.txt:2\tf.txt\thunk\t'
assert_contains "$split_output" 'unit_id: hunk:f.txt:1'
assert_contains "$split_output" '+changed five'
assert_contains "$split_output" 'unit_id: hunk:f.txt:2'
assert_contains "$split_output" '+changed thirty five'

dependency_repo="$tmp_dir/dependency-summary"
mkdir -p "$dependency_repo/src"
init_repo "$dependency_repo"
printf 'export function getUser(id: string) {\n  return { id };\n}\n' >"$dependency_repo/src/api.ts"
printf 'import { getUser } from "./api";\nexport function renderUser(id: string) {\n  return getUser(id).id;\n}\n' >"$dependency_repo/src/client.ts"
git -C "$dependency_repo" add src/api.ts src/client.ts
dependency_output="$tmp_dir/dependency-summary.out"
run_helper "$dependency_repo" "$dependency_output"
assert_contains "$dependency_output" '## Dependency Summary'
assert_contains "$dependency_output" $'file\tchange\tkind\tdetail'
assert_contains "$dependency_output" $'src/api.ts\tadded\texport\texport function getUser(id: string) {'
assert_contains "$dependency_output" $'src/api.ts\tadded\tsignature\texport function getUser(id: string) {'
assert_contains "$dependency_output" $'src/client.ts\tadded\timport\timport { getUser } from "./api";'
assert_contains "$dependency_output" $'src/client.ts\tadded\texport\texport function renderUser(id: string) {'
assert_not_contains "$dependency_output" 'file,change,kind,detail'

printf 'collect_diff_context tests passed\n'
