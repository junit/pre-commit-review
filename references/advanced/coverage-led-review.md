# Coverage-Led Review

This file defines the coverage-led workflow used for large, truncated, grouped, generated-heavy, or otherwise non-trivial commit-readiness reviews.

It answers only these questions:

- When must a review become coverage-led?
- What is the authoritative unit of coverage?
- How should review work be split, tracked, and reduced?
- When does an unreviewed gap become verdict-blocking?
- What parts of the internal coverage process must be surfaced to the developer?

Verdict selection, finding taxonomy, rendering templates, and grading-compatibility wording are defined elsewhere.

## Purpose

Coverage-led review exists to prevent false confidence in large or fragmented reviews.

A normal review can rely on direct inspection of the diff as a whole.
A coverage-led review must additionally prove that all material review units were either:

- fully reviewed, or
- explicitly surfaced as unreviewed review limits with verdict consequences

The single source of truth is coverage accounting over the review units, not the narrative confidence of the reviewer.

## When To Use Coverage-Led Review

Use coverage-led review when any of the following is true:

- the diff is large enough that direct end-to-end inspection is no longer reliable
- the diff is truncated
- the helper emits a review manifest, review groups, or review plan
- one or more review groups are marked as split-required
- the review spans generated, vendored, minified, or snapshot-heavy artifacts that require source-to-output consistency checks
- review work must be delegated, resumed, or reduced across multiple steps
- state must survive across multiple review passes

Do not use coverage-led structure for small routine reviews where the entire diff can be honestly inspected in one pass.

## Core Contract

Coverage-led review requires all of the following:

- an authoritative list of review units
- a coverage ledger that tracks each unit exactly once
- explicit reconciliation between reviewed units and manifest units
- explicit handling of split-required units
- explicit surfacing of remaining material gaps

A coverage-led review may call itself a full review only when coverage validation is empty.

## Authoritative Inputs

Use these inputs in descending authority:

1. `Review Plan JSON`
2. `Review Manifest JSONL`
3. `Review Groups JSONL`
4. human-readable `Review Manifest`
5. human-readable `Review Groups`
6. `Split Suggestions`
7. `Dependency Summary`
8. `Semantic Context Queries`

Rules:

- treat `Review Manifest` as the authoritative list of review units
- treat `Review Groups` as the default work plan, not the ground truth of coverage
- treat `Split Suggestions` as replacement planning for oversized units
- treat `Dependency Summary` and `Semantic Context Queries` as best-effort context only
- never let contextual hints mark a unit as reviewed

## Review Units

A review unit is the smallest material item that can be honestly marked as reviewed.

Typical units include:

- one file
- one hunk of a large file
- one generated artifact paired with its generating source/config
- one grouped low-risk consistency bucket, but only when the group is within budget and materially reviewable as a group

If a file is too large to fit honestly into one review pass, split it into smaller hunk-level units before claiming coverage.

## Coverage Ledger

The coverage ledger is the working record of review progress.

Every manifest unit must end in exactly one of these states:

- pending
- reviewed
- needs-split
- replaced-by-split-units
- unreviewable-with-limit

Rules:

- do not mark a unit reviewed unless it was actually inspected
- do not leave a material unit untracked
- do not keep a stale parent unit after replacing it with split units
- do not treat "probably low risk" as a substitute for coverage accounting

## Work Order

Use this order by default:

1. split oversized or split-required groups first
2. review high-risk groups and files
3. review consistency groups
4. review medium-risk groups
5. review generated or lockfile-heavy groups with source-consistency checks
6. run final reduction only after coverage validation

Reorder only when dependency evidence makes another order safer or more efficient.

## Splitting Rules

When a group or file is too large for honest inspection:

- do not mark the parent group reviewed
- replace it with smaller units before continuing
- prefer the provided `Split Suggestions`
- if no split suggestion exists, split by file or by hunk boundary
- preserve traceability from parent unit to replacement units

Use hunk-level review when:

- the file is too large for one pass
- only certain hunks are high-risk
- the helper provides split previews or file-specific context commands

## Context Retrieval

Use bounded, source-stable context retrieval.

Rules:

- prefer helper-provided group-level `context_command` when the group fits in budget
- use file-level commands when a group must be narrowed or split
- every retrieval command must stay tied to the same diff source
- do not widen review scope accidentally when retrieving more context
- semantic context can inform impact analysis, but cannot satisfy coverage

The goal of context retrieval is to inspect a missing unit, not to gather more ambient confidence.

## Generated, Vendored, and Snapshot-Heavy Changes

Generated, vendored, minified, and snapshot-heavy changes still require coverage handling.

Review them differently, not less rigorously.

For these units:

- verify whether generating source or config also changed
- check whether the output is explainable from the source change
- check whether the artifact appears reproducible
- check for suspicious drift, unexplained version jumps, or mismatched lock/source behavior
- surface unexplained generated output as a review limit or finding

For large generated updates, the review should explicitly note that the method was `coverage-led` and whether the output appears `reproducible`.

## Group Review Result

Each reviewed group or split unit should produce a compact result that records:

- the group or unit identifier
- which required units were actually reviewed
- whether coverage for that group/unit is full or partial
- findings discovered
- contract changes to revisit
- dependencies that need follow-up checks
- recommended tests or manual verification

The result should be reducer-friendly and compact.
Do not use a narrative paragraph as the only durable record of group review progress.

## Reducer State

For long reviews, maintain a compact reducer state containing:

- reviewed units
- pending units
- split-needed units
- group results
- coverage gaps
- merged findings
- dependency follow-ups
- recommended tests

Rules:

- update reducer state after every group result
- reconcile reducer state against the authoritative manifest before each reduction pass
- if a unit is missing from reducer state or appears that is not in the manifest, treat coverage validation as failed until corrected

Do not store reducer state in the repository unless the user explicitly asks for an artifact.

## Coverage Validation

Coverage validation is the required precondition for final synthesis.

Compute:

- `manifest_units - reviewed_units`

Interpretation:

- if the result is empty, coverage is complete
- if the result contains only bounded, clearly non-material units, the review may still be partial but non-blocking
- if the result contains any material or high-risk unit, commit-readiness is blocked

Also verify:

- every `needs-split` unit has replacement results
- no reviewed unit is stale or double-counted
- no parent unit remains marked reviewed after being replaced by split children

## Dependency and Contract Follow-Up

Coverage completion alone is not enough.

After coverage validation and before final verdict:

- inspect contract changes surfaced by group results
- inspect dependency follow-up items
- re-check changed imports, exports, shared types, schemas, config ordering, and migration sequencing where relevant
- verify that the final findings reflect cross-file impact, not just isolated hunk observations
- apply the finding verification gate to high-impact, negative, absolute, security/auth/privacy/data, framework/library behavior, and delegated/reducer findings before reporting them

Do not perform cross-file reduction before coverage validation.

## Review Limits

A review limit is the user-visible representation of an actual unreviewed gap.

Every material gap must be surfaced with:

- scope
- reason
- potential hidden risk
- what would close the gap
- whether it affects the verdict

Rules:

- do not invent a review limit for a unit already marked reviewed
- do not omit a material unreviewed unit from user-visible output
- do not hide a blocking coverage gap under soft wording
- do not enter Tiny mode if any material review limit exists

## Verdict Interaction

Coverage-led workflow affects verdict selection in these ways:

- full-review wording is allowed only when coverage validation is empty
- any unreviewed high-risk or material unit makes commit-readiness `DO_NOT_COMMIT`
- bounded non-material limits can still permit `SAFE_TO_COMMIT_WITH_NOTES`
- advisory fallback must never present sampled or incomplete coverage as fully commit-safe

Coverage-led review does not replace the main verdict rules; it constrains when a full, commit-safe verdict is even eligible.

## What The Developer Must See

The final user-facing report does not need to expose every internal reducer detail.

But for any meaningful coverage-led review, the final output should make the following visible in concise form:

- what the review units or review scope were
- whether any splitting was required
- whether coverage validation completed cleanly
- which material areas remain unreviewed, if any
- whether generated or snapshot-heavy outputs were checked for source consistency
- which follow-up tests or checks are recommended before commit

Keep the report decision-oriented.
Do not dump raw reducer state unless the user explicitly asks for process detail.

## Minimal User-Facing Coverage-Led Summary

When coverage-led review is used, the final report should include enough information to show process rigor without turning the whole output into an internal protocol dump.

A concise summary should usually answer:

- Was this a coverage-led review?
- What units or groups were reviewed?
- Was any unit split?
- Did coverage validation finish cleanly?
- Are any material units still unreviewed?
- Does that gap affect the verdict?

## Advisory Fallback

Use advisory fallback only when:

- repository/helper access is unavailable, or
- the user explicitly wants fast bounded triage, or
- the user declines to continue a required coverage-led review

In advisory fallback:

- name exactly what was reviewed
- name exactly what was not reviewed
- do not present the result as full commit-readiness
- explain what additional review would be needed to upgrade the result into a coverage-led commit decision

## Failure Modes To Avoid

Do not do any of the following:

- sample a large diff and still call it a full review
- mark a split-required group as reviewed without replacement units
- use semantic context as a substitute for reviewing manifest units
- collapse a material coverage gap into a vague "under-verified" phrase
- treat generated or snapshot-heavy changes as exempt from coverage
- claim full review when the diff itself was truncated or materially inaccessible
- emit a confident verdict before coverage validation

## Final Checklist

Before producing the final review, verify:

1. the authoritative manifest is known
2. every manifest unit is accounted for
3. every split-required unit has replacement coverage
4. coverage validation has been computed
5. material gaps are surfaced visibly
6. cross-file dependency or contract follow-up has been completed
7. high-impact reducer findings passed the finding verification gate or were downgraded
8. the final verdict matches the actual coverage state
