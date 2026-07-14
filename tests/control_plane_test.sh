#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
helper="$repo_root/scripts/collect_diff_context.sh"
validator="$repo_root/scripts/validate_schemas.py"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'control plane test failed: %s\n' "$*" >&2
  exit 1
}

fixture="$tmp_dir/repo"
mkdir -p "$fixture/src/auth" "$fixture/docs"
git -C "$fixture" init -q
git -C "$fixture" config user.email a@example.com
git -C "$fixture" config user.name A
printf 'base\n' >"$fixture/README.md"
printf '*.dat diff=review-fixture\n' >"$fixture/.gitattributes"
printf 'old binary-ish content\n' >"$fixture/sample.dat"
printf '#!/bin/sh\nprintf "TEXTCONV_MARKER\\n"\ncat -- "$1"\n' >"$tmp_dir/textconv.sh"
chmod +x "$tmp_dir/textconv.sh"
git -C "$fixture" config diff.review-fixture.textconv "$tmp_dir/textconv.sh"
git -C "$fixture" add README.md .gitattributes sample.dat
git -C "$fixture" commit -q -m init
printf 'export const allowed = (token) => token === "ok";\n' >"$fixture/src/auth/session.ts"
printf 'review notes\n' >"$fixture/docs/review.md"
printf 'new binary-ish content\n' >"$fixture/sample.dat"
weird_path=$'weird file\tname.txt'
printf 'special path content\n' >"$fixture/$weird_path"
git -C "$fixture" add src/auth/session.ts docs/review.md sample.dat "$weird_path"

for impl in rust legacy; do
  output="$tmp_dir/$impl.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --control-plane
  ) >"$output"

  python3 "$validator" --control-plane-output "$output" >/dev/null \
    || fail "$impl output did not validate"
  [ "$(wc -c <"$output" | tr -d ' ')" -lt 60000 ] \
    || fail "$impl control plane exceeded 60KB"
  grep -Fq '"authoritative":true' "$output" \
    || fail "$impl control plane was not authoritative"
  if grep -Fq '## Diff' "$output"; then
    fail "$impl control plane must not emit raw diff"
  fi
done

python3 - "$tmp_dir/rust.out" "$tmp_dir/legacy.out" <<'PY' || fail 'Rust and legacy semantic control planes differ'
import json
import sys

def payload(path):
    lines = open(path, encoding='utf-8').read().splitlines()
    return json.loads(lines[lines.index('## Review Control Plane JSON') + 1])

rust = payload(sys.argv[1])
legacy = payload(sys.argv[2])
for key in ('source', 'head', 'base', 'selected_ref', 'scope_fingerprint', 'counts',
            'unit_tuple_fields', 'units', 'group_tuple_fields', 'groups',
            'work_order_tuple_fields', 'work_order', 'coverage_contract'):
    if rust[key] != legacy[key]:
        raise SystemExit(f'{key} mismatch')
PY

python3 - "$tmp_dir/rust.out" "$tmp_dir/tampered.out" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
marker = lines.index('## Review Control Plane JSON')
payload = json.loads(lines[marker + 1])
payload['counts']['diff_bytes'] += 1
lines[marker + 1] = json.dumps(payload, separators=(',', ':'))
pathlib.Path(sys.argv[2]).write_text('\n'.join(lines) + '\n', encoding='utf-8')
PY
if python3 "$validator" --control-plane-output "$tmp_dir/tampered.out" >/dev/null 2>&1; then
  fail 'semantic validator accepted a tampered control-plane count'
fi
python3 - "$tmp_dir/rust.out" "$tmp_dir/tampered-work-order.out" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8').splitlines()
marker = lines.index('## Review Control Plane JSON')
payload = json.loads(lines[marker + 1])
payload['work_order'][0][0] = 99
payload['work_order'] = list(reversed(payload['work_order']))
lines[marker + 1] = json.dumps(payload, separators=(',', ':'))
pathlib.Path(sys.argv[2]).write_text('\n'.join(lines) + '\n', encoding='utf-8')
PY
if python3 "$validator" --control-plane-output "$tmp_dir/tampered-work-order.out" >/dev/null 2>&1; then
  fail 'semantic validator accepted tampered work-order priority or ordering'
fi

fingerprint="$(python3 - "$tmp_dir/rust.out" <<'PY'
import json, sys
lines = open(sys.argv[1], encoding='utf-8').read().splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['scope_fingerprint'])
PY
)"
group_id="$(python3 - "$tmp_dir/rust.out" <<'PY'
import json, sys
lines = open(sys.argv[1], encoding='utf-8').read().splitlines()
print(json.loads(lines[lines.index('## Review Control Plane JSON') + 1])['groups'][0][0])
PY
)"
group_path="$(python3 - "$tmp_dir/rust.out" <<'PY'
import json, sys
lines = open(sys.argv[1], encoding='utf-8').read().splitlines()
payload = json.loads(lines[lines.index('## Review Control Plane JSON') + 1])
group = payload['groups'][0]
print(payload['units'][group[5][0]][0])
PY
)"
git -C "$fixture" -c color.ui=false diff --no-ext-diff --find-renames --cached -- "$group_path" \
  >"$tmp_dir/expected-group.diff"

for impl in rust legacy; do
  if (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --control-plane --group "$group_id"
  ) >"$tmp_dir/$impl-invalid.out" 2>&1; then
    fail "$impl accepted --control-plane with --group"
  fi
done

for impl in rust legacy; do
  follow="$tmp_dir/$impl-follow.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --group "$group_id" --expect-scope "$fingerprint"
  ) >"$follow"
  grep -Fq "scope_fingerprint: $fingerprint" "$follow" \
    || fail "$impl follow-up was not pinned to the opening scope"
  grep -Fq '## Requested Group Diff' "$follow" \
    || fail "$impl did not emit the requested group"
  python3 - "$follow" "$tmp_dir/expected-group.diff" <<'PY' \
    || fail "$impl group projection differed from its manifest snapshot"
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
start = text.index('```diff\n') + len('```diff\n')
end = text.index('```', start)
actual = text[start:end].encode()
expected = pathlib.Path(sys.argv[2]).read_bytes()
if actual != expected:
    raise SystemExit('projected diff bytes mismatch')
PY
done

for impl in rust legacy; do
  path_follow="$tmp_dir/$impl-path-follow.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --path "$group_path" --expect-scope "$fingerprint"
  ) >"$path_follow"
  grep -Fq "scope_fingerprint: $fingerprint" "$path_follow" \
    || fail "$impl path follow-up was not pinned to the opening scope"
  python3 - "$path_follow" "$tmp_dir/expected-group.diff" <<'PY' \
    || fail "$impl path projection differed from its manifest snapshot"
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
start = text.index('```diff\n') + len('```diff\n')
end = text.index('```', start)
if text[start:end].encode() != pathlib.Path(sys.argv[2]).read_bytes():
    raise SystemExit('projected path bytes mismatch')
PY
done

git -C "$fixture" -c color.ui=false diff --no-ext-diff --no-textconv --find-renames \
  --cached -- "$weird_path" >"$tmp_dir/expected-weird-path.diff"
for impl in rust legacy; do
  weird_follow="$tmp_dir/$impl-weird-follow.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --path "$weird_path" --expect-scope "$fingerprint"
  ) >"$weird_follow"
  if grep -Fq 'src/auth/session.ts' "$weird_follow"; then
    fail "$impl path response leaked metadata from another review unit"
  fi
  python3 - "$weird_follow" "$tmp_dir/expected-weird-path.diff" <<'PY' \
    || fail "$impl raw special path did not resolve to its quoted manifest cache key"
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
start = text.index('```diff\n') + len('```diff\n')
end = text.index('```', start)
if text[start:end].encode() != pathlib.Path(sys.argv[2]).read_bytes():
    raise SystemExit('raw special path projection mismatch')
PY
done

git -C "$fixture" diff --cached --textconv -- sample.dat | grep -Fq 'TEXTCONV_MARKER' \
  || fail 'textconv fixture did not exercise the configured diff driver'
for impl in rust legacy; do
  textconv_follow="$tmp_dir/$impl-textconv-follow.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --path sample.dat --expect-scope "$fingerprint"
  ) >"$textconv_follow"
  if grep -Fq 'TEXTCONV_MARKER' "$textconv_follow"; then
    fail "$impl scoped review bytes diverged from the no-textconv fingerprint semantics"
  fi
done

printf 'changed again\n' >>"$fixture/docs/review.md"
git -C "$fixture" add docs/review.md

for impl in rust legacy; do
  stale="$tmp_dir/$impl-stale.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --group "$group_id" --expect-scope "$fingerprint"
  ) >"$stale"
  python3 "$validator" --control-plane-output "$stale" >/dev/null \
    || fail "$impl stale-scope failure did not validate"
  grep -Fq '"authoritative":false' "$stale" \
    || fail "$impl stale scope did not fail closed"
  if grep -Fq '## Requested Group Diff' "$stale"; then
    fail "$impl stale scope leaked review content"
  fi
done

git -C "$fixture" commit -q -am 'consume staged fixture'
for impl in rust legacy; do
  no_diff="$tmp_dir/$impl-no-diff.out"
  (
    cd "$fixture"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --control-plane
  ) >"$no_diff"
  python3 "$validator" --control-plane-output "$no_diff" >/dev/null \
    || fail "$impl no-diff control plane did not validate"
  grep -Fq '"authoritative":false' "$no_diff" \
    || fail "$impl no-diff control plane did not fail closed"
  grep -Fq '"reason":"no_diff_available"' "$no_diff" \
    || fail "$impl no-diff control plane reported the wrong reason"
done
python3 - "$tmp_dir/rust-no-diff.out" "$tmp_dir/legacy-no-diff.out" <<'PY' \
  || fail 'Rust and legacy no-diff control planes differ'
import json
import pathlib
import sys

def payload(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    return json.loads(lines[lines.index('## Review Control Plane JSON') + 1])

if payload(sys.argv[1]) != payload(sys.argv[2]):
    raise SystemExit('no-diff payload mismatch')
PY

unborn="$tmp_dir/unborn"
mkdir -p "$unborn"
git -C "$unborn" init -q
printf 'first staged file\n' >"$unborn/first.txt"
git -C "$unborn" add first.txt
for impl in rust legacy; do
  unborn_output="$tmp_dir/$impl-unborn.out"
  (
    cd "$unborn"
    PRE_COMMIT_REVIEW_HELPER_IMPL="$impl" PRE_COMMIT_REVIEW_DISABLE_FALLBACK=1 \
      "$helper" --source staged --control-plane
  ) >"$unborn_output"
  python3 "$validator" --control-plane-output "$unborn_output" >/dev/null \
    || fail "$impl unborn-HEAD control plane did not validate"
  grep -Fq '"head":"unknown"' "$unborn_output" \
    || fail "$impl unborn-HEAD control plane did not use a stable non-empty identity"
done
python3 - "$tmp_dir/rust-unborn.out" "$tmp_dir/legacy-unborn.out" <<'PY' \
  || fail 'Rust and legacy unborn-HEAD control planes differ'
import json
import pathlib
import sys

def payload(path):
    lines = pathlib.Path(path).read_text(encoding='utf-8').splitlines()
    return json.loads(lines[lines.index('## Review Control Plane JSON') + 1])

rust = payload(sys.argv[1])
legacy = payload(sys.argv[2])
for key in ('source', 'head', 'base', 'selected_ref', 'scope_fingerprint', 'counts',
            'unit_tuple_fields', 'units', 'group_tuple_fields', 'groups', 'work_order'):
    if rust[key] != legacy[key]:
        raise SystemExit(f'unborn {key} mismatch')
PY

printf 'control plane tests passed\n'
