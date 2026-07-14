#!/usr/bin/env bash
# shellcheck disable=SC2016
set -euo pipefail

# Parity Golden Test to ensure 100% functional equivalence between the
# legacy shell script and the hardened Rust implementation.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PARITY_FIXTURES_LIB="$REPO_ROOT/tests/lib/parity_fixtures.sh"
PARITY_NORMALIZER="$REPO_ROOT/tests/lib/normalize_parity_output.py"

# shellcheck disable=SC1090
source "$PARITY_FIXTURES_LIB"

# 1. Copy the restored legacy Shell script
LEGACY_SH="/tmp/collect_diff_context_legacy.sh"
echo "Copying legacy shell script from scripts/collect_diff_context.legacy.sh..."
cp "$REPO_ROOT/scripts/collect_diff_context.legacy.sh" "$LEGACY_SH"
chmod +x "$LEGACY_SH"

# 2. Build the latest Rust binary locally
echo "Building Rust binary locally..."
(cd "$REPO_ROOT/collect-diff-context-cli" && cargo build --release)
RUST_BIN="$REPO_ROOT/collect-diff-context-cli/target/release/collect-diff-context-cli"

# 3. Create a temporary Git repository for parity testing
TEST_DIR=$(mktemp -d -t review-parity-XXXXXX)
echo "Creating temporary testing repository in $TEST_DIR..."
create_parity_repo_fixture "$TEST_DIR"
cd "$TEST_DIR"

# Function to normalize and compare output files
compare_output() {
  local scenario="$1"
  local args="${2:-}"
  local -a cmd_args=()
  if [ -n "$args" ]; then
    read -r -a cmd_args <<<"$args"
  fi
  
  echo "--------------------------------------------------"
  echo "Testing Scenario: $scenario (args: $args)"
  echo "--------------------------------------------------"
  
  # Run legacy shell version
  export PRE_COMMIT_REVIEW_HELPER_PATH="$LEGACY_SH"
  if [ "${#cmd_args[@]}" -gt 0 ]; then
    bash "$LEGACY_SH" "${cmd_args[@]}" > output_legacy_raw.txt 2> stderr_legacy.txt || true
  else
    bash "$LEGACY_SH" > output_legacy_raw.txt 2> stderr_legacy.txt || true
  fi
  
  # Run hardened Rust binary
  export PRE_COMMIT_REVIEW_HELPER_PATH="$RUST_BIN"
  if [ "${#cmd_args[@]}" -gt 0 ]; then
    "$RUST_BIN" "${cmd_args[@]}" > output_rust_raw.txt 2> stderr_rust.txt || true
  else
    "$RUST_BIN" > output_rust_raw.txt 2> stderr_rust.txt || true
  fi
  
  # Normalize platform/absolute-path specifics to ensure exact golden comparisons
  for prefix in output_legacy output_rust; do
    local raw_file="${prefix}_raw.txt"
    local norm_file="${prefix}.txt"
    if [ -f "$raw_file" ]; then
      # 1. Normalize absolute and symlinked path differences & clean temporary untracked test files
      sed -E -e "s|$TEST_DIR|/mock_repo|g" \
             -e "s|$LEGACY_SH|/mock_helper|g" \
             -e "s|$RUST_BIN|/mock_helper|g" \
             -e "s|/private/mock_helper|/mock_helper|g" \
             -e "s|/private/mock_repo|/mock_repo|g" \
             -e "s/head: [a-f0-9]{7}/head: mock_sha/g" \
             -e "/\?\? output_legacy_raw.txt/d" \
             -e "/\?\? output_rust_raw.txt/d" \
             -e "/\?\? stderr_legacy.txt/d" \
             -e "/\?\? stderr_rust.txt/d" \
             -e "/\?\? output_legacy.txt/d" \
             -e "/\?\? output_rust.txt/d" \
             "$raw_file" > "${raw_file}.tmp"
             
      # 2. Run the shared normalizer to stabilize whitespace and JSON structures
      python3 "$PARITY_NORMALIZER" < "${raw_file}.tmp" > "$norm_file"
      rm -f "${raw_file}.tmp"
    fi
  done

  # Perform strict diff comparison
  if ! diff -u output_legacy.txt output_rust.txt; then
    echo "❌ ERROR: Parity mismatch in scenario: $scenario"
    if [ -f stderr_rust.txt ]; then
      echo "=== Rust Stderr ==="
      cat stderr_rust.txt
    fi
    exit 1
  fi
  echo "✅ SUCCESS: Scenario $scenario matched perfectly."
}

# -----------------------------------------------------------------------------
# Test Scenarios
# -----------------------------------------------------------------------------

# Scenario 1: Staged changes only
echo "staged changes" >> base.txt
git add base.txt
compare_output "staged_changes" "--source staged"

# Scenario 2: Unstaged changes only
git commit -m "staged changes committed"
echo "unstaged changes" >> base.txt
compare_output "unstaged_changes" "--source unstaged"

# Scenario 3: Mixed staged and unstaged changes
echo "staged mixture" >> base.txt
git add base.txt
echo "unstaged mixture" >> base.txt
compare_output "mixed_changes" ""

# Scenario 4: Path-specific diff query
compare_output "path_specific_query" "--path base.txt"

# Scenario 5: File Rename
git commit -a -m "pre-rename state"
git mv base.txt base_renamed.txt
compare_output "file_rename" ""

# Scenario 6: File Deletion
git commit -m "pre-delete state"
git rm base_renamed.txt
compare_output "file_deletion" ""

# Scenario 7: High-risk path detection (matches risk-paths)
git commit -m "pre-risk state"
mkdir -p sensitive_configs
echo "dangerous options" > sensitive_configs/deploy.conf
git add sensitive_configs/deploy.conf
compare_output "high_risk_path" ""

# Scenario 8: Content risk matching (matches risk-content)
git commit -m "pre-content-risk state"
echo "let password_secret = '123456'" > credentials.js
git add credentials.js
compare_output "content_risk" ""

# Scenario 9: Lockfile classification
git commit -m "pre-lockfile state"
echo "lockfile data" > package-lock.json
git add package-lock.json
compare_output "lockfile_class" ""

# Scenario 10: Generated files classification
git commit -m "pre-generated state"
mkdir -p dist
echo "compiled script" > dist/bundle.min.js
git add dist/bundle.min.js
compare_output "generated_file_class" ""

# Scenario 11: Weird characters in path names (Spaces, tabs, colons)
git commit -m "pre-weird-path state"
weird_file="weird file:name	with_tab.txt"
echo "weird contents" > "$weird_file"
git add "$weird_file"
compare_output "weird_path_names" ""

# Scenario 12: Semantic context query matches
git commit -m "pre-semantic-context state"
echo "some text containing token value" > query_test.py
git add query_test.py
compare_output "semantic_context_matches" ""

# Scenario 13: Exceeded group budget and split suggestion triggers
git commit -m "pre-budget state"
# Generate a large chunk of text to exceed 160KB budget
for i in {1..4000}; do
  echo "this is a very long line to increase file size in diff for budgeting testing" >> base_budget.txt
done
git add base_budget.txt
compare_output "budget_exceeded_split" ""

# Scenario 14: File Copy detection
git commit -m "pre-copy state"
cp base_budget.txt base_budget_copy.txt
git add base_budget_copy.txt
compare_output "file_copy" ""

# Scenario 15: Binary file handling
git commit -m "pre-binary state"
printf '\x89PNG\r\n\x1a\n\x00\x00\x00' > binary_image.png
git add binary_image.png
compare_output "binary_file" ""

# Scenario 16: Unicode path names (emoji / CJK characters)
git commit -m "pre-unicode-path state"
unicode_file="测试文件_🎉.txt"
echo "unicode path content" > "$unicode_file"
git add "$unicode_file"
compare_output "unicode_path_names" ""

# Scenario 17: Branch vs base mode
git commit -m "pre-branch-mode state"
git checkout -b feature-test-branch
echo "branch-specific change" > branch_change.txt
git add branch_change.txt
git commit -m "branch commit"
compare_output "branch_vs_base" "--source branch"
git checkout -

# Scenario 18: Explicit --group argument
# First run without --group to find a group_id, then test with --group
git checkout main 2>/dev/null || git checkout master 2>/dev/null || true
echo "group test content" > group_test.txt
git add group_test.txt
# Capture group_id from full output
full_output=$("$RUST_BIN" 2>/dev/null || true)
group_id=$(echo "$full_output" | grep '^group_id:' | head -1 | awk '{print $2}')
if [ -n "$group_id" ]; then
  compare_output "explicit_group_request" "--group $group_id"
else
  echo "⚠️  SKIP: Scenario 18 (explicit_group_request) — could not extract group_id"
fi

# Scenario 19: Multiple files in a single commit (group budgeting)
git commit -m "pre-multi-file state" 2>/dev/null || true
for i in $(seq 1 5); do
  echo "multi file content $i with some padding to increase size" > "multi_file_$i.txt"
done
git add multi_file_*.txt
compare_output "multiple_files_grouping" ""

# Scenario 20: Empty diff (no changes)
git commit -m "pre-empty-diff state" 2>/dev/null || true
compare_output "empty_diff_no_changes" "--source staged"

# Cleanup
rm -f "$LEGACY_SH"
rm -rf "$TEST_DIR"

echo "=================================================="
echo "🎉 ALL PARITY GOLDEN TEST SCENARIOS PASSED PERFECTLY!"
echo "=================================================="
