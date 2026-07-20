# Static Analysis Evidence

Use this reference only when the user explicitly supplies a SARIF or normalized static-analysis report for the commit candidate.

## Purpose

Static analysis is an optional deterministic evidence lane. It supplements diff reasoning; it does not replace manifest coverage, finding verification, focused tests, or the final control-plane refresh.

Never auto-discover reports, execute analyzer commands, load repository-provided plugins, or infer that a report belongs to the selected diff from its file location. An explicit result path is data input, not authority to execute the tool that produced it.

## Collection Workflow

1. Open the normal authoritative control plane and record its `scope_fingerprint` and selected source.
2. Accept only result paths explicitly supplied by the user or trusted CI context.
3. Resolve `scripts/collect_static_evidence.sh` relative to the skill package containing `SKILL.md`.
4. Run:

   ```bash
   scripts/collect_static_evidence.sh \
     --source <staged|unstaged|branch> \
     --expect-scope <scope_fingerprint> \
     --result <report.sarif-or-json>
   ```

5. A normalized JSON report must use `static_analysis_input/v1` and embed the same `scope_fingerprint`.
6. SARIF 2.1.0 may embed the fingerprint in `runs[].properties.preCommitReviewScopeFingerprint`. If it does not, use `--result-scope <scope_fingerprint>` only when the user or trusted CI context explicitly confirms that the report was produced from that exact snapshot.
7. Treat collector failure, missing Python, malformed input, an invalid schema, or a scope mismatch as unavailable static evidence. Continue the ordinary review unless the user explicitly required that evidence or the missing result leaves a material high-risk area unverified.
8. If evidence reports `truncated: true`, rerun with a higher bounded `--max-findings` value. Do not claim complete static-evidence review while material candidates remain hidden by truncation.
9. Before final synthesis, rerun the normal control plane. Its fingerprint, units, groups, and work order must still match both the opening scope and the emitted static evidence.

The collector is read-only, bounded, and does not run the analyzer. It maps findings only to manifest units in the authoritative scope and computes added-line membership from Git diff bytes with external diff and textconv drivers disabled.

## Evidence States

Every normalized finding has one disposition:

- `blocking-candidate`: completed, explicitly supplied tool evidence maps a high-confidence critical/error security, privacy, build, correctness, data, compatibility, or reliability finding to an added line. It is a strong hypothesis, not an automatic final blocker.
- `priority-candidate`: material tool evidence needs execution-point or impact verification before severity and verdict selection.
- `note`: historical, unchanged, unbaselined, maintainability-only, low-confidence, or failed-report evidence that cannot block by itself.
- `outside-scope`: the result does not map to a manifest unit in the selected commit candidate and cannot affect the verdict by itself.

An added-line match establishes that the referenced line is new in this diff, so its normalized baseline becomes `new`. A finding on an unchanged line remains `existing` or `unknown` unless a trusted analyzer baseline says it is new.

## Reducer Integration

Add every static finding to the same candidate disposition ledger used for model findings. Preserve these fields through reduction:

- `finding_id` and `report_ids`;
- tool name and version;
- rule id, file, line, and manifest unit;
- category, severity, confidence, baseline state, and line scope;
- initial static disposition;
- final report location or reason it was disproven.

When a static finding and a model finding share the same affected object, trigger, failure mode, root cause, and corrective action, merge them into one finding and cite both evidence sources. Do not merge findings merely because they share a category or file.

For manifest-based reviews, attach a mapped finding to its owning review unit or group result before cross-file reduction. Static evidence never marks a unit reviewed: the diff content must still be inspected or provenance-verified.

## Verdict Interaction

- Independently verify every `blocking-candidate` under `finding-verification.md` before treating it as blocking.
- A confirmed build/type failure or reachable material security/correctness failure introduced on an added line normally forces `DO_NOT_COMMIT` under the main verdict rules.
- A false positive, unreachable path, suppressed rule with a verified reason, or mismapped location must be downgraded or rejected visibly.
- `priority-candidate` findings must appear as a verified priority finding, suggested verification, review limitation, or explicit rejection.
- `note` and `outside-scope` findings cannot force a blocking verdict by themselves.
- Tool success is evidence only for the rules and scope actually reported. It is never proof that the change has no other defects.

## Safety and Privacy

The static evidence wrapper applies the same optional local sanitizer used by the diff gateway when available. Never reconstruct a redacted value. If redaction is unavailable or disabled, do not claim the evidence output was protected from secret exposure.

Do not include raw source snippets from SARIF in normalized evidence. Keep messages bounded. Do not auto-run package scripts, build targets, analyzers, repository plugins, or remote rule downloads as part of this phase.

## Final Checklist

Before the verdict:

1. static evidence is bound to the opening fingerprint;
2. the report source was explicitly supplied;
3. every blocking or priority candidate has a visible final disposition;
4. no unchanged, unbaselined, failed-report, or outside-scope finding blocks by itself;
5. static findings were deduplicated with model findings only when root cause and fix match;
6. manifest coverage was completed independently of static evidence;
7. the final authoritative fingerprint still matches the evidence scope.
8. evidence is not truncated across undisposed material candidates.
