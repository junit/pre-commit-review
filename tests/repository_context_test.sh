#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
resolver="$repo_root/scripts/lib/repository_context_cli.sh"
wrapper="$repo_root/scripts/collect_impact_context.sh"
helper="$repo_root/scripts/collect_diff_context.sh"
rust_helper="$repo_root/collect-diff-context-cli/target/release/collect-diff-context-cli"
context_bin="$repo_root/collect-diff-context-cli/target/release/repository-context-cli"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'repository context test failed: %s\n' "$*" >&2
  exit 1
}

[ -r "$resolver" ] || fail 'resolver is missing'
[ -x "$wrapper" ] || fail 'wrapper is missing or not executable'
[ -x "$rust_helper" ] || fail 'release collect-diff-context-cli is missing'
[ -x "$context_bin" ] || fail 'release repository-context-cli is missing'

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
PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$isolated_scripts/collect_impact_context.sh" --source staged \
    --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --mode deep \
    >"$tmp_dir/unavailable-deep.out"
grep -Fq '"mode":"deep"' "$tmp_dir/unavailable-deep.out" \
  || fail 'missing binary unavailable context did not preserve deep mode'

security_repo="$tmp_dir/security-repo"
mkdir -p "$security_repo/.pre-commit-review" "$security_repo/grammars" "$security_repo/scripts"
git -C "$security_repo" init -q
git -C "$security_repo" config user.email review@example.test
git -C "$security_repo" config user.name Review
printf 'base\n' >"$security_repo/README.md"
printf '(function_item) @repository_query\n' >"$security_repo/.pre-commit-review/tree-sitter-rust.scm"
printf '{"grammars":["repository"]}\n' >"$security_repo/tree-sitter.json"
printf 'plugin\0payload' >"$security_repo/grammars/libtree-sitter-rust.so"
repo_hook_sentinel="$tmp_dir/repository-hook-invoked"
cat >"$security_repo/scripts/repository-context-hook.sh" <<'EOF_REPO_HOOK'
#!/usr/bin/env bash
touch "$PCR_REPOSITORY_HOOK_SENTINEL"
exit 97
EOF_REPO_HOOK
chmod +x "$security_repo/scripts/repository-context-hook.sh"
git -C "$security_repo" add README.md .pre-commit-review/tree-sitter-rust.scm \
  tree-sitter.json grammars/libtree-sitter-rust.so scripts/repository-context-hook.sh
git -C "$security_repo" commit -qm base
printf 'pub fn changed() {}\n' >"$security_repo/src.rs"
git -C "$security_repo" add src.rs

security_control="$tmp_dir/security-control.out"
(
  cd "$security_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
    "$helper" --source staged --control-plane
) >"$security_control"
security_fingerprint="$(python3 - "$security_control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"

command_dir="$tmp_dir/command-shims"
cache_dir="$tmp_dir/cache"
exec_log="$tmp_dir/executed-commands.log"
forbidden_log="$tmp_dir/forbidden-commands.log"
mkdir -p "$command_dir" "$cache_dir"
: >"$exec_log"
: >"$forbidden_log"
real_git="$(command -v git)"
security_status=0
cat >"$command_dir/git" <<'EOF_GIT_SHIM'
#!/usr/bin/env bash
printf '%s\n' git >>"$PCR_EXEC_LOG"
exec "$PCR_REAL_GIT" "$@"
EOF_GIT_SHIM
chmod +x "$command_dir/git"
for forbidden_command in \
  cargo rustc rust-analyzer curl wget nc \
  npm npx pnpm yarn bun pip pip3 poetry uv \
  go gradle mvn; do
  cat >"$command_dir/$forbidden_command" <<'EOF_FORBIDDEN_SHIM'
#!/usr/bin/env bash
printf '%s\n' "${0##*/}" >>"$PCR_FORBIDDEN_LOG"
exit 97
EOF_FORBIDDEN_SHIM
  chmod +x "$command_dir/$forbidden_command"
done

(
  cd "$security_repo"
  PATH="$command_dir:/usr/bin:/bin:/usr/sbin:/sbin" \
  PCR_EXEC_LOG="$exec_log" \
  PCR_FORBIDDEN_LOG="$forbidden_log" \
  PCR_REAL_GIT="$real_git" \
  PCR_REPOSITORY_HOOK_SENTINEL="$repo_hook_sentinel" \
  PRE_COMMIT_REVIEW_CACHE_DIR="$cache_dir" \
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  HTTP_PROXY='http://127.0.0.1:9' \
  HTTPS_PROXY='http://127.0.0.1:9' \
  ALL_PROXY='socks5://127.0.0.1:9' \
  NO_PROXY='' \
    "$context_bin" collect --source staged \
      --expect-scope "$security_fingerprint" --mode fast
) >"$tmp_dir/security-context.json" || security_status=$?
case "$security_status" in
  0|3) ;;
  *) fail "security fixture exited with status $security_status" ;;
esac
grep -Fq '"kind":"impact_context"' "$tmp_dir/security-context.json" \
  || fail 'security fixture did not emit impact context'
[ ! -s "$forbidden_log" ] || fail 'fast collection invoked a forbidden executable'
[ ! -e "$repo_hook_sentinel" ] || fail 'fast collection invoked a repository-owned script'
if grep -Fvx 'git' "$exec_log" >/dev/null; then
  fail 'fast collection invoked an external process other than Git'
fi
if find "$cache_dir" -mindepth 1 -print -quit | grep -q .; then
  fail 'fast collection wrote persistent cache state'
fi

malformed_repo="$tmp_dir/malformed-git-repo"
mkdir -p "$malformed_repo"
git -C "$malformed_repo" init -q
git -C "$malformed_repo" config user.email review@example.test
git -C "$malformed_repo" config user.name Review
printf 'base\n' >"$malformed_repo/file.txt"
git -C "$malformed_repo" add file.txt
git -C "$malformed_repo" commit -qm base
printf 'changed\n' >>"$malformed_repo/file.txt"
git -C "$malformed_repo" add file.txt
malformed_control="$tmp_dir/malformed-control.out"
(
  cd "$malformed_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
    "$helper" --source staged --control-plane
) >"$malformed_control"
malformed_fingerprint="$(python3 - "$malformed_control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
printf 'malformed-index' >"$malformed_repo/.git/index"
if (
  cd "$malformed_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$context_bin" collect --source staged \
      --expect-scope "$malformed_fingerprint" --mode fast
) >"$tmp_dir/malformed.out" 2>"$tmp_dir/malformed.err"; then
  fail 'malformed Git metadata was accepted'
fi
[ ! -s "$tmp_dir/malformed.out" ] || fail 'malformed Git metadata released context facts'
grep -Fq 'repository-context-cli:' "$tmp_dir/malformed.err" \
  || fail 'malformed Git metadata lacked a stable CLI error'

printf 'repository context tests passed\n'
