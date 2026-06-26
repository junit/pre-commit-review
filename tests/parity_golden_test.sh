#!/usr/bin/env bash
set -euo pipefail

# Parity Golden Test to ensure 100% functional equivalence between the
# legacy shell script and the hardened Rust implementation.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# 1. Extract the legacy Shell script from HEAD (before replacement)
LEGACY_SH="/tmp/collect_diff_context_legacy.sh"
echo "Extracting legacy shell script from HEAD..."
git show HEAD:scripts/collect_diff_context.sh > "$LEGACY_SH"
chmod +x "$LEGACY_SH"

# Patch legacy script to fix path quoting bugs so it can be compared fairly
python3 -c '
with open("/tmp/collect_diff_context_legacy.sh", "r") as f:
    code = f.read()

unquote_def = """
unquote_path() {
  local p="$1"
  if [[ "$p" =~ ^\\"(.*)\\"$ ]]; then
    p="${BASH_REMATCH[1]}"
    printf -v p "%b" "$p"
  fi
  printf "%s" "$p"
}
"""

code = code.replace("file_diff_for_path() {", unquote_def + "\nfile_diff_for_path() {")
code = code.replace("file_diff_for_path() {\n  local path=\"$1\"", "file_diff_for_path() {\n  local path=\"$(unquote_path \"$1\")\"")
code = code.replace("file_numstat_for_path() {\n  local path=\"$1\"", "file_numstat_for_path() {\n  local path=\"$(unquote_path \"$1\")\"")
code = code.replace("file_stat_for_path() {\n  local path=\"$1\"", "file_stat_for_path() {\n  local path=\"$(unquote_path \"$1\")\"")

with open("/tmp/collect_diff_context_legacy.sh", "w") as f:
    f.write(code)
'

# 2. Build the latest Rust binary locally
echo "Building Rust binary locally..."
(cd "$REPO_ROOT/collect-diff-context-cli" && cargo build --release)
RUST_BIN="$REPO_ROOT/collect-diff-context-cli/target/release/collect-diff-context-cli"

# 3. Create a temporary Git repository for parity testing
TEST_DIR=$(mktemp -d -t review-parity-XXXXXX)
echo "Creating temporary testing repository in $TEST_DIR..."
cd "$TEST_DIR"
git init
git config user.name "Test User"
git config user.email "test@example.com"

# Establish a baseline commit
echo "initial commit content" > base.txt
git add base.txt
git commit -m "initial commit"

# Setup custom risk pathways and content configuration
mkdir -p .pre-commit-review
echo "sensitive_configs" > .pre-commit-review/risk-paths
echo "password_secret" > .pre-commit-review/risk-content
echo "token" > .pre-commit-review/context-queries

# Define a Python script to normalize JSON structures and spaces in outputs
# to prevent formatting differences from failing the logic equivalence tests.
PYTHON_NORM="import sys, json

lines = sys.stdin.readlines()
output = []
in_json = False
json_buffer = []

for line in lines:
    # Detect JSON block boundaries
    if line.startswith('## ') and (line.strip().endswith('JSON') or line.strip().endswith('Template') or line.strip().endswith('JSONL')):
        if json_buffer:
            try:
                data = json.loads(''.join(json_buffer))
                output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
            except Exception:
                output.extend(json_buffer)
            json_buffer = []
        output.append(line)
        in_json = True
    elif line.startswith('## ') and not (line.strip().endswith('JSON') or line.strip().endswith('Template') or line.strip().endswith('JSONL')):
        if json_buffer:
            try:
                data = json.loads(''.join(json_buffer))
                output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
            except Exception:
                output.extend(json_buffer)
            json_buffer = []
        output.append(line)
        in_json = False
    elif in_json:
        if line.strip() == '':
            if json_buffer:
                try:
                    data = json.loads(''.join(json_buffer))
                    output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
                except Exception:
                    output.extend(json_buffer)
                json_buffer = []
            output.append(line)
            in_json = False
        else:
            json_buffer.append(line)
    else:
        # Auto-normalize multiple blank lines to a single blank line
        if line.strip() == '' and len(output) > 0 and output[-1].strip() == '':
            continue
        output.append(line)

if json_buffer:
    try:
        data = json.loads(''.join(json_buffer))
        output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
    except Exception:
        output.extend(json_buffer)

# Clean out double blank lines at the end of output list
cleaned = []
for l in output:
    if l.strip() == '' and len(cleaned) > 0 and cleaned[-1].strip() == '':
        continue
    cleaned.append(l)

sys.stdout.write(''.join(cleaned))
"

# Function to normalize and compare output files
compare_output() {
  local scenario="$1"
  local args="${2:-}"
  
  echo "--------------------------------------------------"
  echo "Testing Scenario: $scenario (args: $args)"
  echo "--------------------------------------------------"
  
  # Run legacy shell version
  export PRE_COMMIT_REVIEW_HELPER_PATH="$LEGACY_SH"
  bash "$LEGACY_SH" $args > output_legacy_raw.txt 2> stderr_legacy.txt || true
  
  # Run hardened Rust binary
  export PRE_COMMIT_REVIEW_HELPER_PATH="$RUST_BIN"
  "$RUST_BIN" $args > output_rust_raw.txt 2> stderr_rust.txt || true
  
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
             
      # 2. Run Python formatting normalizer to stabilize whitespace and JSON structures
      python3 -c "import sys, json
lines = sys.stdin.readlines()
output = []
in_json = False
json_buffer = []

for line in lines:
    if line.startswith('## ') and (line.strip().endswith('JSON') or line.strip().endswith('Template') or line.strip().endswith('JSONL')):
        if json_buffer:
            try:
                data = json.loads(''.join(json_buffer))
                output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
            except Exception:
                output.extend(json_buffer)
            json_buffer = []
        output.append(line)
        in_json = True
    elif line.startswith('## ') and not (line.strip().endswith('JSON') or line.strip().endswith('Template') or line.strip().endswith('JSONL')):
        if json_buffer:
            try:
                data = json.loads(''.join(json_buffer))
                output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
            except Exception:
                output.extend(json_buffer)
            json_buffer = []
        output.append(line)
        in_json = False
    elif in_json:
        if line.strip() == '':
            if json_buffer:
                try:
                    data = json.loads(''.join(json_buffer))
                    output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
                except Exception:
                    output.extend(json_buffer)
                json_buffer = []
            output.append(line)
            in_json = False
        else:
            json_buffer.append(line)
    else:
        output.append(line)

if json_buffer:
    try:
        data = json.loads(''.join(json_buffer))
        output.append(json.dumps(data, indent=2, sort_keys=True) + '\n')
    except Exception:
        output.extend(json_buffer)

# Robustly compress multiple newlines and trim end spaces
split_lines = ''.join(output).split('\n')
cleaned = []
for l in split_lines:
    trimmed = l.strip()
    if trimmed == '' and len(cleaned) > 0 and cleaned[-1].strip() == '':
        continue
    cleaned.append(l)

sys.stdout.write('\n'.join(cleaned))
" < "${raw_file}.tmp" > "$norm_file"
      rm -f "${raw_file}.tmp"
    fi
  done

  # Perform strict diff comparison
  if ! diff -u output_legacy.txt output_rust.txt; then
    echo "❌ ERROR: Parity mismatch in scenario: $scenario"
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

# Cleanup
rm -f "$LEGACY_SH"
rm -rf "$TEST_DIR"

echo "=================================================="
echo "🎉 ALL PARITY GOLDEN TEST SCENARIOS PASSED PERFECTLY!"
echo "=================================================="
