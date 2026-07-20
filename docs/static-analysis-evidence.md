# Static Analysis Evidence Integration

`pre-commit-review` can ingest precomputed SARIF 2.1.0 or normalized JSON as an optional deterministic evidence lane. The integration never discovers reports automatically and never runs the analyzer that produced them.

## Workflow

Open the ordinary review control plane first:

```bash
scripts/collect_diff_context.sh --control-plane
```

Record its `source` and `scope_fingerprint`. Then collect one or more explicitly supplied reports:

```bash
scripts/collect_static_evidence.sh \
  --source staged \
  --expect-scope <scope_fingerprint> \
  --result /trusted/path/results.sarif \
  --result /trusted/path/typecheck.json
```

The collector reopens the control plane with `--expect-scope`, normalizes and deduplicates findings, maps result paths to manifest units, computes whether locations touch added lines, and revalidates the complete control plane before emitting evidence. A stale or mismatched report fails closed.

Static result files are explicit data inputs. Supplying a report does not authorize package scripts, build targets, analyzers, repository plugins, or remote rules to run.

## Normalized JSON Input

Normalized JSON uses `static_analysis_input/v1`, defined by `collect-diff-context-cli/schemas/static-analysis-input.schema.json`:

```json
{
  "schema_version": 1,
  "kind": "static_analysis_input",
  "scope_fingerprint": "<40-or-64-character-fingerprint>",
  "tool": {"name": "type-checker", "version": "1.2.3"},
  "status": "completed",
  "findings": [
    {
      "rule_id": "TYPE-1001",
      "message": "Returned value is incompatible with the declared type.",
      "path": "src/service.ts",
      "start_line": 42,
      "end_line": 42,
      "severity": "error",
      "category": "build",
      "confidence": "high",
      "baseline_state": "new"
    }
  ]
}
```

Supported statuses are `completed`, `failed`, `timeout`, and `unavailable`. Failed or incomplete report evidence cannot become a blocking candidate by itself.

## SARIF Scope Binding

SARIF 2.1.0 can embed the review fingerprint in each run:

```json
{
  "version": "2.1.0",
  "runs": [
    {
      "properties": {
        "preCommitReviewScopeFingerprint": "<scope_fingerprint>"
      },
      "tool": {"driver": {"name": "scanner"}},
      "results": []
    }
  ]
}
```

For a raw SARIF report that cannot embed custom properties, `--result-scope <scope_fingerprint>` records an explicit assertion. Use that option only when the user or trusted CI context confirms the report was produced from the exact opening snapshot. An embedded mismatch cannot be overridden.

## Evidence Output

The collector emits one `static_analysis_evidence/v1` object, defined by `static-analysis-evidence.schema.json`. Each finding includes:

- stable finding and report identifiers;
- tool and rule identity;
- normalized severity, category, confidence, and baseline state;
- manifest unit and line-scope mapping;
- one reducer disposition: `blocking-candidate`, `priority-candidate`, `note`, or `outside-scope`.

`blocking-candidate` is deliberately not an automatic verdict. The review must still verify the execution point, reachability, impact, and visible mitigations. Static evidence does not mark any manifest unit reviewed.

Validate an emitted artifact with:

```bash
python3 scripts/validate_schemas.py \
  --static-evidence-output /path/to/static-evidence.out
```

## Bounds and Safety

- Python 3 is required only for this optional evidence lane.
- Input is limited to 10 MB per file by default; override with `PRE_COMMIT_REVIEW_STATIC_MAX_INPUT_BYTES`.
- At most 10,000 input findings are processed and 500 are emitted by default; `--max-findings` accepts 1 to 5,000.
- Blocking and priority candidates are emitted before notes and outside-scope results. A truncated result must be expanded before claiming complete static-evidence review when material candidates remain undisposed.
- External Git diff and textconv drivers are disabled during changed-line mapping.
- Output includes bounded messages but no raw source snippets.
- The wrapper applies the existing optional local Gitleaks sanitizer to machine-readable output when available.
- No network request, analyzer execution, repository mutation, or report auto-discovery occurs.
