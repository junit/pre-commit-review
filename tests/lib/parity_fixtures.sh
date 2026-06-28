#!/usr/bin/env bash
set -euo pipefail

create_parity_repo_fixture() {
  local repo_dir="$1"

  mkdir -p "$repo_dir"
  git -C "$repo_dir" init -q
  git -C "$repo_dir" config user.name "Test User"
  git -C "$repo_dir" config user.email "test@example.com"

  printf 'initial commit content\n' >"$repo_dir/base.txt"
  git -C "$repo_dir" add base.txt
  git -C "$repo_dir" commit -q -m "initial commit"

  mkdir -p "$repo_dir/.pre-commit-review"
  printf 'sensitive_configs\n' >"$repo_dir/.pre-commit-review/risk-paths"
  printf 'password_secret\n' >"$repo_dir/.pre-commit-review/risk-content"
  printf 'token\n' >"$repo_dir/.pre-commit-review/context-queries"
}
