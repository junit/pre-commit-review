# Contributing to pre-commit-review

Contributions are welcome and best focused on:

- improving review heuristics
- tightening safety boundaries
- refining the output template
- making diff collection more robust across repository states

## Documentation and Layout Discipline

- If you change script paths or repository layout, update `SKILL.md` accordingly.
- If you update user-facing documentation, keep the localized README files (`README.md` and `README.zh-CN.md`) synchronized.
- `README.md` and `README.zh-CN.md` are **contract files**: several tests assert that specific phrases still appear in them. When editing README, keep the exact strings checked by `tests/skill_contract_test.sh` and `evals/readme_surface_test.sh` intact (or update both the README and the contract assertions together in the same change).

## Development

Shell scripts (`scripts/*.sh`, `install.sh`, `tests/*.sh`, `evals/*.sh`) are linted by [shellcheck](https://www.shellcheck.net/) in CI (`.github/workflows/lint.yml`). Install it locally (`brew install shellcheck` on macOS) and run `shellcheck -s bash scripts/*.sh install.sh tests/*.sh evals/*.sh` before submitting changes.

To build the Rust CLI binary locally for the current host, run `cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml`. To refresh bundled release binaries, run `scripts/build_with_docker.sh`, which delegates to `scripts/build_all_binaries.sh` and uses native macOS targets plus Docker/cross compilation for Linux and Windows targets when needed.

## Tests

The deterministic unit test suite is `bash tests/*_test.sh`. The eval harness also ships deterministic self-tests that do not call a model: `bash evals/eval_contract_test.sh`, `bash evals/output_eval_runner_test.sh`, and `bash evals/output_eval_host_wrappers_test.sh` (or run all eval self-tests via `for f in evals/*_test.sh; do bash "$f"; done`). The model-backed runners (`evals/output_eval_codex_runner.sh`, `evals/output_eval_claude_runner.sh`) require a real Codex or Claude CLI and are not part of CI.

The manual real-host smoke workflow is `.github/workflows/real-host-smoke.yml`. It is intended for a self-hosted runner that already has authenticated `claude` and `codex` CLIs available, and it delegates to `evals/run_real_host_smoke.sh`.

## Pull Requests

Before opening a PR:

1. Run `shellcheck -s bash scripts/*.sh install.sh tests/*.sh evals/*.sh` — it should be clean.
2. Run the deterministic suites: `bash tests/*_test.sh` and `for f in evals/*_test.sh; do bash "$f"; done`.
3. If you touched `README.md` or `README.zh-CN.md`, confirm `bash tests/skill_contract_test.sh` and `bash evals/readme_surface_test.sh` still pass.
4. Keep `README.md` and `README.zh-CN.md` aligned if your change is user-facing.
