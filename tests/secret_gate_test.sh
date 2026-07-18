#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
helper="$repo_root/scripts/collect_diff_context.sh"
rust_bin="${PRE_COMMIT_REVIEW_RUST_BIN:-$repo_root/collect-diff-context-cli/target/debug/collect-diff-context-cli}"
tmp_dir="$(mktemp -d)"
cleanup() {
  if [ "${KEEP_SECRET_GATE_FIXTURE:-0}" = '1' ]; then
    printf 'secret gate fixture retained at %s\n' "$tmp_dir" >&2
  else
    rm -rf "$tmp_dir"
  fi
}
trap cleanup EXIT

fail() {
  printf 'secret gate test failed: %s\n' "$*" >&2
  exit 1
}

if [ -z "${PRE_COMMIT_REVIEW_RUST_BIN:-}" ]; then
  cargo build --manifest-path "$repo_root/collect-diff-context-cli/Cargo.toml" >/dev/null
fi
[ -x "$rust_bin" ] || fail "Rust helper binary is unavailable: $rust_bin"
export PRE_COMMIT_REVIEW_SANITIZER_BIN="$rust_bin"

repo="$tmp_dir/repo"
mkdir -p "$repo"
git -C "$repo" init -q
git -C "$repo" config user.email 'test@example.com'
git -C "$repo" config user.name 'Secret Gate Test'

secret="glpat-$(printf '%s%s' '1234567890' 'abcdefghij')"
printf 'old_token = "%s"\n' "$secret" > "$repo/config.py"
git -C "$repo" add config.py
git -C "$repo" commit -qm 'seed old credential fixture'

printf 'old_token = os.environ["OLD_TOKEN"]\n' > "$repo/config.py"
printf 'new_token = "%s" # gitleaks:allow\n' "$secret" > "$repo/new_config.py"
cat > "$repo/.gitleaks.toml" <<'EOF'
title = "Untrusted repository config"
[extend]
useDefault = true
disabledRules = ["gitlab-pat"]
EOF
git -C "$repo" add config.py new_config.py .gitleaks.toml

rust_stdout="$tmp_dir/rust.stdout"
rust_stderr="$tmp_dir/rust.stderr"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$rust_stdout" 2> "$rust_stderr"

if grep -Fq "$secret" "$rust_stdout" "$rust_stderr"; then
  fail 'Rust helper output contained the secret literal'
fi
grep -Fq '[redacted:gitlab-pat]' "$rust_stdout" \
  || {
    grep -E '^(#|secret_scan:|scanner:|status:|redactions:|redaction_mode:|collect_diff_context:)' \
      "$rust_stdout" "$rust_stderr" >&2 || true
    fail 'Rust helper did not redact Gitleaks match coordinates'
  }
grep -Fq '## Secret Scan' "$rust_stdout" \
  || fail 'Rust helper did not emit the secret scan summary'
grep -Fq $'gitlab-pat\t' "$rust_stdout" \
  || fail 'secret scan summary did not include the rule id'
grep -Fq '+new_token = "[redacted:gitlab-pat]" # gitleaks:allow' "$rust_stdout" \
  || fail 'Gitleaks coordinates did not preserve text around the redacted match'

raw_diff="$(git -C "$repo" diff --cached --no-ext-diff --no-textconv --)"
prefix_before_secret="${raw_diff%%"$secret"*}"
boundary_limit="$(( ${#prefix_before_secret} + 10 ))"
boundary_stdout="$tmp_dir/boundary.stdout"
boundary_stderr="$tmp_dir/boundary.stderr"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_MAX_DIFF_BYTES="$boundary_limit" \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$boundary_stdout" 2> "$boundary_stderr"

if grep -Fq 'glpat-1234' "$boundary_stdout" "$boundary_stderr"; then
  fail 'diff truncation released a partial secret before local redaction'
fi
grep -Fq 'status: redacted' "$boundary_stdout" \
  || fail 'full diff was not scanned before the sanitized view was truncated'

legacy_stdout="$tmp_dir/legacy.stdout"
legacy_stderr="$tmp_dir/legacy.stderr"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_HELPER_IMPL=legacy \
    "$helper" --source staged --include-diff always
) > "$legacy_stdout" 2> "$legacy_stderr"

if grep -Fq "$secret" "$legacy_stdout" "$legacy_stderr"; then
  fail 'wrapper final gate released a secret from the legacy helper'
fi
grep -Fq '[redacted:gitlab-pat]' "$legacy_stdout" \
  || fail 'legacy output was not redacted by the final egress sanitizer'
grep -Fq 'status: redacted' "$legacy_stdout" \
  || fail 'legacy egress sanitizer did not report redaction'

failing_rust="$tmp_dir/failing-rust"
cat > "$failing_rust" <<EOF
#!/usr/bin/env bash
printf 'rust stderr contained %s\n' '$secret' >&2
exit 1
EOF
chmod +x "$failing_rust"

fallback_stdout="$tmp_dir/fallback.stdout"
fallback_stderr="$tmp_dir/fallback.stderr"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_RUST_BIN="$failing_rust" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    "$helper" --source staged --include-diff always
) > "$fallback_stdout" 2> "$fallback_stderr"

if grep -Fq "$secret" "$fallback_stdout" "$fallback_stderr"; then
  fail 'Rust failure or legacy fallback released a secret literal'
fi
grep -Fq '[redacted:gitlab-pat]' "$fallback_stdout" \
  || fail 'legacy fallback was not sanitized before release'
if grep -Fq 'secret_scan: blocked' "$fallback_stdout"; then
  fail 'legacy fallback was blocked instead of sanitized'
fi

leaky_stderr_rust="$tmp_dir/leaky-stderr-rust"
cat > "$leaky_stderr_rust" <<EOF
#!/usr/bin/env bash
printf 'safe stdout\n'
printf 'successful stderr contained %s\n' '$secret' >&2
EOF
chmod +x "$leaky_stderr_rust"

stderr_gate_stdout="$tmp_dir/stderr-gate.stdout"
stderr_gate_stderr="$tmp_dir/stderr-gate.stderr"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_RUST_BIN="$leaky_stderr_rust" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --control-plane
) > "$stderr_gate_stdout" 2> "$stderr_gate_stderr"

if grep -Fq "$secret" "$stderr_gate_stdout" "$stderr_gate_stderr"; then
  fail 'wrapper sanitizer released a secret from successful stderr'
fi
grep -Fq '[redacted:gitlab-pat]' "$stderr_gate_stderr" \
  || fail 'successful stderr was not sanitized before release'
grep -Fq 'status: redacted' "$stderr_gate_stderr" \
  || fail 'stderr sanitizer did not report redaction'

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) os_name='darwin' ;;
  linux) os_name='linux' ;;
  msys*|mingw*|cygwin*) os_name='windows' ;;
  *) fail 'unsupported operating system for bundled discovery test' ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch_name='arm64' ;;
  x86_64|amd64) arch_name='amd64' ;;
  *) fail 'unsupported architecture for bundled discovery test' ;;
esac

collector_name="collect_diff_context-${os_name}-${arch_name}"
scanner_name="gitleaks-${os_name}-${arch_name}"
if [ "$os_name" = 'windows' ]; then
  collector_name="${collector_name}.exe"
  scanner_name="${scanner_name}.exe"
fi

scanner_source="$repo_root/scripts/bin/$scanner_name"
[ -x "$scanner_source" ] || fail 'no Gitleaks binary is available for bundled discovery test'

runtime_root="$tmp_dir/runtime"
mkdir -p "$runtime_root/scripts/bin" "$runtime_root/references/security"
cp "$helper" "$runtime_root/scripts/collect_diff_context.sh"
cp "$rust_bin" "$runtime_root/scripts/bin/$collector_name"
cp "$scanner_source" "$runtime_root/scripts/bin/$scanner_name"
cp "$repo_root/scripts/gitleaks.version" \
  "$repo_root/scripts/gitleaks-binaries.sha256" \
  "$runtime_root/scripts/"
cp -R "$repo_root/scripts/lib" "$runtime_root/scripts/"
cp "$repo_root/references/security/gitleaks.toml" \
  "$runtime_root/references/security/gitleaks.toml"
chmod +x "$runtime_root/scripts/collect_diff_context.sh" \
  "$runtime_root/scripts/bin/$collector_name" \
  "$runtime_root/scripts/bin/$scanner_name"

bundled_stdout="$tmp_dir/bundled.stdout"
bundled_stderr="$tmp_dir/bundled.stderr"
(
  cd "$repo"
  env -u PRE_COMMIT_REVIEW_RUST_BIN \
    -u PRE_COMMIT_REVIEW_SANITIZER_BIN \
    -u PRE_COMMIT_REVIEW_GITLEAKS_BIN \
    -u PRE_COMMIT_REVIEW_GITLEAKS_CONFIG \
    PATH=/usr/bin:/bin \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$runtime_root/scripts/collect_diff_context.sh" \
      --source staged --include-diff always
) > "$bundled_stdout" 2> "$bundled_stderr"

if grep -Fq "$secret" "$bundled_stdout" "$bundled_stderr"; then
  fail 'self-contained runtime released a secret literal'
fi
grep -Fq '[redacted:gitlab-pat]' "$bundled_stdout" \
  || fail 'self-contained runtime did not discover its bundled scanner and config'

printf '\n# tampered\n' >> "$runtime_root/scripts/bin/$scanner_name"
tampered_stdout="$tmp_dir/tampered.stdout"
(
  cd "$repo"
  env -u PRE_COMMIT_REVIEW_GITLEAKS_BIN \
    -u PRE_COMMIT_REVIEW_GITLEAKS_CONFIG \
    PATH=/usr/bin:/bin \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$runtime_root/scripts/collect_diff_context.sh" \
      --source staged --include-diff always
) > "$tampered_stdout" 2> "$tmp_dir/tampered.stderr"
grep -Fq "$secret" "$tampered_stdout" \
  || fail 'tampered optional scanner prevented unredacted review from continuing'
grep -Fq 'status: unavailable' "$tampered_stdout" \
  || fail 'tampered optional scanner did not report unavailable redaction'
grep -Fq 'review_continued: yes' "$tampered_stdout" \
  || fail 'tampered optional scanner did not preserve review availability'

rm -f "$runtime_root/scripts/bin/$scanner_name"
path_scanner_dir="$tmp_dir/path-scanner"
mkdir -p "$path_scanner_dir"
cat > "$path_scanner_dir/gitleaks" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'PATH scanner must not run' >&2
exit 99
EOF
chmod +x "$path_scanner_dir/gitleaks"
path_stdout="$tmp_dir/path.stdout"
(
  cd "$repo"
  env -u PRE_COMMIT_REVIEW_GITLEAKS_BIN \
    -u PRE_COMMIT_REVIEW_GITLEAKS_CONFIG \
    -u PRE_COMMIT_REVIEW_SANITIZER_BIN \
    PATH="$path_scanner_dir:/usr/bin:/bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$runtime_root/scripts/collect_diff_context.sh" --source staged --include-diff always
) > "$path_stdout" 2> "$tmp_dir/path.stderr"
grep -Fq 'status: unavailable' "$path_stdout" \
  || fail 'implicit PATH scanner was not rejected'
grep -Fq "$secret" "$path_stdout" \
  || fail 'missing bundled scanner prevented review from continuing'
if grep -Fq 'PATH scanner must not run' "$path_stdout" "$tmp_dir/path.stderr"; then
  fail 'implicit PATH scanner was executed'
fi

missing_stdout="$tmp_dir/missing.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$tmp_dir/does-not-exist" \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$missing_stdout" 2> "$tmp_dir/missing.stderr"

grep -Fq 'status: unavailable' "$missing_stdout" \
  || fail 'missing optional scanner status was not reported'
grep -Fq 'review_continued: yes' "$missing_stdout" \
  || fail 'missing optional scanner did not preserve review availability'
grep -Fq "$secret" "$missing_stdout" \
  || fail 'missing optional scanner withheld review content'

relative_scanner="$repo/relative-gitleaks"
cat > "$relative_scanner" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'relative scanner must not run' >&2
exit 99
EOF
chmod +x "$relative_scanner"
relative_stdout="$tmp_dir/relative.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN='relative-gitleaks' \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$relative_stdout" 2> "$tmp_dir/relative.stderr"
grep -Fq 'status: unavailable' "$relative_stdout" \
  || fail 'relative explicit scanner path did not degrade to unredacted review'
grep -Fq "$secret" "$relative_stdout" \
  || fail 'relative optional scanner path withheld review content'
if grep -Fq 'relative scanner must not run' "$relative_stdout" "$tmp_dir/relative.stderr"; then
  fail 'relative explicit scanner was executed'
fi

wrong_version_scanner="$tmp_dir/wrong-version-gitleaks"
wrong_version_scan_marker="$tmp_dir/wrong-version-scan-ran"
cat > "$wrong_version_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '0.0.0'
  exit 0
fi
touch '$wrong_version_scan_marker'
exit 99
EOF
chmod +x "$wrong_version_scanner"
wrong_version_stdout="$tmp_dir/wrong-version.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$wrong_version_scanner" \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$wrong_version_stdout" 2> "$tmp_dir/wrong-version.stderr"
grep -Fq 'status: unavailable' "$wrong_version_stdout" \
  || fail 'wrong scanner version did not degrade to unredacted review'
grep -Fq "$secret" "$wrong_version_stdout" \
  || fail 'wrong optional scanner version withheld review content'
[ ! -e "$wrong_version_scan_marker" ] \
  || fail 'wrong-version scanner received scan input'

capability_scanner="$tmp_dir/no-stdin-json-gitleaks"
cat > "$capability_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")'
  exit 0
fi
exit 99
EOF
chmod +x "$capability_scanner"
capability_stdout="$tmp_dir/capability.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$capability_scanner" \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$capability_stdout" 2> "$tmp_dir/capability.stderr"
grep -Fq 'status: unavailable' "$capability_stdout" \
  || fail 'scanner without stdin/JSON capability did not degrade cleanly'
grep -Fq "$secret" "$capability_stdout" \
  || fail 'scanner without stdin/JSON capability withheld review content'

invalid_location_scanner="$tmp_dir/invalid-location-gitleaks"
cat > "$invalid_location_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")'
  exit 0
fi
payload="\$(cat)"
if [ -z "\$payload" ]; then
  printf '%s\n' '[]'
  exit 0
fi
printf '%s\n' '[{"RuleID":"test-rule","StartLine":999,"EndLine":999,"StartColumn":1,"EndColumn":7,"Match":"missing"}]'
exit 42
EOF
chmod +x "$invalid_location_scanner"
invalid_location_stdout="$tmp_dir/invalid-location.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$invalid_location_scanner" \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$invalid_location_stdout" 2> "$tmp_dir/invalid-location.stderr"
grep -Fq 'status: redaction-failed' "$invalid_location_stdout" \
  || fail 'an identified finding with an invalid location was reported as scanner unavailable'
grep -Fq 'reason: redaction-location-invalid' "$invalid_location_stdout" \
  || fail 'the redaction implementation failure reason was not preserved'
grep -Fq 'findings_detected: yes' "$invalid_location_stdout" \
  || fail 'the redaction failure did not say that Gitleaks returned a finding'
grep -Fq "$secret" "$invalid_location_stdout" \
  || fail 'a redaction implementation failure withheld review content'

timeout_scanner="$tmp_dir/timeout-gitleaks"
cat > "$timeout_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")'
  exit 0
fi
payload="\$(cat)"
if [ -z "\$payload" ]; then
  printf '%s\n' '[]'
  exit 0
fi
while :; do :; done
EOF
chmod +x "$timeout_scanner"
timeout_stdout="$tmp_dir/timeout.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$timeout_scanner" \
    PRE_COMMIT_REVIEW_GITLEAKS_TIMEOUT_MS=100 \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$timeout_stdout" 2> "$tmp_dir/timeout.stderr"
grep -Fq 'reason: scanner-timeout' "$timeout_stdout" \
  || fail 'timed-out scanner did not report its specific downgrade reason'
grep -Fq 'review_continued: yes' "$timeout_stdout" \
  || fail 'timed-out scanner blocked review output'
grep -Fq "$secret" "$timeout_stdout" \
  || fail 'timed-out optional scanner withheld the original review content'

capability_timeout_scanner="$tmp_dir/capability-timeout-gitleaks"
cat > "$capability_timeout_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")'
  exit 0
fi
cat >/dev/null
while :; do :; done
EOF
chmod +x "$capability_timeout_scanner"
capability_timeout_stdout="$tmp_dir/capability-timeout.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_GITLEAKS_BIN="$capability_timeout_scanner" \
    PRE_COMMIT_REVIEW_GITLEAKS_TIMEOUT_MS=100 \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$capability_timeout_stdout" 2> "$tmp_dir/capability-timeout.stderr"
grep -Fq 'reason: scanner-timeout' "$capability_timeout_stdout" \
  || fail 'capability-check timeout was collapsed into a generic capability failure'
grep -Fq 'review_continued: yes' "$capability_timeout_stdout" \
  || fail 'capability-check timeout blocked review output'

many_hunks_repo="$tmp_dir/many-hunks-repo"
mkdir -p "$many_hunks_repo"
git -C "$many_hunks_repo" init -q
git -C "$many_hunks_repo" config user.email 'test@example.com'
git -C "$many_hunks_repo" config user.name 'Secret Gate Test'
for index in $(seq 1 600); do
  printf 'line %04d original\n' "$index"
done > "$many_hunks_repo/many.txt"
git -C "$many_hunks_repo" add many.txt
git -C "$many_hunks_repo" commit -qm 'many hunk baseline'
for index in $(seq 1 600); do
  if [ $((index % 20)) -eq 0 ]; then
    printf 'line %04d changed\n' "$index"
  else
    printf 'line %04d original\n' "$index"
  fi
done > "$many_hunks_repo/many.txt"
git -C "$many_hunks_repo" add many.txt

counting_scanner="$tmp_dir/counting-gitleaks"
cat > "$counting_scanner" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1:-}" = 'version' ]; then
  printf '%s\n' '$(tr -d '[:space:]' < "$repo_root/scripts/gitleaks.version")'
  exit 0
fi
cat >/dev/null
printf '%s\n' scan >> "\$FAKE_GITLEAKS_LOG"
printf '%s\n' '[]'
EOF
chmod +x "$counting_scanner"
scan_log="$tmp_dir/counting-scans.log"
(
  cd "$many_hunks_repo"
  FAKE_GITLEAKS_LOG="$scan_log" \
    PRE_COMMIT_REVIEW_HELPER_PATH="$helper" \
    PRE_COMMIT_REVIEW_GITLEAKS_BIN="$counting_scanner" \
    PRE_COMMIT_REVIEW_GITLEAKS_CONFIG="$repo_root/references/security/gitleaks.toml" \
    PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES=80 \
    PRE_COMMIT_REVIEW_GROUP_HARD_BYTES=100 \
    "$rust_bin" --source staged --group module-many.txt
) > "$tmp_dir/many-hunks.stdout" 2> "$tmp_dir/many-hunks.stderr"
scan_calls="$(wc -l < "$scan_log" | tr -d '[:space:]')"
[ "$scan_calls" -le 3 ] \
  || fail "split preview spawned the scanner once per hunk: $scan_calls calls"

disabled_stdout="$tmp_dir/disabled.stdout"
(
  cd "$repo"
  PRE_COMMIT_REVIEW_SECRET_SCAN=off \
    PRE_COMMIT_REVIEW_RUST_BIN="$rust_bin" \
    PRE_COMMIT_REVIEW_HELPER_IMPL=rust \
    PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
    "$helper" --source staged --include-diff always
) > "$disabled_stdout" 2> "$tmp_dir/disabled.stderr"
grep -Fq "$secret" "$disabled_stdout" \
  || fail 'disabled optional scanner withheld review content'
grep -Fq 'status: disabled' "$disabled_stdout" \
  || fail 'disabled optional scanner status was not reported'
grep -Fq 'review_continued: yes' "$disabled_stdout" \
  || fail 'disabled optional scanner did not preserve review availability'

printf 'secret gate tests passed\n'
