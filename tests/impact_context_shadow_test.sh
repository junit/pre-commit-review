#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
runner="$repo_root/evals/run_impact_context_shadow.sh"
rust_helper="$repo_root/collect-diff-context-cli/target/release/collect-diff-context-cli"
context_bin="$repo_root/collect-diff-context-cli/target/release/repository-context-cli"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'impact context shadow test failed: %s\n' "$*" >&2
  exit 1
}

[ -x "$runner" ] || fail 'shadow runner is missing or not executable'
[ -x "$rust_helper" ] || fail 'release collect-diff-context-cli is missing'
[ -x "$context_bin" ] || fail 'release repository-context-cli is missing'
command -v jq >/dev/null 2>&1 || fail 'jq is required'

fixture="$tmp_dir/repo"
mkdir -p "$fixture/src" "$fixture/.pre-commit-review"
git -C "$fixture" init -q
git -C "$fixture" config user.email review@example.test
git -C "$fixture" config user.name Review
printf '[package]\nname = "fixture"\nversion = "0.1.0"\n' >"$fixture/Cargo.toml"
printf 'pub fn base() {}\n' >"$fixture/src/lib.rs"
git -C "$fixture" add Cargo.toml src/lib.rs
git -C "$fixture" commit -qm base
printf '\n[dependencies]\nserde = "1"\n' >>"$fixture/Cargo.toml"
printf 'pub fn changed() { println!("changed"); }\n' >"$fixture/src/lib.rs"
printf 'changed\n' >"$fixture/.pre-commit-review/context-queries"
git -C "$fixture" add Cargo.toml src/lib.rs .pre-commit-review/context-queries

metrics="$tmp_dir/metrics.json"
stdout_file="$tmp_dir/stdout"
(
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
  PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$context_bin" \
    "$runner" --source staged --output "$metrics"
) >"$stdout_file"

grep -Fq '## Dependency Summary' "$stdout_file" \
  || fail 'legacy Rust report was not preserved on stdout'
if grep -Fq '## Impact Context JSON' "$stdout_file" \
  || grep -Fq '"kind":"impact_context"' "$stdout_file"; then
  fail 'new impact context leaked into production stdout'
fi

jq -e '
  .schema_version == 1 and
  .kind == "impact_context_shadow_metrics" and
  (.scope_fingerprint | test("^[0-9a-f]{40}([0-9a-f]{24})?$")) and
  .legacy_dependency_rows >= 1 and
  .legacy_semantic_query_matches >= 1 and
  .new_changed_symbols >= 1 and
  .new_impact_edges >= 1 and
  .new_domain_summaries >= 1 and
  (.new_status == "completed" or .new_status == "partial") and
  (.new_limitation_codes | type == "array") and
  (.elapsed_ms | type == "number" and . >= 0)
' "$metrics" >/dev/null || fail 'shadow metrics are invalid'

if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
  PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$context_bin" \
    "$runner" --source staged
) >"$tmp_dir/no-output.stdout" 2>"$tmp_dir/no-output.stderr"; then
  fail 'shadow runner accepted a missing --output'
fi

if (
  cd "$fixture"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
  PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$context_bin" \
    "$runner" --source staged --output relative.json
) >"$tmp_dir/relative.stdout" 2>"$tmp_dir/relative.stderr"; then
  fail 'shadow runner accepted a relative output path'
fi
[ ! -e "$fixture/relative.json" ] || fail 'shadow runner wrote metrics inside the repository'

printf 'impact context shadow tests passed\n'
