#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"

fail() {
  printf 'artifact reachability test failed: %s\n' "$*" >&2
  exit 1
}

runtime_wrappers=(
  scripts/collect_diff_context.sh
  scripts/collect_static_evidence.sh
  scripts/run_static_analysis.sh
  scripts/orchestrate_static_analysis.sh
  scripts/index_repository_context.sh
  scripts/run_repository_context_provider.sh
  scripts/lib/repository_context_provider_cli.sh
)
for relative_path in "${runtime_wrappers[@]}"; do
  path="$repo_root/$relative_path"
  [ -f "$path" ] || fail "missing runtime wrapper: $relative_path"
  if rg -n '(^|[[:space:]])artifacts (verify|provision|doctor)|fetch_gitleaks|rust-analyzer' "$path"; then
    fail "$relative_path can reach artifact provisioning or a third-party binary"
  fi
done

if rg -n 'cargo[[:space:]]+(build|install)|rustup|rust:latest|apt-get|brew[[:space:]]+install|npm[[:space:]]+install' \
  "$repo_root/scripts/collect_diff_context.sh" \
  "$repo_root/scripts/build_all_binaries.sh"; then
  fail 'runtime or local builder contains an implicit toolchain/package fallback'
fi

for source_dir in \
  "$repo_root/collect-diff-context-cli/src/static_analysis" \
  "$repo_root/collect-diff-context-cli/src/impact_context"; do
  if rg -n 'crate::artifacts|artifacts::cli|ArtifactCommand' "$source_dir"; then
    fail "ordinary analysis source reaches the artifacts command"
  fi
done

if git -C "$repo_root" ls-files 'collect-diff-context-cli/fuzz/artifacts/**' | grep -q .; then
  fail 'generated fuzz artifact files are tracked'
fi
while IFS= read -r corpus_path; do
  corpus_name="$(basename "$corpus_path")"
  if [[ "$corpus_name" =~ ^[0-9a-f]{16,64}$ ]]; then
    fail "hash-named fuzz corpus file is tracked: $corpus_path"
  fi
done < <(git -C "$repo_root" ls-files 'collect-diff-context-cli/fuzz/corpus/**')

if git -C "$repo_root" ls-files | rg -i '(^|/)rust-analyzer(-|_)(darwin|linux|windows)|(^|/)rust-analyzer(\.exe)?$'; then
  fail 'rust-analyzer executable is tracked in the source tree'
fi

printf 'artifact reachability tests passed\n'
