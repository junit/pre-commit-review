#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
. "$script_dir/host_failure_taxonomy.sh"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
output_eval_file="$repo_root/evals/output-eval.json"

runner_command=''
responses_dir=''
fixtures_dir=''
case_filter=''
keep_fixtures='no'
manifest_file=''

usage() {
  cat <<'USAGE'
Usage: output_eval_runner.sh [options]

Prepare pre-commit-review output-eval fixtures, optionally run an external model runner,
and grade saved responses against an eval JSON file.

Options:
  --runner CMD         Shell command used to produce one response per case.
                       The command runs inside the case workdir with environment variables:
                       PRE_COMMIT_REVIEW_EVAL_CASE_ID
                       PRE_COMMIT_REVIEW_EVAL_SCENARIO
                       PRE_COMMIT_REVIEW_EVAL_LOCALE
                       PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE
                       PRE_COMMIT_REVIEW_EVAL_METADATA_FILE
                       PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE
                       PRE_COMMIT_REVIEW_EVAL_SKILL_DIR
                       Case-specific helper env vars are also exported when configured.
  --responses-dir DIR  Directory containing or receiving response files named <case-id>.md
  --fixtures-dir DIR   Directory where case fixtures will be prepared
  --eval-file FILE     Eval JSON file to prepare and grade. Defaults to evals/output-eval.json
                       Use a layered eval file such as evals/output/visual-output-eval.json
                       to run one output matrix directly.
  --case SCENARIO      Run one scenario only
  --manifest FILE      Write a JSON manifest describing the prepared fixtures
  --keep-fixtures      Do not remove the auto-created temporary fixtures directory
  -h, --help           Show this help
USAGE
}

fail() {
  printf 'output eval runner failed: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

init_repo() {
  local dir="$1"

  git -C "$dir" init -q
  git -C "$dir" config user.email a@example.com
  git -C "$dir" config user.name A
  printf 'baseline\n' >"$dir/base.txt"
  git -C "$dir" add base.txt
  git -C "$dir" commit -q -m init
}

write_prompt_file() {
  local prompt_file="$1"
  local prompt="$2"
  local fixture="$3"
  local locale="$4"

  printf 'Prompt:\n%s\n\nFixture:\n%s\n\nLocale:\n%s\n' \
    "$prompt" "$fixture" "$locale" >"$prompt_file"
}

write_metadata_file() {
  local metadata_file="$1"
  local case_json="$2"
  local workdir="$3"
  local prompt_file="$4"
  local response_file="$5"
  local env_json="$6"

  jq -n \
    --argjson case "$case_json" \
    --arg workdir "$workdir" \
    --arg prompt_file "$prompt_file" \
    --arg response_file "$response_file" \
    --arg skill_dir "$repo_root" \
    --argjson env "$env_json" \
    '
    {
      id: $case.id,
      scenario: $case.scenario,
      locale: $case.locale,
      prompt: $case.prompt,
      fixture: $case.fixture,
      expected: $case.expected,
      workdir: $workdir,
      prompt_file: $prompt_file,
      response_file: $response_file,
      skill_dir: $skill_dir,
      env: $env
    }
    ' >"$metadata_file"
}

build_case_tiny_docs() {
  local workdir="$1"

  mkdir -p "$workdir"
  init_repo "$workdir"
  printf '# Project\n\nOld wording.\n\nUsage note.\n' >"$workdir/README.md"
  git -C "$workdir" add README.md
  git -C "$workdir" commit -q -m docs-baseline
  printf '# Project\n\nClearer wording.\n\nUpdated usage note.\n' >"$workdir/README.md"
  git -C "$workdir" add README.md
}

build_case_mixed_staged_unstaged() {
  local workdir="$1"

  mkdir -p "$workdir/src"
  init_repo "$workdir"
  printf 'export function parseUser(input) {\n  return input.trim();\n}\n' >"$workdir/src/user.ts"
  git -C "$workdir" add src/user.ts
  git -C "$workdir" commit -q -m code-baseline
  printf 'export function parseUser(input) {\n  return input.trim().toLowerCase();\n}\n' >"$workdir/src/user.ts"
  git -C "$workdir" add src/user.ts
  printf '\nexport function debugUser(input) {\n  return input;\n}\n' >>"$workdir/src/user.ts"
}

build_case_unstaged_only() {
  local workdir="$1"

  mkdir -p "$workdir/src"
  init_repo "$workdir"
  printf 'export function parseUser(input) {\n  return input.trim();\n}\n' >"$workdir/src/user.ts"
  git -C "$workdir" add src/user.ts
  git -C "$workdir" commit -q -m code-baseline
  printf 'export function parseUser(input) {\n  return input.trim().toLowerCase();\n}\n' >"$workdir/src/user.ts"
}


build_case_hardcoded_secret() {
  local workdir="$1"

  mkdir -p "$workdir/config"
  init_repo "$workdir"
  printf 'export const apiBase = process.env.API_BASE;\n' >"$workdir/config/runtime.ts"
  git -C "$workdir" add config/runtime.ts
  git -C "$workdir" commit -q -m config-baseline
  printf 'export const apiBase = process.env.API_BASE;\nexport const serviceToken = "sk_live_1234567890example";\n' >"$workdir/config/runtime.ts"
  git -C "$workdir" add config/runtime.ts
}

build_case_breaking_api() {
  local workdir="$1"

  mkdir -p "$workdir/src"
  init_repo "$workdir"
  printf 'export function toApiUser(user) {\n  return { id: user.id, displayName: user.name };\n}\n' >"$workdir/src/api.ts"
  git -C "$workdir" add src/api.ts
  git -C "$workdir" commit -q -m api-baseline
  printf 'export function toApiUser(user) {\n  return { id: user.id, fullName: user.name };\n}\n' >"$workdir/src/api.ts"
  git -C "$workdir" add src/api.ts
}

build_case_large_generated() {
  local workdir="$1"

  mkdir -p "$workdir/snapshots"
  init_repo "$workdir"
  for i in $(seq 1 160); do
    printf 'snapshot line %s\n' "$i"
  done >"$workdir/snapshots/component.snap"
  git -C "$workdir" add snapshots/component.snap
}

build_case_full_review_split_reducer() {
  local workdir="$1"

  mkdir -p "$workdir/snapshots" "$workdir/zzz_auth" "$workdir/db/migrations"
  init_repo "$workdir"
  for i in $(seq 1 80); do
    printf 'snapshot line %s\n' "$i"
  done >"$workdir/snapshots/large.snap"
  printf 'def allow(user):\n    return True\n' >"$workdir/zzz_auth/session.py"
  printf 'alter table legacy_users drop column password_hash;\n' >"$workdir/db/migrations/20260518_drop_legacy_users.sql"
  git -C "$workdir" add snapshots/large.snap zzz_auth/session.py db/migrations/20260518_drop_legacy_users.sql
}

build_case_auth_execution_point() {
  local workdir="$1"

  mkdir -p "$workdir/src"
  init_repo "$workdir"
  printf 'export type Role = "member" | "owner";\n\nexport function requireOrgRole(actor, orgId: string, role: Role) {\n  if (!actor || !actor.orgRoles || actor.orgRoles[orgId] !== role) {\n    throw new Error("forbidden");\n  }\n}\n' >"$workdir/src/auth.ts"
  printf 'import { grantOrgAdmin } from "./service";\n\nexport async function postGrantAdmin(req) {\n  const { orgId, targetUserId } = req.body;\n  return grantOrgAdmin(req.actor, orgId, targetUserId);\n}\n' >"$workdir/src/controller.ts"
  printf 'import { requireOrgRole } from "./auth";\n\nexport async function grantOrgAdmin(actor, orgId: string, targetUserId: string) {\n  requireOrgRole(actor, orgId, "owner");\n  return { orgId, targetUserId, role: "admin" };\n}\n' >"$workdir/src/service.ts"
  git -C "$workdir" add src/auth.ts src/controller.ts src/service.ts
}

build_case_negative_search_cross_module() {
  local workdir="$1"

  mkdir -p "$workdir/src/session"
  init_repo "$workdir"
  printf 'export function createSession(userId: string, now: string) {\n  return { userId, createdAt: now, lastSeenAt: now };\n}\n' >"$workdir/src/session/create.ts"
  printf 'export function refreshSession(session, now: string) {\n  return { ...session, lastSeenAt: now };\n}\n' >"$workdir/src/session/refresh.ts"
  git -C "$workdir" add src/session/create.ts src/session/refresh.ts
  git -C "$workdir" commit -q -m session-baseline
  printf 'export function refreshSession(session, now: string) {\n  return { ...session, refreshedAt: now };\n}\n' >"$workdir/src/session/refresh.ts"
  git -C "$workdir" add src/session/refresh.ts
}

build_case_framework_behavior_source() {
  local workdir="$1"

  mkdir -p "$workdir/src" "$workdir/vendor/acme-orm"
  init_repo "$workdir"
  printf '# Acme ORM optimistic lock behavior\n\nFor `update(entity, wrapper)`, `OptimisticLockInterceptor` appends `version = entity.version` to the generated `WHERE` clause and increments `entity.version` after a successful update.\n' >"$workdir/vendor/acme-orm/optimistic-lock.md"
  printf 'export async function saveUserName(orm, user) {\n  return orm.update({ id: user.id, name: user.name }, { id: user.id });\n}\n' >"$workdir/src/userRepo.ts"
  git -C "$workdir" add vendor/acme-orm/optimistic-lock.md src/userRepo.ts
  git -C "$workdir" commit -q -m orm-baseline
  printf 'export async function saveUserName(orm, user) {\n  return orm.update({ id: user.id, name: user.name, version: user.version }, { id: user.id });\n}\n' >"$workdir/src/userRepo.ts"
  git -C "$workdir" add src/userRepo.ts
}

build_case_no_git_repo() {
  local workdir="$1"

  mkdir -p "$workdir"
  printf 'not a repository\n' >"$workdir/README.txt"
}

build_case_chinese_request() {
  local workdir="$1"

  mkdir -p "$workdir/src" "$workdir/tests"
  init_repo "$workdir"
  printf 'export function validateUser(input) {\n  return Boolean(input && input.name);\n}\n' >"$workdir/src/user.ts"
  printf 'import { validateUser } from "../src/user";\n\ntest("valid user", () => {\n  expect(validateUser({ name: "A" })).toBe(true);\n});\n' >"$workdir/tests/user.test.ts"
  git -C "$workdir" add src/user.ts tests/user.test.ts
  git -C "$workdir" commit -q -m zh-baseline
  printf 'export function validateUser(input) {\n  return Boolean(input && input.name && input.id);\n}\n' >"$workdir/src/user.ts"
  printf 'import { validateUser } from "../src/user";\n\ntest("valid user", () => {\n  expect(validateUser({ id: "1", name: "A" })).toBe(true);\n});\n' >"$workdir/tests/user.test.ts"
  git -C "$workdir" add src/user.ts tests/user.test.ts
}

build_case_pasted_diff() {
  local workdir="$1"

  mkdir -p "$workdir"
  printf -- '--- a/src/calc.ts\n+++ b/src/calc.ts\n@@ -1,3 +1,4 @@\n export function sum(a, b) {\n+  console.log("debug");\n   return a + b;\n }\n' >"$workdir/pasted.patch"
}

prepare_case_fixture() {
  local case_json="$1"
  local case_dir="$2"

  local case_id scenario prompt fixture locale response_file prompt_file metadata_file workdir env_json

  case_id="$(jq -r '.id' <<<"$case_json")"
  scenario="$(jq -r '.scenario' <<<"$case_json")"
  prompt="$(jq -r '.prompt' <<<"$case_json")"
  fixture="$(jq -r '.fixture' <<<"$case_json")"
  locale="$(jq -r '.locale' <<<"$case_json")"

  workdir="$case_dir/workdir"
  prompt_file="$case_dir/prompt.txt"
  metadata_file="$case_dir/metadata.json"
  response_file="$responses_dir/$case_id.md"
  env_json='{}'

  case "$scenario" in
    tiny-docs) build_case_tiny_docs "$workdir" ;;
    mixed-staged-unstaged) build_case_mixed_staged_unstaged "$workdir" ;;
    unstaged-only) build_case_unstaged_only "$workdir" ;;
    hardcoded-secret) build_case_hardcoded_secret "$workdir" ;;
    breaking-api) build_case_breaking_api "$workdir" ;;
    large-generated)
      build_case_large_generated "$workdir"
      env_json='{"PRE_COMMIT_REVIEW_MAX_DIFF_BYTES":"80"}'
      ;;
    full-review-split-reducer)
      build_case_full_review_split_reducer "$workdir"
      env_json='{"PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES":"200","PRE_COMMIT_REVIEW_GROUP_HARD_BYTES":"500"}'
      ;;
    auth-execution-point) build_case_auth_execution_point "$workdir" ;;
    negative-search-cross-module) build_case_negative_search_cross_module "$workdir" ;;
    framework-behavior-source) build_case_framework_behavior_source "$workdir" ;;
    no-git-repo) build_case_no_git_repo "$workdir" ;;
    chinese-request) build_case_chinese_request "$workdir" ;;
    pasted-diff)
      build_case_pasted_diff "$workdir"
      prompt="$(printf '%s\n\n%s\n\n```diff\n%s\n```\n' "$prompt" 'Use this user-provided diff:' "$(cat "$workdir/pasted.patch")")"
      ;;
    *) fail "unknown scenario: $scenario" ;;
  esac

  mkdir -p "$case_dir" "$responses_dir"
  write_prompt_file "$prompt_file" "$prompt" "$fixture" "$locale"
  write_metadata_file "$metadata_file" "$case_json" "$workdir" "$prompt_file" "$response_file" "$env_json"
}

write_manifest() {
  local fixtures_root="$1"
  local manifest_target="$2"

  jq -n --arg fixtures_root "$fixtures_root" '
    {fixtures_root: $fixtures_root, generated_at: "deterministic-shell-runner"}
  ' >"$manifest_target"
}

grade_case() {
  local case_json="$1"
  local response_file="$2"

  local case_id scenario expected_verdict must_include_file missing=0 actual_verdict=''

  case_id="$(jq -r '.id' <<<"$case_json")"
  scenario="$(jq -r '.scenario' <<<"$case_json")"
  expected_verdict="$(jq -r '.expected.verdict' <<<"$case_json")"
  must_include_file="$(mktemp)"
  jq -r '.expected.must_include[]' <<<"$case_json" >"$must_include_file"

  [ -f "$response_file" ] || fail "missing response file for $case_id: $response_file"

  case "$expected_verdict" in
    NO_VERDICT)
      if grep -Eq 'SAFE_TO_COMMIT|SAFE_TO_COMMIT_WITH_NOTES|DO_NOT_COMMIT' "$response_file"; then
        rm -f "$must_include_file"
        fail "scenario $scenario expected no verdict token"
      fi
      ;;
    CASE_DEPENDENT) ;;
    *)
      actual_verdict="$(
        grep -Eo 'SAFE_TO_COMMIT_WITH_NOTES|SAFE_TO_COMMIT|DO_NOT_COMMIT' "$response_file" \
          | head -n 1 || true
      )"
      [ "$actual_verdict" = "$expected_verdict" ] || {
        rm -f "$must_include_file"
        fail "scenario $scenario expected verdict $expected_verdict but got ${actual_verdict:-<none>}"
      }
      ;;
  esac

  while IFS= read -r term; do
    [ -n "$term" ] || continue
    if ! grep -Fq "$term" "$response_file"; then
      printf 'missing required term for %s: %s\n' "$scenario" "$term" >&2
      missing=1
    fi
  done <"$must_include_file"
  rm -f "$must_include_file"
  [ "$missing" -eq 0 ] || fail "scenario $scenario failed must_include checks"

  local must_not_include_file forbidden_present=0
  must_not_include_file="$(mktemp)"
  jq -r '.expected.must_not_include[]?' <<<"$case_json" >"$must_not_include_file"

  while IFS= read -r term; do
    [ -n "$term" ] || continue
    if [ "$term" = "**VERDICT:** SAFE_TO_COMMIT" ]; then
      if grep -Eq '^\*\*VERDICT:\*\* SAFE_TO_COMMIT$' "$response_file"; then
        printf 'forbidden term present for %s: %s\n' "$scenario" "$term" >&2
        forbidden_present=1
      fi
    elif grep -Fq "$term" "$response_file"; then
      printf 'forbidden term present for %s: %s\n' "$scenario" "$term" >&2
      forbidden_present=1
    fi
  done <"$must_not_include_file"
  rm -f "$must_not_include_file"
  [ "$forbidden_present" -eq 0 ] || fail "scenario $scenario failed must_not_include check: forbidden value reproduced"

  printf 'PASS %s\n' "$scenario"
}

run_case() {
  local case_json="$1"
  local case_dir="$2"
  local metadata_file response_file workdir case_id scenario locale prompt_file
  local env_exports_tmp runner_status=0

  metadata_file="$case_dir/metadata.json"
  response_file="$responses_dir/$(jq -r '.id' <<<"$case_json").md"
  workdir="$case_dir/workdir"
  case_id="$(jq -r '.id' "$metadata_file")"
  scenario="$(jq -r '.scenario' "$metadata_file")"
  locale="$(jq -r '.locale' "$metadata_file")"
  prompt_file="$(jq -r '.prompt_file' "$metadata_file")"
  env_exports_tmp="$(mktemp)"

  jq -r '
    .env
    | to_entries[]
    | @sh "\(.key)=\(.value)"
  ' "$metadata_file" >"$env_exports_tmp"

  (
    cd "$workdir"
    export PRE_COMMIT_REVIEW_EVAL_CASE_ID="$case_id"
    export PRE_COMMIT_REVIEW_EVAL_SCENARIO="$scenario"
    export PRE_COMMIT_REVIEW_EVAL_LOCALE="$locale"
    export PRE_COMMIT_REVIEW_EVAL_PROMPT_FILE="$prompt_file"
    export PRE_COMMIT_REVIEW_EVAL_METADATA_FILE="$metadata_file"
    export PRE_COMMIT_REVIEW_EVAL_RESPONSE_FILE="$response_file"
    export PRE_COMMIT_REVIEW_EVAL_SKILL_DIR="$repo_root"
    while IFS= read -r assignment; do
      [ -n "$assignment" ] || continue
      eval "export $assignment"
    done <"$env_exports_tmp"
    sh -c "$runner_command"
  ) || runner_status=$?

  rm -f "$env_exports_tmp"

  [ "$runner_status" -eq 0 ] \
    || host_eval_taxonomy_fail 'runner-exit-nonzero' "runner exited non-zero for scenario $scenario: $runner_status"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --runner)
      shift
      [ "$#" -gt 0 ] || fail '--runner requires a value'
      runner_command="$1"
      ;;
    --responses-dir)
      shift
      [ "$#" -gt 0 ] || fail '--responses-dir requires a value'
      responses_dir="$1"
      ;;
    --fixtures-dir)
      shift
      [ "$#" -gt 0 ] || fail '--fixtures-dir requires a value'
      fixtures_dir="$1"
      ;;
    --eval-file)
      shift
      [ "$#" -gt 0 ] || fail '--eval-file requires a value'
      output_eval_file="$1"
      ;;
    --case)
      shift
      [ "$#" -gt 0 ] || fail '--case requires a value'
      case_filter="$1"
      ;;
    --manifest)
      shift
      [ "$#" -gt 0 ] || fail '--manifest requires a value'
      manifest_file="$1"
      ;;
    --keep-fixtures) keep_fixtures='yes' ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

require_command git
require_command jq

[ -f "$output_eval_file" ] || fail "missing output eval cases: $output_eval_file"

cleanup_fixtures='no'
if [ -z "$fixtures_dir" ]; then
  fixtures_dir="$(mktemp -d)"
  cleanup_fixtures='yes'
fi

if [ -z "$responses_dir" ]; then
  responses_dir="$fixtures_dir/responses"
fi
mkdir -p "$fixtures_dir" "$responses_dir"

cases_json="$(jq -c '.cases[]' "$output_eval_file")"
[ -n "$cases_json" ] || fail 'no output-eval cases found'

prepared_cases=0
while IFS= read -r case_json; do
  [ -n "$case_json" ] || continue
  scenario="$(jq -r '.scenario' <<<"$case_json")"
  case_id="$(jq -r '.id' <<<"$case_json")"
  if [ -n "$case_filter" ] && [ "$scenario" != "$case_filter" ]; then
    continue
  fi

  case_dir="$fixtures_dir/$case_id"
  rm -rf "$case_dir"
  mkdir -p "$case_dir"
  prepare_case_fixture "$case_json" "$case_dir"
  prepared_cases=$((prepared_cases + 1))

  if [ -n "$runner_command" ]; then
    run_case "$case_json" "$case_dir"
    response_file="$responses_dir/$case_id.md"
    [ -f "$response_file" ] \
      || host_eval_taxonomy_fail 'response-missing' "missing response file for $case_id: $response_file"
    [ -s "$response_file" ] \
      || host_eval_taxonomy_fail 'response-empty' "response file is empty: $response_file"
  fi

  if [ -f "$responses_dir/$case_id.md" ]; then
    grade_case "$case_json" "$responses_dir/$case_id.md"
  else
    printf 'PREPARED %s\n' "$scenario"
  fi
done <<<"$cases_json"

[ "$prepared_cases" -gt 0 ] || fail 'no cases matched the selection'

if [ -n "$manifest_file" ]; then
  write_manifest "$fixtures_dir" "$manifest_file"
fi

if [ "$cleanup_fixtures" = 'yes' ] && [ "$keep_fixtures" = 'no' ]; then
  rm -rf "$fixtures_dir"
fi

printf 'output eval runner completed\n'
