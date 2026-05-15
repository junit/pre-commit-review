#!/usr/bin/env bash
set -euo pipefail

# Collect Git diff context for the pre-commit-review skill.
# This script is read-only. It does not fetch, stage, reset, install, or mutate files.
# Optional environment variable:
#   PRE_COMMIT_REVIEW_MAX_DIFF_BYTES  default 200000; set 0 for no truncation

MAX_DIFF_BYTES="${PRE_COMMIT_REVIEW_MAX_DIFF_BYTES:-200000}"
case "$MAX_DIFF_BYTES" in
  ''|*[!0-9]*) MAX_DIFF_BYTES=200000 ;;
esac

print_kv() {
  printf '%s: %s\n' "$1" "$2"
}

fail_no_repo() {
  echo '# Pre-Commit Review Diff Context'
  echo
  print_kv 'repository' 'not a git repository'
  print_kv 'diff_source' 'unavailable'
  print_kv 'review_limits' 'no local repository access'
  echo
  echo 'No diff available. Stage your changes or provide a diff to review.'
  exit 0
}

if ! repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
  fail_no_repo
fi

cd "$repo_root"

has_staged_changes() {
  set +e
  git diff --cached --quiet --exit-code -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

has_unstaged_changes() {
  set +e
  git diff --quiet --exit-code -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

has_diff_for_ref() {
  local ref="$1"
  set +e
  git diff --quiet --exit-code "$ref...HEAD" -- . >/dev/null 2>&1
  local rc=$?
  set -e
  [ "$rc" -eq 1 ]
}

detect_base_branch() {
  local base=''
  base="$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||' || true)"

  if [ -z "$base" ]; then
    if git rev-parse --verify --quiet origin/main >/dev/null; then
      base='main'
    elif git rev-parse --verify --quiet origin/master >/dev/null; then
      base='master'
    elif git rev-parse --verify --quiet main >/dev/null; then
      base='main'
    elif git rev-parse --verify --quiet master >/dev/null; then
      base='master'
    else
      base='main'
    fi
  fi

  printf '%s\n' "$base"
}

summarize_numstat() {
  awk '
    BEGIN { files=0; add=0; del=0 }
    NF >= 3 {
      files += 1
      if ($1 ~ /^[0-9]+$/) add += $1
      if ($2 ~ /^[0-9]+$/) del += $2
    }
    END { printf "%d files, %d insertions(+), %d deletions(-)", files, add, del }
  '
}

emit_diff_limited() {
  local tmp_file="$1"
  local size
  size="$(wc -c < "$tmp_file" | tr -d ' ')"

  print_kv 'diff_bytes' "$size"
  print_kv 'max_diff_bytes' "$MAX_DIFF_BYTES"

  echo
  echo '## Diff'
  echo '```diff'
  if [ "$MAX_DIFF_BYTES" = '0' ] || [ "$size" -le "$MAX_DIFF_BYTES" ]; then
    cat "$tmp_file"
  else
    head -c "$MAX_DIFF_BYTES" "$tmp_file"
    echo
    printf '[diff truncated after %s bytes; inspect high-risk files with file-specific git diff commands before making safety claims]\n' "$MAX_DIFF_BYTES"
  fi
  echo '```'
}

branch="$(git branch --show-current 2>/dev/null || true)"
head_sha="$(git rev-parse --short HEAD 2>/dev/null || true)"
base="$(detect_base_branch)"

staged='no'
unstaged='no'
if has_staged_changes; then staged='yes'; fi
if has_unstaged_changes; then unstaged='yes'; fi
untracked_names="$(git ls-files --others --exclude-standard || true)"

mode='none'
source='none'
review_limit_note='no diff found in staged, unstaged, or branch-vs-base comparisons'
unreviewed_note='none'
selected_ref=''

diff_tmp="$(mktemp)"
staged_files=''
unstaged_files=''
same_files=''
trap 'rm -f "$diff_tmp" ${staged_files:+"$staged_files"} ${unstaged_files:+"$unstaged_files"} ${same_files:+"$same_files"}' EXIT

run_diff() {
  case "$mode" in
    staged) git -c color.ui=false diff --no-ext-diff --find-renames --cached "$@" -- . ;;
    unstaged) git -c color.ui=false diff --no-ext-diff --find-renames "$@" -- . ;;
    branch) git -c color.ui=false diff --no-ext-diff --find-renames "$@" "$selected_ref...HEAD" -- . ;;
    *) return 1 ;;
  esac
}

if [ "$staged" = 'yes' ]; then
  mode='staged'
  source='staged changes via git diff --cached'
  review_limit_note='full staged diff available unless truncated by helper output limit'
  run_diff > "$diff_tmp"

  if [ "$unstaged" = 'yes' ]; then
    unreviewed_note='unstaged changes exist and were not reviewed as part of the staged commit candidate'
    staged_files="$(mktemp)"
    unstaged_files="$(mktemp)"
    same_files="$(mktemp)"
    git diff --cached --name-only -- . | sort -u > "$staged_files"
    git diff --name-only -- . | sort -u > "$unstaged_files"
    comm -12 "$staged_files" "$unstaged_files" > "$same_files" || true
    if [ -s "$same_files" ]; then
      overlap_csv="$(paste -sd ',' "$same_files")"
      unreviewed_note="unstaged changes touch files also staged for commit; actual working tree behavior may differ from reviewed commit candidate: ${overlap_csv}"
    fi
  fi
elif [ "$unstaged" = 'yes' ]; then
  mode='unstaged'
  source='unstaged changes via git diff'
  review_limit_note='full unstaged diff available unless truncated by helper output limit'
  run_diff > "$diff_tmp"
else
  remote_ref="origin/$base"
  if git rev-parse --verify --quiet "$remote_ref" >/dev/null && has_diff_for_ref "$remote_ref"; then
    mode='branch'
    selected_ref="$remote_ref"
    source="branch vs base via git diff ${remote_ref}...HEAD"
    review_limit_note="full diff available from local ${remote_ref}; remote freshness not verified because git fetch was not run"
    run_diff > "$diff_tmp"
  elif git rev-parse --verify --quiet "$base" >/dev/null && has_diff_for_ref "$base"; then
    mode='branch'
    selected_ref="$base"
    source="branch vs local base via git diff ${base}...HEAD"
    review_limit_note='full local branch-vs-base diff available unless truncated by helper output limit'
    run_diff > "$diff_tmp"
  fi
fi

if [ -n "$untracked_names" ]; then
  if [ "$unreviewed_note" = 'none' ]; then
    unreviewed_note='untracked files exist but are not part of git diff; stage them or provide file content to review'
  else
    unreviewed_note="$unreviewed_note; untracked files exist but are not part of git diff"
  fi
fi

if [ -s "$diff_tmp" ]; then
  diff_size="$(wc -c < "$diff_tmp" | tr -d ' ')"
  if [ "$MAX_DIFF_BYTES" != '0' ] && [ "$diff_size" -gt "$MAX_DIFF_BYTES" ]; then
    review_limit_note='partial diff output; inspect file list and prioritize risky files before making safety claims'
  fi
fi

files_changed='0 files, 0 insertions(+), 0 deletions(-)'
if [ "$mode" != 'none' ]; then
  files_changed="$(run_diff --numstat | summarize_numstat)"
fi

echo '# Pre-Commit Review Diff Context'
echo
print_kv 'repository' "$repo_root"
print_kv 'branch' "${branch:-detached-or-unknown}"
print_kv 'head' "${head_sha:-unknown}"
print_kv 'detected_base' "$base"
print_kv 'diff_source' "$source"
print_kv 'review_limits' "$review_limit_note"
print_kv 'files_changed' "$files_changed"
print_kv 'staged_changes' "$staged"
print_kv 'unstaged_changes' "$unstaged"
if [ -n "$untracked_names" ]; then
  print_kv 'untracked_files' 'yes'
else
  print_kv 'untracked_files' 'no'
fi
print_kv 'unreviewed_changes' "$unreviewed_note"
echo

echo '## Status'
git status --short || true
echo

if [ "$mode" = 'none' ]; then
  echo 'No diff available. Stage your changes or provide a diff to review.'
  exit 0
fi

echo '## Diff Stat'
run_diff --stat || true
echo

echo '## File List'
run_diff --name-status || true
echo

echo '## Numstat'
run_diff --numstat || true

if [ -n "$same_files" ] && [ -s "$same_files" ]; then
  echo
  echo '## Staged Files With Unstaged Changes Too'
  cat "$same_files"
fi

emit_diff_limited "$diff_tmp"
