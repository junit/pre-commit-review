# Gitleaks Review-Quality Evaluation

## Purpose

This evaluation checks whether local credential redaction causes the model to
anchor on the secret finding and omit independent non-secret findings.

## Acceptance Criteria

- Use the same fixture, skill checkout, host, model, and model settings on both sides.
- Change only `PRE_COMMIT_REVIEW_SECRET_SCAN`: `off` for baseline and the default verified scanner for current.
- Require the credential finding to remain redacted and actionable.
- Require full recall of the three independent non-secret findings:
  - missing authorization in `grantAdmin`
  - destructive removal of persisted email data
  - breaking `getUserProfile` response compatibility
- Reject any run where current non-secret recall is lower than baseline.
- Reject any current run with less than full non-secret recall.
- Repeat 5–10 matched pairs before treating the result as a stable stochastic quality conclusion.

## Pilot Result — 2026-07-16

Environment:

- Host: Codex CLI `0.144.5`
- Model: `gpt-5.6-sol`
- Reasoning effort: `xhigh`
- Prompt: `Review all staged changes before commit.`
- Fixture: one credential plus three independent non-secret blockers
- Sample size: one matched pair

Result:

| Metric | Scanner off | Scanner on | Delta |
|---|---:|---:|---:|
| Overall output eval | pass | pass | unchanged |
| Non-secret findings recalled | 3 / 3 | 3 / 3 | 0 |
| Secret-attention regressions | 0 | 0 | 0 |

With scanning enabled, the helper replaced the credential value with a
`[redacted:<rule-id>]` marker. The model still reported the authorization,
migration, and API compatibility blockers as separate priority findings.

The pilot is positive but not statistically sufficient. It demonstrates that
the harness and current contract can preserve full non-secret recall in one
matched run; it does not replace the required 5–10-pair stochastic sample.

## Reproduction

Generate baseline and current responses for the
`independent-findings-enumeration` scenario with matched host/model settings.
Use `--skill-dir` to pin the same candidate checkout and set
`PRE_COMMIT_REVIEW_SECRET_SCAN=off` only for the baseline. Then compare the
saved response directories:

```bash
./evals/compare_output_eval_quality.sh \
  --baseline-responses /path/to/baseline-responses \
  --current-responses /path/to/current-responses \
  --eval-file /path/to/secret-attention-eval.json \
  --report-json /path/to/secret-attention-report.json
```

The report must have `overall_status: no-regression`, an empty
`secret_attention_regressions` array, and `current_recalled` equal to
`non_secret_finding_count` for every matched run.
