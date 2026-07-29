#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
lint="$repo_root/.github/workflows/lint.yml"
release="$repo_root/.github/workflows/release.yml"
cargo_manifest="$repo_root/collect-diff-context-cli/Cargo.toml"
fuzz_overlay="$repo_root/collect-diff-context-cli/fuzz/fuzz_targets/repository_overlay.rs"
fuzz_traversal="$repo_root/collect-diff-context-cli/fuzz/fuzz_targets/repository_traversal.rs"
repository_bench="$repo_root/collect-diff-context-cli/benches/repository_index.rs"

fail() {
  printf 'repository index workflow test failed: %s\n' "$*" >&2
  exit 1
}

grep -Fq 'cargo clippy --all-targets --all-features -- -D warnings' "$lint" \
  || fail 'lint workflow does not run all-feature Clippy'
grep -Fq 'cargo test --release --test repository_index_integration -- --nocapture' "$lint" \
  || fail 'lint workflow does not run production repository-index release gates'
grep -Fq 'cargo bench --bench repository_index -- --test' "$lint" \
  || fail 'lint workflow does not smoke all repository-index benchmark stages'
for target in file_facts_decode repository_graph_row repository_overlay repository_traversal; do
  grep -Fq "cargo +nightly fuzz run $target" "$lint" \
    || fail "lint workflow does not fuzz $target"
done
for target in "$fuzz_overlay" "$fuzz_traversal"; do
  grep -Fq 'arbitrary_graph' "$target" \
    || fail "$(basename "$target") does not derive repository graphs from fuzz input"
  if grep -Eq 'synthetic_graph|OnceLock' "$target"; then
    fail "$(basename "$target") still relies on a fixed repository graph fixture"
  fi
done
if grep -Fq 'scale_row_stream' "$repository_bench"; then
  fail 'repository scale benchmark still hashes fake row pairs'
fi
grep -Fq 'scale/sqlite_generation' "$repository_bench" \
  || fail 'repository scale benchmark does not exercise real SQLite generations'
grep -Fq '.integrity_check()' "$repository_bench" \
  || fail 'repository scale benchmark does not validate generation integrity'

# shellcheck disable=SC2016,SC1003 # the assertion intentionally matches literal workflow variables and a trailing continuation
grep -Fq 'cargo +1.95.0 build --release --locked --target ${{ matrix.target }} --bins' "$release" \
  || fail 'release workflow does not build the bundled product binaries'
grep -Fq 'index build --source staged' "$release" \
  || fail 'release workflow does not build a production repository index'
grep -Fq 'index doctor --cache-dir' "$release" \
  || fail 'release workflow does not doctor the production repository index'
grep -Fq 'index inspect --generation' "$release" \
  || fail 'release workflow does not run an immutable production query'
# shellcheck disable=SC2016,SC1003 # the assertion intentionally matches literal workflow variables and a trailing continuation
grep -Fq 'inspect_report="$(cd "$repository" && PRE_COMMIT_REVIEW_CACHE_DIR="$cache" \' "$release" \
  || fail 'release workflow does not bind inspect to the smoke cache through the supported environment override'
if grep -Eq 'index inspect .*--cache-dir' "$release"; then
  fail 'release workflow passes unsupported --cache-dir to index inspect'
fi
grep -Fq 'rusqlite@0.40.1' "$release" \
  || fail 'release SBOM gate does not require rusqlite'
grep -Fq 'libsqlite3-sys@0.38.1' "$release" \
  || fail 'release SBOM gate does not require bundled SQLite bindings'
grep -Fq 'THIRD_PARTY_LICENSES/rusqlite-LICENSE' "$release" \
  || fail 'release package does not verify rusqlite license evidence'
grep -Fq 'THIRD_PARTY_LICENSES/sqlite-PUBLIC-DOMAIN.md' "$release" \
  || fail 'release package does not verify SQLite public-domain evidence'

if grep -Fq 'sqlite-storage-spike' "$lint" "$release" "$cargo_manifest"; then
  fail 'temporary SQLite spike remains in production configuration'
fi
if [ -e "$repo_root/collect-diff-context-cli/src/bin/sqlite_storage_spike.rs" ] \
  || [ -e "$repo_root/collect-diff-context-cli/tests/sqlite_storage_spike.rs" ]; then
  fail 'temporary SQLite spike source or tests remain'
fi

printf 'repository index workflow tests passed\n'
