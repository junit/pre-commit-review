# Review Verdict Rules

This file defines verdict selection rules for commit-readiness reviews.

It answers only these questions:

- What does each verdict mean?
- Which findings are blocking by default?
- Which findings are normally non-blocking?
- What consistency checks must pass before emitting the final review?

Rendering templates, evidence formatting, visual review matrices, coverage-led workflow, and grading compatibility rules are defined elsewhere.

## Verdict Tokens

Use exactly one top-level verdict token:

- `SAFE_TO_COMMIT`
- `SAFE_TO_COMMIT_WITH_NOTES`
- `DO_NOT_COMMIT`

Do not emit multiple verdict tokens in one review.

## Verdict Definitions

### SAFE_TO_COMMIT

Use `SAFE_TO_COMMIT` only when all of the following are true:

- no blocking issue was found within the reviewed scope
- no high-confidence correctness, security, privacy, auth, data, migration, runtime, build, release, or compatibility risk requires pre-commit action
- any remaining test gaps do not affect the commit decision because the change is low-risk or already has sufficient alternative validation
- there is no unreviewed material area that could reasonably change the verdict

This verdict means the developer can commit now without needing to read caveats to avoid breakage, leaks, or irreversible damage.

### SAFE_TO_COMMIT_WITH_NOTES

Use `SAFE_TO_COMMIT_WITH_NOTES` when all of the following are true:

- no blocking issue was found within the reviewed scope
- one or more non-blocking notes, moderate risks, suggested checks, maintainability concerns, review limitations, or test gaps are worth surfacing
- the remaining concerns do not require fixing before commit
- any partial-review limitation is clearly bounded and does not reasonably change the commit decision

This verdict means the developer can commit now, but should read the notes to understand residual risk, suggested validation, or follow-up work.

### DO_NOT_COMMIT

Use `DO_NOT_COMMIT` when any of the following are true:

- at least one blocking issue exists
- a high-confidence issue could cause build failure, runtime failure, data corruption, privilege escalation, authorization bypass, privacy leak, security vulnerability, irreversible migration harm, major compatibility breakage, or a release incident
- a high-risk or material area could not be reviewed and that gap could change the verdict
- a test gap covers high-risk logic and there is no sufficient alternative validation
- the review is coverage-led and coverage validation is materially incomplete

This verdict means the change is not safe to commit until the blocking issue is fixed or the blocking review gap is closed.

## Blocking Matrix

The following conditions are blocking by default unless there is strong, specific evidence that the actual impact is negligible.

| Category | Block when |
|---|---|
| Build | The change cannot compile, type-check, package, or initialize correctly |
| Runtime | A reachable code path can throw, crash, deadlock, leak resources, or mis-handle core control flow |
| Security | The diff introduces or exposes auth bypass, injection, XSS, SSRF, unsafe deserialization, secret leakage, weak trust-boundary validation, or other reachable vulnerabilities |
| Privacy | The diff can expose PII, log secrets or sensitive payloads, or allow unauthorized data access |
| Data | The diff can corrupt data, lose data, duplicate irreversible side effects, or apply schema changes incompatibly |
| Migration | The migration is irreversible, unsafe to roll out, unsafe to roll back, or incompatible with live code ordering |
| Compatibility | A public API, event, config, serialization shape, or downstream contract is broken without protection or migration handling |
| Release | The rollout depends on missing feature flags, wrong sequencing, missing config, broken rollback, or other unsafe release mechanics |
| Dependency | The change introduces an untrusted dependency, suspicious lockfile mismatch, supply-chain risk, or incompatible runtime change |
| Performance | A hot path gains an N+1 pattern, unbounded loop, unbounded memory growth, or other concrete regression likely to move a real metric |
| Testing | High-risk logic lacks tests and there is no sufficient manual or existing coverage to reduce the uncertainty |
| Review scope | A high-risk or material unit remains unreviewed and could change the final verdict |

## Normally Non-blocking Matrix

The following conditions are usually non-blocking unless compounded by high-risk context or direct evidence of impact.

| Category | Usually non-blocking when |
|---|---|
| Maintainability | Naming, local structure, comments, or cleanup could be improved but runtime behavior is not affected |
| Refactoring | The behavior appears unchanged and surrounding evidence supports that conclusion |
| Documentation | Docs, comments, or examples are incomplete but do not mislead the release or break users |
| Test suggestion | Additional tests would be helpful, but the path is low-risk or already sufficiently covered |
| Minor performance note | The performance concern is small, off the hot path, or speculative |
| Internal compatibility note | Internal callers changed together in the same diff and no external contract is broken |
| Visual/UI suggestion | Spacing, copy, or style issues do not materially harm usability or accessibility |
| Review limitation | The missing area is clearly low-risk or cosmetic and cannot reasonably change the verdict |

## How To Decide

Use this order:

1. Identify whether any blocking condition is directly supported by evidence.
2. If yes, choose `DO_NOT_COMMIT`.
3. If no blocker exists, check whether any note, test gap, or limitation is still worth surfacing.
4. If yes, choose `SAFE_TO_COMMIT_WITH_NOTES`.
5. If nothing important remains and the reviewed scope is materially clean, choose `SAFE_TO_COMMIT`.

## Special Cases

### New code with no callers yet

New code without active callers can still be blocking when the issue is intrinsic to the code itself, especially for:

- security-sensitive logic
- data handling
- migrations
- operational infrastructure
- obviously broken control flow

Style, readability, naming, or low-risk completeness issues in truly unused new code are usually non-blocking.

### Partial review

A review is not automatically partial just because broader repository context is unavailable.

Use partial-review wording only when:

- part of the diff itself could not be reviewed
- a binary, generated artifact, truncated output, or missing source prevented full inspection
- a material runtime state or visual state required for the review is unavailable

If the entire accessible diff was reviewed, lack of external context is a blind spot, not automatically a partial review.

### Coverage-led review

For coverage-led reviews:

- full-review wording is allowed only when coverage validation is empty
- any unreviewed material high-risk unit makes the verdict `DO_NOT_COMMIT`
- advisory fallback must not present sampled coverage as commit-safe coverage

## Output Quality Gate

Before emitting the final review, verify all of the following:

1. Verdict consistency
   - If any blocking issue exists, the verdict must be `DO_NOT_COMMIT`.
   - If no blocker exists but meaningful notes, test gaps, or bounded review limits remain, the verdict is usually `SAFE_TO_COMMIT_WITH_NOTES`.
   - `SAFE_TO_COMMIT` must not include actions that are required before commit.

2. Actionability
   - Every surfaced priority finding must identify a concrete object, failure mode, and actionable next step.

3. Evidence discipline
   - Do not promote speculation to a finding.
   - Low-confidence concerns belong in review limitations, suggested verification, or a clearly marked low-confidence note.

4. Scope honesty
   - Do not hide material unreviewed areas.
   - If a review gap could change the verdict, the verdict must reflect that.

5. Test-gap honesty
   - Do not wave away missing tests with generic prose.
   - State whether the missing validation affects the commit decision.

6. Risk calibration
   - Do not turn style or polish comments into blockers.
   - Do not downplay security, data, auth, migration, or release issues as routine notes.

## Final Check

Use this mental test before choosing the verdict:

- If the developer commits without reading the review, can something break, leak, corrupt data, bypass a boundary, or create an irreversible bad state?
  - If yes, use `DO_NOT_COMMIT`.
- If the answer is no, but the developer would still benefit from reading caveats, use `SAFE_TO_COMMIT_WITH_NOTES`.
- If even the caveats are unnecessary for safe commit behavior, use `SAFE_TO_COMMIT`.
