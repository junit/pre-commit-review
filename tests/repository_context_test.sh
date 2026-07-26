#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
resolver="$repo_root/scripts/lib/repository_context_cli.sh"
wrapper="$repo_root/scripts/collect_impact_context.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'repository context test failed: %s\n' "$*" >&2
  exit 1
}

[ -r "$resolver" ] || fail 'resolver is missing'
[ -x "$wrapper" ] || fail 'wrapper is missing or not executable'

fake_bin="$tmp_dir/repository-context-cli"
cat >"$fake_bin" <<'EOF_FAKE'
#!/usr/bin/env bash
printf '%s' '{"schema_version":1,"kind":"impact_context","scope":{"fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"staged","candidate_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"mode":"fast","status":"unavailable","providers":[],"units":[],"changed_symbols":[],"impact_edges":[],"domain_summaries":[],"coverage":{"total_candidate_files":0,"changed_candidate_files":0,"syntax_eligible_files":0,"parsed_files":0,"clean_parse_files":0,"recovered_parse_files":0,"degraded_parse_files":0,"unsupported_files":0,"resource_limited_files":0,"unavailable_files":0,"cache_hits":0,"cache_misses":0,"cache_stale":0,"cache_corrupt":0,"requested_graph_depth":0,"reached_graph_depth":0,"graph_index_completeness":"unavailable","graph_query_completeness":"unavailable","output_truncated":false},"limitations":[],"metrics":{"elapsed_ms":0,"candidate_input_files":0,"candidate_input_bytes":0,"nodes_visited":0,"max_nesting_depth":0,"facts_emitted":0,"edges_emitted":0,"summaries_emitted":0,"output_bytes":0}}'
EOF_FAKE
chmod +x "$fake_bin"

output="$tmp_dir/output"
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" collect --source staged \
    --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --mode fast >"$output"
grep -Fq '## Impact Context JSON' "$output" || fail 'wrapper omitted JSON section'
grep -Fq '"kind":"impact_context"' "$output" || fail 'wrapper omitted collector JSON'

if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN='relative-bin' \
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" collect --source staged \
    --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --mode fast \
    >"$tmp_dir/relative.out" 2>"$tmp_dir/relative.err"; then
  fail 'relative override was accepted'
fi

isolated_root="$tmp_dir/isolated"
isolated_scripts="$isolated_root/scripts"
mkdir -p "$isolated_scripts/lib" \
  "$isolated_scripts/bin" \
  "$isolated_root/collect-diff-context-cli/target/release"
cp "$resolver" "$isolated_scripts/lib/repository_context_cli.sh"
cp "$wrapper" "$isolated_scripts/collect_impact_context.sh"
chmod +x "$isolated_scripts/collect_impact_context.sh" \
  "$isolated_scripts/lib/repository_context_cli.sh"

local_bin="$isolated_root/collect-diff-context-cli/target/release/repository-context-cli"
cat >"$local_bin" <<'EOF_LOCAL'
#!/usr/bin/env bash
printf '%s' '{"resolver":"local-release"}'
EOF_LOCAL
chmod +x "$local_bin"

os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch_name="$(uname -m)"
case "$os_name" in
  darwin) os_name='darwin' ;;
  msys*|mingw*|cygwin*) os_name='windows' ;;
  *) os_name='linux' ;;
esac
case "$arch_name" in
  x86_64|amd64) arch_name='amd64' ;;
  arm64|aarch64) arch_name='arm64' ;;
  *) fail 'unsupported test architecture' ;;
esac
packaged_name="repository_context-${os_name}-${arch_name}"
[ "$os_name" = 'windows' ] && packaged_name="${packaged_name}.exe"
cat >"$isolated_scripts/bin/$packaged_name" <<'EOF_PACKAGED'
#!/usr/bin/env bash
printf '%s' '{"resolver":"packaged"}'
EOF_PACKAGED
chmod +x "$isolated_scripts/bin/$packaged_name"

PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$isolated_scripts/collect_impact_context.sh" --source staged \
    --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --mode fast \
    >"$tmp_dir/local-order.out"
grep -Fq '"resolver":"local-release"' "$tmp_dir/local-order.out" \
  || fail 'local release binary did not precede packaged binary'

rm -f "$local_bin" "$isolated_scripts/bin/$packaged_name"
legacy_sentinel="$tmp_dir/legacy-invoked"
cat >"$isolated_scripts/collect_diff_context.legacy.sh" <<EOF_LEGACY
#!/usr/bin/env bash
touch '$legacy_sentinel'
exit 99
EOF_LEGACY
chmod +x "$isolated_scripts/collect_diff_context.legacy.sh"
PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$isolated_scripts/collect_impact_context.sh" --source staged \
    --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --mode fast \
    >"$tmp_dir/unavailable.out"
grep -Fq '"status":"unavailable"' "$tmp_dir/unavailable.out" \
  || fail 'missing binary did not produce unavailable context'
[ ! -e "$legacy_sentinel" ] || fail 'missing binary invoked legacy helper'

printf 'repository context tests passed\n'
