#!/usr/bin/env bash

set -euo pipefail

repo_root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
wrapper="$repo_root/scripts/check_artifacts.sh"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/check-artifacts-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

target="$tmp_dir/target"
mkdir -p "$target/scripts/lib" "$target/scripts/bin"
canonical_target="$(CDPATH='' cd -- "$target" && pwd -P)"

cat >"$target/scripts/lib/collect_diff_context_cli.sh" <<'EOF_RESOLVER'
#!/usr/bin/env bash

resolve_packaged_collect_diff_context_cli() {
  printf '%s\n' "$1/bin/custom-collector"
}
EOF_RESOLVER
chmod +x "$target/scripts/lib/collect_diff_context_cli.sh"

cat >"$target/scripts/bin/custom-collector" <<EOF_COLLECTOR
#!/usr/bin/env bash
printf '%s\n' "\$*" >"$target/arguments"
printf '{"custom":"ok"}'
exit 7
EOF_COLLECTOR
chmod +x "$target/scripts/bin/custom-collector"

set +e
output="$($wrapper "$target" 2>"$target/stderr")"
status=$?
set -e

[ "$status" -eq 7 ]
[ "$output" = '{"custom":"ok"}' ]
[ ! -s "$target/stderr" ]
[ "$(cat "$target/arguments")" = "artifacts doctor --target-root $canonical_target" ]

if "$wrapper" relative-target >/dev/null 2>&1; then
  printf 'relative target unexpectedly succeeded\n' >&2
  exit 1
fi

printf 'check_artifacts tests passed\n'
