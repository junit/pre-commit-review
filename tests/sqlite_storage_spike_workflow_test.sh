#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

fail() {
  printf 'sqlite storage spike workflow test failed: %s\n' "$*" >&2
  exit 1
}

for workflow in lint.yml release.yml; do
  path="$repo_root/.github/workflows/$workflow"
  grep -Fq -- '--features sqlite-storage-spike' "$path" \
    || fail "$workflow does not enable the spike feature"
  grep -Fq -- '--bin sqlite-storage-spike' "$path" \
    || fail "$workflow does not select the spike binary"
  grep -Fq -- 'sqlite-storage-spike --help' "$path" \
    || fail "$workflow does not run the spike help smoke"
done

grep -Fq -- 'SQLite storage spike 100k gate' "$repo_root/.github/workflows/lint.yml" \
  || fail 'lint workflow does not run the 100k spike gate'
grep -Fq -- 'SQLite storage spike 1M gate' "$repo_root/.github/workflows/lint.yml" \
  || fail 'lint workflow does not run the 1M spike gate'
grep -Fq -- 'Build SQLite storage spike' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not build the spike on every target'
grep -Fq -- 'Smoke-test SQLite storage spike' "$repo_root/.github/workflows/release.yml" \
  || fail 'release workflow does not smoke-test the spike on every target'

if grep -Eq 'cp .*sqlite-storage-spike|find artifacts .*sqlite-storage-spike|sqlite-storage-spike.*dist/' \
  "$repo_root/.github/workflows/release.yml"; then
  fail 'release workflow packages the temporary spike binary'
fi

printf 'sqlite storage spike workflow tests passed\n'
