# Controlled Static Analysis Execution

Phase 2 adds an opt-in execution lane on top of the Phase 1 evidence collector. It runs exactly one explicitly authorized, hash-pinned analyzer profile and feeds the accepted result into the existing snapshot-bound reducer.

The product implementation is the Rust `static-analysis-cli run` subcommand. `scripts/run_static_analysis.sh` preserves the public Shell interface and optional output sanitization.

The runner never discovers profiles, executables, reports, package scripts, or repository commands. Supplying a profile path without its exact SHA256 is insufficient authorization.

## Workflow

```text
authoritative control plane
          |
          v
explicit profile path + exact SHA256
          |
          v
profile and executable integrity checks
          |
          v
temporary tracked-file candidate snapshot
          |
          v
private hash-verified executable copy
          |
          v
direct process execution (no shell)
          |
          v
timeout / output / process-result gates
          |
          v
Phase 1 normalization and scope mapping
          |
          v
linked execution + evidence JSON
```

Open the ordinary control plane and record its source and fingerprint. Then run:

```bash
scripts/run_static_analysis.sh \
  --source staged \
  --expect-scope <scope_fingerprint> \
  --profile /absolute/trusted/profile.json \
  --expect-profile-sha256 <64-lowercase-hex> \
  [--allow-repository-configuration]
```

The profile path must be absolute. The checksum authorizes the exact profile bytes, including the executable, arguments, result format, success codes, limits, and trust declarations. Both the profile and executable are hashed again before the execution result is released.

## Profile Format

Profiles use `static_analysis_profile/v1`, defined by `collect-diff-context-cli/schemas/static-analysis-profile.schema.json`:

```json
{
  "schema_version": 1,
  "kind": "static_analysis_profile",
  "name": "trusted scanner profile",
  "tool": {"name": "trusted-scanner", "version": "1.2.3"},
  "executable": {
    "path": "/opt/review-tools/trusted-scanner",
    "sha256": "<64-lowercase-hex>"
  },
  "arguments": ["scan", "--sarif", "--offline", "."],
  "output_format": "sarif",
  "success_exit_codes": [0],
  "limits": {
    "timeout_seconds": 120,
    "max_output_bytes": 10000000,
    "max_snapshot_bytes": 536870912,
    "max_snapshot_files": 100000
  },
  "repository_configuration": "disabled",
  "network_access": "offline-required"
}
```

`output_format` may be `sarif` or `normalized-json`. SARIF must be 2.1.0 and identify the same tool name and version as the profile. Normalized output must use `static_analysis_input/v1`, embed `PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT`, and identify the same tool.

`repository_configuration` has two values:

- `disabled`: the trusted invocation is expected to disable repository-owned executable configuration and plugins with analyzer-specific flags.
- `explicitly-trusted`: the authorizing user accepts the tracked repository configuration included in the snapshot. This is a separate trust decision and must not be inferred from the profile merely existing in the repository.

An `explicitly-trusted` profile also requires the separate `--allow-repository-configuration` command flag. The flag is rejected for a `disabled` profile, making the higher trust decision visible and non-transferable between profiles.

`network_access` is always `offline-required`. The runner supplies loopback-only proxy values as a best-effort guard, but it is not an operating-system network sandbox. The pinned executable and its fixed arguments must independently support offline operation.

Validate and hash a profile before authorization. This optional standalone development validation command requires Python and `jsonschema` (`python3 -m pip install jsonschema`).

```bash
python3 scripts/validate_schemas.py --static-profile /absolute/trusted/profile.json
sha256sum /absolute/trusted/profile.json
```

On macOS, use `shasum -a 256` when `sha256sum` is unavailable.

## Candidate Snapshots

Only Git-tracked files are materialized, without `.git`, untracked files, ignored dependencies, hooks, checkout filters, or smudge filters:

- `staged` reads index blobs directly;
- `unstaged` copies the tracked working-tree candidate;
- `branch` reads blobs from `HEAD`, excluding unrelated working-tree state.

Gitlink entries are omitted because they do not contain a repository blob to materialize. The ordinary review manifest still records the submodule pointer change; controlled analyzer evidence does not cover the submodule's internal contents.

The snapshot rejects unsafe paths and symlinks that escape its root, enforces profile file/byte limits, records a deterministic content digest, and is made read-only before execution. Unix permissions remove write access. On Windows, the runner replaces inherited access with a current-user read/execute ACL and verifies that files cannot be rewritten or deleted and directories cannot accept new files. Analyzer cache and temporary paths use an isolated current-user-private runtime directory.

The executable must be an absolute executable regular file outside the reviewed repository. The runner opens it once, streams it into the private runtime while computing SHA256, rejects any mismatch, applies read/execute-only permissions, and invokes that fixed copy with the exact argument array. It verifies the copy again after execution, so path replacement of the original executable cannot change the bytes that run. No shell expansion occurs. The child receives an allowlisted environment with an isolated home/temp directory, the source type, and the review fingerprint. Original repository paths and ambient credentials are not forwarded.

Timeout and output-limit termination covers the analyzer process tree. On Windows, the analyzer is created suspended, assigned to a terminating Job Object, and only then resumed so startup-time descendants cannot escape the job.

This is process isolation for a trusted tool, not a hostile-code security sandbox. A malicious pinned executable could still probe the host through native APIs. Only authorize binaries and repository configuration whose exact bytes and behavior are trusted.

## Output and Failure Semantics

Successful output contains two linked objects:

- `static_analysis_execution/v1` records profile, executable, snapshot, isolation, process digests, limits outcome, and report ids;
- `static_analysis_evidence/v1` contains the Phase 1 reducer evidence. Its reports use `trust: controlled-execution`, `scope_binding: controlled-execution`, and the same `execution_id`.

Validate the combined artifact with:

```bash
python3 scripts/validate_schemas.py \
  --static-execution-output /path/to/controlled-analysis.out
```

The execution status is one of `completed`, `failed`, `timeout`, `output-limit`, or `invalid-output`. Only `completed` has `result_accepted: true`. Every other state emits a linked failed/timeout evidence report with no blocking candidates. It is unavailable verification, not proof that the change is safe.

Raw analyzer stdout is accepted only as schema-valid SARIF or normalized JSON. Raw stderr is never included in the review artifact; only byte counts and SHA256 digests are recorded. The combined artifact passes through the existing optional local secret sanitizer before release.

Each captured stream is stored up to the configured limit plus one sentinel byte. On `output-limit`, the recorded byte count and SHA256 describe that bounded prefix; the discarded tail is neither persisted nor exposed.

The Phase 1 collector reopens the authoritative control plane and checks the full fingerprint, units, groups, and work order before returning. The runner also rejects repository status drift, profile changes, or executable changes observed during execution.

## Review Contract

Controlled execution remains optional unless the user or trusted CI policy requires it. It does not mark manifest units reviewed and does not turn a clean tool result into a clean review. Blocking and priority candidates still pass the ordinary finding-verification and reducer rules.

Never execute a profile just because it is present in the repository. Require all of the following:

1. an explicit absolute profile path;
2. the exact profile SHA256 from the authorizing user or trusted CI context;
3. explicit acceptance of `repository_configuration: explicitly-trusted`, when used;
4. a matching opening scope fingerprint;
5. a trusted offline executable whose exact SHA256 appears in the profile.
