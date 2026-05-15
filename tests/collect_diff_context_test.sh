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

  grep -Fq "$expected" "$file" || {
    printf '--- output ---\n' >&2
    cat "$file" >&2
    printf '--------------\n' >&2
    fail "expected output to contain: $expected"
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

printf 'collect_diff_context tests passed\n'
