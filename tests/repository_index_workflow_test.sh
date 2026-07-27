#!/usr/bin/env bash
set -euo pipefail

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
repo_root="$(CDPATH='' cd -- "$script_dir/.." && pwd -P)"
lint="$repo_root/.github/workflows/lint.yml"
release="$repo_root/.github/workflows/release.yml"
cargo_manifest="$repo_root/collect-diff-context-cli/Cargo.toml"

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

grep -Fq 'cargo build --release --target ${{ matrix.target }} --bins' "$release" \
  || fail 'release workflow does not build the bundled product binaries'
grep -Fq 'index build --source staged' "$release" \
  || fail 'release workflow does not build a production repository index'
grep -Fq 'index doctor --cache-dir' "$release" \
  || fail 'release workflow does not doctor the production repository index'
grep -Fq 'index inspect --generation' "$release" \
  || fail 'release workflow does not run an immutable production query'
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
