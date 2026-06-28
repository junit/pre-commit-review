#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
eval_file="$repo_root/evals/taxonomy/marker-eval.json"

required_markers=('🔒' '❌' '⚠️' '🧪' '👁️' '📈' '🧭')

usage() {
  cat <<'EOF'
Usage: run_marker_eval_checks.sh [--eval-file FILE]

Validate and summarize taxonomy marker eval coverage.

Options:
  --eval-file FILE   Override the marker eval JSON file
  -h, --help         Show this help
EOF
}

fail() {
  printf 'run marker eval checks failed: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --eval-file)
      shift
      [ "$#" -gt 0 ] || fail '--eval-file requires a value'
      eval_file="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

command -v jq >/dev/null 2>&1 || fail 'jq is required'
[ -f "$eval_file" ] || fail "missing marker eval file: $eval_file"
jq empty "$eval_file" >/dev/null || fail 'marker eval file must be valid JSON'

jq -e '
  has("evaluation_method") and
  has("cases") and
  (.cases | type == "array" and length >= 1) and
  all(.cases[];
    has("id") and
    has("scenario") and
    has("prompt") and
    has("locale") and
    has("expected") and
    (.expected | has("verdict") and has("expected_primary_marker") and has("expected_blocking") and has("expected_tally") and has("must_include"))
  )
' "$eval_file" >/dev/null || fail 'marker eval file is missing required fields'

missing_markers=()
covered_markers=()
for marker in "${required_markers[@]}"; do
  if jq -e --arg marker "$marker" '.cases | map(.expected.expected_primary_marker) | index($marker) != null' "$eval_file" >/dev/null; then
    covered_markers+=("$marker")
  else
    missing_markers+=("$marker")
  fi
done

if [ "${#missing_markers[@]}" -gt 0 ]; then
  fail "missing required markers: ${missing_markers[*]}"
fi

total_cases="$(jq -r '.cases | length' "$eval_file")"
blocking_cases="$(jq -r '[.cases[] | select(.expected.expected_blocking == true)] | length' "$eval_file")"
nonblocking_cases="$(jq -r '[.cases[] | select(.expected.expected_blocking == false)] | length' "$eval_file")"

printf 'Marker eval file: %s\n' "$eval_file"
printf 'Covered markers: %s\n' "${covered_markers[*]}"
printf 'Total cases: %s\n' "$total_cases"
printf 'Blocking cases: %s\n' "$blocking_cases"
printf 'Non-blocking cases: %s\n' "$nonblocking_cases"
