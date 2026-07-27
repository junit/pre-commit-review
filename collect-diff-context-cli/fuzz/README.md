# Structural and Repository Index Fuzzing

CI compiles all fuzz targets with the pinned corpus. Run sustained nightly jobs with:

```bash
rtk cargo +nightly fuzz run tree_sitter_rust --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
rtk cargo +nightly fuzz run impact_contract --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
rtk cargo +nightly fuzz run file_facts_decode --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
rtk cargo +nightly fuzz run repository_graph_row --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
rtk cargo +nightly fuzz run repository_overlay --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
rtk cargo +nightly fuzz run repository_traversal --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=3600
```

Minimize reproducible crashes and commit them under `fuzz/corpus/<target>/` as permanent regression seeds. Do not commit transient files from `fuzz/artifacts/`.
