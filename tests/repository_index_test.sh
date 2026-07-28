#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
wrapper="$repo_root/scripts/index_repository_context.sh"
resolver="$repo_root/scripts/lib/repository_context_cli.sh"
context_bin="$repo_root/collect-diff-context-cli/target/release/repository-context-cli"
control_helper="$repo_root/scripts/collect_diff_context.sh"
rust_helper="$repo_root/collect-diff-context-cli/target/release/collect-diff-context-cli"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'repository index test failed: %s\n' "$*" >&2
  exit 1
}

[ -x "$wrapper" ] || fail 'index wrapper is missing or not executable'
[ -r "$resolver" ] || fail 'repository context resolver is missing'
[ -x "$context_bin" ] || fail 'release repository-context-cli is missing'
[ -x "$rust_helper" ] || fail 'release control helper is missing'

scope='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
generation='cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
repository_id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
fake_bin="$tmp_dir/repository-context-cli"
fake_log="$tmp_dir/fake.log"
cat >"$fake_bin" <<'EOF_FAKE'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$PCR_FAKE_LOG"
action='build'
scope_value='null'
generation_value='null'
case " $* " in
  *' index build '*)
    action='build'
    scope_arg=''
    previous=''
    for argument in "$@"; do
      if [ "$previous" = '--expect-scope' ]; then scope_arg="$argument"; fi
      previous="$argument"
    done
    scope_value="\"$scope_arg\""
    generation_value='"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
    ;;
  *' index doctor '*) action='doctor' ;;
  *' index inspect '*)
    action='inspect'
    generation_value='"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"'
    ;;
  *' index clean '*) action='clean' ;;
esac
printf '%s' "{\"schema_version\":1,\"kind\":\"repository_index_report\",\"action\":\"$action\",\"status\":\"completed\",\"scope_fingerprint\":$scope_value,\"repository_id\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"generation_key\":$generation_value,\"metrics\":{\"elapsed_ms\":0,\"manifest_files\":0,\"manifest_bytes\":0,\"file_fact_hits\":0,\"file_fact_misses\":0,\"file_fact_writes\":0,\"parsed_files\":0,\"parsed_bytes\":0,\"symbols\":0,\"edges\":0,\"query_rows\":0,\"generation_bytes\":0,\"output_bytes\":0},\"limitations\":[]}"
EOF_FAKE
chmod +x "$fake_bin"

for arguments in \
  'index build --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
  'index build --source staged' \
  'index build --source invalid --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
  'index build --source staged --expect-scope invalid'; do
  if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
    PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$wrapper" $arguments >"$tmp_dir/invalid.out" 2>"$tmp_dir/invalid.err"; then
    fail "invalid wrapper arguments were accepted: $arguments"
  fi
done

if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN='relative-bin' \
  PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index doctor >"$tmp_dir/relative.out" 2>"$tmp_dir/relative.err"; then
  fail 'relative repository context override was accepted'
fi

if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
  PRE_COMMIT_REVIEW_CACHE_DIR='relative-cache' \
  PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index doctor >"$tmp_dir/relative-cache-env.out" \
    2>"$tmp_dir/relative-cache-env.err"; then
  fail 'relative repository cache environment override was accepted'
fi

if PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
  PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index doctor --cache-dir relative-cache \
    >"$tmp_dir/relative-cache-arg.out" 2>"$tmp_dir/relative-cache-arg.err"; then
  fail 'relative repository cache argument was accepted'
fi

expected="$tmp_dir/expected.json"
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index build --source staged --expect-scope "$scope" \
    --max-symbols 5 >"$tmp_dir/build.json"
grep -Fqx 'index build --source staged --expect-scope aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --max-symbols 5' "$fake_log" \
  || fail 'index build arguments were not forwarded exactly'
printf '%s' "{\"schema_version\":1,\"kind\":\"repository_index_report\",\"action\":\"build\",\"status\":\"completed\",\"scope_fingerprint\":\"$scope\",\"repository_id\":\"$repository_id\",\"generation_key\":\"$generation\",\"metrics\":{\"elapsed_ms\":0,\"manifest_files\":0,\"manifest_bytes\":0,\"file_fact_hits\":0,\"file_fact_misses\":0,\"file_fact_writes\":0,\"parsed_files\":0,\"parsed_bytes\":0,\"symbols\":0,\"edges\":0,\"query_rows\":0,\"generation_bytes\":0,\"output_bytes\":0},\"limitations\":[]}" >"$expected"
cmp -s "$expected" "$tmp_dir/build.json" || fail 'index build compact JSON was rewritten'

PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index doctor --generation "$generation" >"$tmp_dir/doctor.json"
grep -Fq '"action":"doctor"' "$tmp_dir/doctor.json" || fail 'doctor report was not forwarded'

PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index inspect --generation "$generation" --path src/lib.rs \
    --max-rows 1 >"$tmp_dir/inspect.json"
grep -Fq -- '--max-rows 1' "$fake_log" || fail 'inspect row bound was not forwarded'

sentinel="$tmp_dir/clean-sentinel"
printf 'keep\n' >"$sentinel"
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index clean >"$tmp_dir/clean-dry-run.json"
[ -f "$sentinel" ] || fail 'clean without execute mutated state'
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$fake_bin" \
PCR_FAKE_LOG="$fake_log" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
  "$wrapper" index clean --execute --max-bytes 1 --retain-generations 0 \
    >"$tmp_dir/clean-execute.json"
grep -Fq 'index clean --execute --max-bytes 1 --retain-generations 0' "$fake_log" \
  || fail 'explicit clean execute arguments were not forwarded'

stderr_secret="glpat-$(printf '%s%s' '1234567890' 'abcdefghij')"
stderr_private_path="$tmp_dir/private/index.sqlite"
stderr_sanitizer="$tmp_dir/stderr-sanitizer"
stderr_sanitizer_log="$tmp_dir/stderr-sanitizer.log"
cat >"$stderr_sanitizer" <<'EOF_SANITIZER'
#!/usr/bin/env bash
sed -e "s|$PCR_STDERR_SECRET|[redacted:index-secret]|g" \
  -e "s|$PCR_STDERR_PRIVATE_PATH|[redacted:index-path]|g"
printf '%s\n' "$PRE_COMMIT_REVIEW_SANITIZE_STREAM" >>"$PCR_SANITIZER_LOG"
cat >"$PRE_COMMIT_REVIEW_SANITIZE_REPORT" <<'EOF_REPORT'
protocol: pcr-sanitizer-v1
status: redacted
EOF_REPORT
EOF_SANITIZER
chmod +x "$stderr_sanitizer"
stderr_leaky_bin="$tmp_dir/stderr-leaky-repository-context-cli"
cat >"$stderr_leaky_bin" <<'EOF_STDERR_LEAK'
#!/usr/bin/env bash
printf '%s' '{"schema_version":1,"kind":"repository_index_report","action":"doctor","status":"partial","scope_fingerprint":null,"repository_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","generation_key":null,"metrics":{"elapsed_ms":0,"manifest_files":0,"manifest_bytes":0,"file_fact_hits":0,"file_fact_misses":0,"file_fact_writes":0,"parsed_files":0,"parsed_bytes":0,"symbols":0,"edges":0,"query_rows":0,"generation_bytes":0,"output_bytes":0},"limitations":[]}'
printf 'repository index failed at %s with token %s\n' \
  "$PCR_STDERR_PRIVATE_PATH" "$PCR_STDERR_SECRET" >&2
exit "${PCR_STDERR_EXIT:-3}"
EOF_STDERR_LEAK
chmod +x "$stderr_leaky_bin"
stderr_exit=0
PCR_STDERR_SECRET="$stderr_secret" \
PCR_STDERR_PRIVATE_PATH="$stderr_private_path" \
PCR_SANITIZER_LOG="$stderr_sanitizer_log" \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$stderr_leaky_bin" \
PRE_COMMIT_REVIEW_SANITIZER_BIN="$stderr_sanitizer" \
  "$wrapper" index doctor >"$tmp_dir/stderr-sanitized.out" \
    2>"$tmp_dir/stderr-sanitized.err" || stderr_exit=$?
[ "$stderr_exit" -eq 3 ] || fail 'index wrapper did not preserve partial exit status'
if grep -Fq "$stderr_secret" "$tmp_dir/stderr-sanitized.out" "$tmp_dir/stderr-sanitized.err"; then
  fail 'index wrapper released a secret from repository context stderr'
fi
if grep -Fq "$stderr_private_path" "$tmp_dir/stderr-sanitized.out" "$tmp_dir/stderr-sanitized.err"; then
  fail 'index wrapper released a private path from repository context stderr'
fi
grep -Fq '[redacted:index-secret]' "$tmp_dir/stderr-sanitized.err" \
  || fail 'index wrapper did not publish sanitized stderr secret output'
grep -Fq '[redacted:index-path]' "$tmp_dir/stderr-sanitized.err" \
  || fail 'index wrapper did not publish sanitized stderr path output'
grep -Fqx 'repository-index-stderr' "$stderr_sanitizer_log" \
  || fail 'index wrapper did not invoke the stderr sanitizer stream'
stderr_failure_exit=0
PCR_STDERR_SECRET="$stderr_secret" \
PCR_STDERR_PRIVATE_PATH="$stderr_private_path" \
PCR_STDERR_EXIT=1 \
PCR_SANITIZER_LOG="$stderr_sanitizer_log" \
PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$stderr_leaky_bin" \
PRE_COMMIT_REVIEW_SANITIZER_BIN="$stderr_sanitizer" \
  "$wrapper" index doctor >"$tmp_dir/stderr-failure.out" \
    2>"$tmp_dir/stderr-failure.err" || stderr_failure_exit=$?
[ "$stderr_failure_exit" -eq 0 ] || fail 'index wrapper did not degrade operation failure safely'
grep -Fq '"status":"unavailable"' "$tmp_dir/stderr-failure.out" \
  || fail 'index wrapper did not emit unavailable report after operation failure'
if grep -Fq "$stderr_secret" "$tmp_dir/stderr-failure.out" "$tmp_dir/stderr-failure.err" \
  || grep -Fq "$stderr_private_path" "$tmp_dir/stderr-failure.out" "$tmp_dir/stderr-failure.err"; then
  fail 'index wrapper released raw stderr before operation failure degradation'
fi
grep -Fq '[redacted:index-secret]' "$tmp_dir/stderr-failure.err" \
  || fail 'index wrapper did not sanitize ordinary failure stderr'

isolated_root="$tmp_dir/isolated"
mkdir -p "$isolated_root/scripts/lib" "$isolated_root/scripts/bin"
cp "$resolver" "$isolated_root/scripts/lib/repository_context_cli.sh"
cp "$wrapper" "$isolated_root/scripts/index_repository_context.sh"
chmod +x "$isolated_root/scripts/index_repository_context.sh"
missing_cache="$tmp_dir/missing-cache"
mkdir -p "$missing_cache"
(
  cd "$repo_root"
  PRE_COMMIT_REVIEW_CACHE_DIR="$missing_cache" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$isolated_root/scripts/index_repository_context.sh" index build \
      --source staged --expect-scope "$scope"
) >"$tmp_dir/unavailable.json"
grep -Fq '"status":"unavailable"' "$tmp_dir/unavailable.json" \
  || fail 'missing binary did not emit an unavailable index report'
if find "$missing_cache" -mindepth 1 -print -quit | grep -q .; then
  fail 'missing binary wrote cache state'
fi

stage_repo="$tmp_dir/stage-repo"
mkdir -p "$stage_repo/src"
git -C "$stage_repo" init -q
git -C "$stage_repo" config user.email review@example.test
git -C "$stage_repo" config user.name Review
printf '[package]\nname="fixture"\nversion="0.1.0"\nedition="2021"\n' >"$stage_repo/Cargo.toml"
printf 'pub fn base() {}\n' >"$stage_repo/src/lib.rs"
git -C "$stage_repo" add Cargo.toml src/lib.rs
git -C "$stage_repo" commit -qm base
printf 'pub fn staged_only() {}\n' >"$stage_repo/src/lib.rs"
git -C "$stage_repo" add src/lib.rs
printf 'pub fn working_only() {}\n' >"$stage_repo/src/lib.rs"
control="$tmp_dir/control.out"
(
  cd "$stage_repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off PRE_COMMIT_REVIEW_RUST_BIN="$rust_helper" \
    "$control_helper" --source staged --control-plane
) >"$control"
stage_scope="$(python3 - "$control" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
stage_cache="$tmp_dir/stage-cache"
mkdir -p "$stage_cache"
(
  cd "$stage_repo"
  PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_BIN="$context_bin" \
  PRE_COMMIT_REVIEW_CACHE_DIR="$stage_cache" PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    "$wrapper" index build --source staged --expect-scope "$stage_scope"
) >"$tmp_dir/stage-build.json"
python3 - "$tmp_dir/stage-build.json" "$stage_cache" <<'PY'
import json
import pathlib
import sqlite3
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
database = pathlib.Path(sys.argv[2]) / 'v2' / 'repos' / report['repository_id'] / 'graphs' / f"{report['generation_key']}.sqlite"
connection = sqlite3.connect(f"file:{database}?mode=ro&immutable=1", uri=True)
rows = '\n'.join(row[0] for row in connection.execute('SELECT canonical_json FROM symbols ORDER BY symbol_id'))
connection.close()
if 'staged_only' not in rows or 'working_only' in rows:
    raise SystemExit('staged index did not use stage-zero bytes')
PY

printf 'repository index tests passed\n'
