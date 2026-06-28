#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
checker="$repo_root/evals/run_marker_eval_checks.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'run marker eval checks test failed: %s\n' "$*" >&2
  exit 1
}

bash "$checker" >"$tmp_dir/default.out"

grep -Fq 'marker-eval.json' "$tmp_dir/default.out" \
  || fail 'checker output must mention the default marker eval file'
grep -Fq 'Covered markers: 🔒 ❌ ⚠️ 🧪 👁️ 📈 🧭' "$tmp_dir/default.out" \
  || fail 'checker output must list all covered primary markers'
grep -Fq 'Total cases: 7' "$tmp_dir/default.out" \
  || fail 'checker output must include the total case count'
grep -Fq 'Blocking cases: 3' "$tmp_dir/default.out" \
  || fail 'checker output must include the blocking case count'
grep -Fq 'Non-blocking cases: 4' "$tmp_dir/default.out" \
  || fail 'checker output must include the non-blocking case count'

cat >"$tmp_dir/incomplete-marker-eval.json" <<'EOF'
{
  "evaluation_method": "broken",
  "cases": [
    {
      "id": "only-security",
      "scenario": "hardcoded-secret",
      "locale": "en",
      "prompt": "Review this staged config diff before I commit.",
      "expected": {
        "verdict": "DO_NOT_COMMIT",
        "expected_primary_marker": "🔒",
        "expected_blocking": true,
        "expected_tally": {
          "blockers": 1,
          "warnings": 0,
          "test_gaps": 0,
          "review_limits": 0
        },
        "must_include": ["redacted", "rotate"]
      }
    }
  ]
}
EOF

if bash "$checker" --eval-file "$tmp_dir/incomplete-marker-eval.json" >"$tmp_dir/bad.out" 2>&1; then
  fail 'checker must fail when marker coverage is incomplete'
fi
grep -Fq 'missing required markers' "$tmp_dir/bad.out" \
  || fail 'checker must report missing required markers'

printf 'run marker eval checks tests passed\n'
