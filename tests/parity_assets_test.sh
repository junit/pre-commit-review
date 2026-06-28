#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
fixtures_lib="$repo_root/tests/lib/parity_fixtures.sh"
normalizer="$repo_root/tests/lib/normalize_parity_output.py"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'parity assets test failed: %s\n' "$*" >&2
  exit 1
}

[ -f "$fixtures_lib" ] || fail "missing parity fixtures library: $fixtures_lib"
[ -f "$normalizer" ] || fail "missing parity normalizer: $normalizer"

# shellcheck disable=SC1090
source "$fixtures_lib"

fixture_repo="$tmp_dir/parity-fixture"
create_parity_repo_fixture "$fixture_repo"

git -C "$fixture_repo" rev-parse HEAD >/dev/null 2>&1 \
  || fail 'fixture helper should initialize a git repository with a baseline commit'
[ -f "$fixture_repo/.pre-commit-review/risk-paths" ] \
  || fail 'fixture helper should create risk-paths config'
[ -f "$fixture_repo/.pre-commit-review/risk-content" ] \
  || fail 'fixture helper should create risk-content config'
[ -f "$fixture_repo/.pre-commit-review/context-queries" ] \
  || fail 'fixture helper should create context-queries config'
grep -Fxq 'sensitive_configs' "$fixture_repo/.pre-commit-review/risk-paths" \
  || fail 'risk-paths config should seed parity-specific content'
grep -Fxq 'password_secret' "$fixture_repo/.pre-commit-review/risk-content" \
  || fail 'risk-content config should seed parity-specific content'
grep -Fxq 'token' "$fixture_repo/.pre-commit-review/context-queries" \
  || fail 'context-queries config should seed parity-specific content'

sample_input="$tmp_dir/sample.txt"
sample_output="$tmp_dir/normalized.txt"
cat >"$sample_input" <<'EOF'
## Review Groups JSONL
{"group_id":"module-z","value":2}
{"group_id":"module-a","value":1}


## Plain Section
alpha


EOF

python3 "$normalizer" <"$sample_input" >"$sample_output"

python3 - "$sample_output" <<'PY' || fail 'normalizer should sort JSONL payloads and collapse repeated blank lines'
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
if text.count("\n\n\n") != 0:
    raise SystemExit("repeated blank lines were not collapsed")

first = text.index('"group_id": "module-a"')
second = text.index('"group_id": "module-z"')
if first > second:
    raise SystemExit("JSONL payload was not normalized into deterministic order")
PY

printf 'parity assets tests passed\n'
