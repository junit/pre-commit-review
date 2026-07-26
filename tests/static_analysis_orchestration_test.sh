#!/usr/bin/env bash
set -euo pipefail

skill_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cargo test \
  --manifest-path "$skill_root/collect-diff-context-cli/Cargo.toml" \
  --test static_orchestration \
  contracts

echo "static analysis orchestration contract tests passed"
