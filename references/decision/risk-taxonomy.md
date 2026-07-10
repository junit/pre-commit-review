# Review Risk Taxonomy

This file defines the taxonomy used for priority findings in commit-readiness reviews.

It answers only these questions:

- Which marker should a finding use?
- How are tally counts calculated?
- What fields must each finding contain?
- What counts as acceptable evidence?

Verdict selection rules, rendering templates, visual review layout, coverage-led workflow, and grading compatibility are defined elsewhere.

## Contents

- Finding Markers
- Marker Selection Rule
- Tally Rules
- Required Finding Structure
- Evidence Rules
- Evidence Discipline
- Scope and Confidence Interaction
- Performance and Release Escalation
- Final Selection Checklist

## Finding Markers

Each priority finding must use exactly one primary marker.

### `🔒` Security and Privacy

Use for findings involving:

- credentials, keys, tokens, or secrets
- auth or authorization gaps
- injection or code-execution vectors
- trust-boundary validation failures
- PII exposure
- private host or infrastructure leakage
- supply-chain security concerns

Default interpretation: usually blocking unless clearly non-live, unreachable, or obviously harmless.

### `❌` Correctness and Runtime

Use for findings involving:

- logic bugs
- broken control flow
- compile or type failures
- runtime exceptions
- schema incompatibility
- broken initialization
- incorrect API behavior
- data corruption through code behavior

Default interpretation: usually blocking when the affected path is reachable or core to the reviewed change.

### `⚠️` Non-blocking Risk and Maintainability

Use for findings involving:

- maintainability issues
- naming or structure problems
- weak documentation
- low-risk edge cases
- non-critical convention drift
- moderate but non-blocking compatibility or rollout notes

Default interpretation: non-blocking unless stronger evidence promotes it into a blocker category.

### `🧪` Test Gap and Verification Need

Use for findings involving:

- missing automated coverage
- missing contract tests
- missing negative-path validation
- missing migration verification
- missing manual validation for risky paths

Blocking status depends on the underlying change risk, not on the marker alone.

### `👁️` Review Limitation and Scope Gap

Use for findings involving:

- truncated diffs
- unreadable binary or generated assets
- missing screenshots
- unavailable runtime states
- unavailable supporting source
- any material area that could not actually be inspected

Blocking status depends on whether the gap could change the verdict.

### `📈` Performance and Scalability

Use for findings involving:

- N+1 queries
- synchronous waits on hot paths
- large payload inefficiency
- unbounded loops
- unbounded memory growth
- token or API cost expansion
- missing batching or indexing
- concrete cost regressions

Blocking status depends on whether the impact is real, material, and likely on an important path.

### `🧭` Release, Migration, and Operations

Use for findings involving:

- rollout-order hazards
- feature-flag gaps
- environment or config mismatches
- migration sequencing risks
- rollback hazards
- deployment safety issues
- operational diagnosability gaps tied to the release itself

Blocking status depends on the severity of the rollout or operational risk.

## Marker Selection Rule

Choose the marker that best represents the primary reason the developer should care.

If a finding spans multiple dimensions:

- pick one primary marker
- explain the other dimensions in the title, impact, or fix
- do not attach multiple markers to one finding

## Tally Rules

Tally counts are based on the primary finding type, not every secondary concern mentioned in the finding text.

### Blockers

Count as blockers:

- all `🔒` findings that are judged blocking
- all `❌` findings that are judged blocking
- any `🧪`, `👁️`, `📈`, or `🧭` finding that is judged blocking

### Non-blocking warnings

Count as warnings:

- all `⚠️` findings
- any non-blocking `📈` finding
- any non-blocking `🧭` finding
- any non-blocking `👁️` finding

### Test gaps

Count as test gaps:

- every `🧪` finding, whether blocking or non-blocking
- any material missing-test or missing-coverage concern surfaced outside priority findings, when it covers changed behavior that is security-sensitive, data-affecting, compatibility-sensitive, operationally important, or otherwise central to the commit decision

### Review limits

Count as review limits:

- every `👁️` finding, whether blocking or non-blocking

### Tally consistency

The top-level tally, priority findings, commit guidance, and risk summary must describe the same risk set.

Rules:

- If the report recommends adding a missing test, missing negative-path assertion, missing integration check, or missing contract test for material changed behavior, the tally must include at least one test gap unless the same item is explicitly classified as only routine post-commit hardening with no commit-decision value.
- If suggested verification only says to run existing tests, perform a normal smoke check, or confirm deployment environment prerequisites, do not count that alone as a test gap.
- If the tally includes zero test gaps, do not describe any material changed path as lacking meaningful test coverage elsewhere in the report.
- If the risk summary says test coverage is sufficient, its basis must not simultaneously call out missing material tests or under-verified high-risk behavior. Use `Gaps`, `Not run`, `有缺口`, or `未运行` instead.

## Required Finding Structure

Each priority finding must include the following, unless the template or mode explicitly says otherwise.

### 1. Object reference

Use a concrete location whenever possible, such as:

- `` `file:line` ``
- `` `file:line-line` ``
- `` `file:function` ``
- `` `file:config_key` ``
- `` `file` `` when no more precise reference is available

### 2. Issue title

State the actionable issue, not just a topic label.

Good titles describe:

- error mechanism
- affected object
- trigger condition

Avoid vague titles such as:

- potential problem
- attention needed
- risk exists

### 3. Evidence

Provide the shortest direct evidence that supports the finding.

Good evidence sources include:

- diff hunks
- nearby code context
- schema or migration files
- tests or snapshots
- screenshots
- logs or CI output
- visible contract changes

Do not paste large blocks when a short excerpt or precise description is enough.

### 4. Impact

Describe:

- who or what is affected
- when the issue triggers
- how it fails
- the worst consequence

State whether the impact is:

- user-visible
- data-related
- production-related
- contract-related
- operationally significant

### 5. Fix

Provide a concrete remediation direction.

A good fix tells the developer:

- what code path to change
- what boundary condition to handle
- what safer or preferred approach to use

### 6. Verification

Provide a minimal verification loop, such as:

- a named test
- a specific command
- a manual path
- a contract check
- a migration rehearsal
- a monitoring check after rollout

Generic advice like "test more" is not acceptable.

### 7. Confidence

Use one of:

- High
- Medium
- Low

Guidance:

- `High`: directly visible in the diff or strongly proven by adjacent evidence
- `Medium`: plausible and supported, but depends on unseen caller/runtime/domain context
- `Low`: possible concern, but not strong enough for a normal priority finding unless clearly marked

Low-confidence suspicions should usually move to review limitations or suggested verification rather than appear as a strong priority finding.

High confidence for a strong claim requires the applicable verification basis:

- direct diff evidence for local behavior
- checked `file:line` or symbol evidence for cited locations
- cross-file trace for caller/callee, auth, data-flow, or contract-impact claims
- adequate search coverage for negative or exhaustive claims
- versioned source, official documentation, or a focused test for framework/library behavior

If that basis is missing, lower the confidence, narrow the claim, or move it to review limitations or suggested verification.

### 8. Blocking reason

Include this field only for blocking findings.

It must explain why the issue must be fixed before commit, not just repeat the title.

## Evidence Rules

Always prefer direct evidence from the reviewed diff.

Use surrounding context only when it materially helps determine impact or correctness.

For priority findings, blocking review limits, delegated/reducer findings, security/auth/privacy/data claims, negative or absolute claims, or framework/library behavior claims, apply `references/decision/finding-verification.md` before finalizing the finding.

Acceptable supplementary evidence includes:

- upstream callers
- downstream callees
- changed imports or exports
- shared types or schemas
- migration ordering
- config files
- lockfiles
- generated assets
- tests and snapshots
- runtime logs
- CI output
- screenshots or visual states
- documented project conventions visible in nearby code

## Evidence Discipline

Do not output a priority finding when:

- the issue is only a vague suspicion
- the supporting evidence is absent
- the concern depends entirely on unknown business rules
- the problem could equally plausibly be intentional and harmless
- the concern is only a clean-code smell with no demonstrated behavior, contract, release, performance, security, data, or testing impact

Do not treat "priority finding" as a synonym for "blocker." Non-blocking issues with concrete runtime, security, data, compatibility, operational, or testing impact can still be priority findings. Use the verdict and the optional blocking-reason line to communicate whether the issue blocks commit.

Do not use the clean-code-smell exclusion for verified boundary or contract failures. A concern remains a priority-threshold candidate when the evidence shows:

- an empty, null, missing, stale, malformed, or environment-specific boundary value can create a broken externally observable value, operation target, artifact reference, or persisted state
- an ignored caller intent, validation, mode, guard, or safety input can bypass side-effect protection, access control, isolation, preview/simulation behavior, rollback, or retention semantics
- a safety helper leaves a reachable TOCTOU gap such as DNS rebinding, redirect target drift, or post-validation connection drift
- a state, data, artifact, or configuration operation can silently invalidate live references, derived behavior, downstream consumers, or future recomputation

In those cases, use one of:

- review limitation
- suggested verification
- domain confirmation needed
- follow-up cleanup
- non-priority supporting analysis

Non-priority does not mean invisible. If a verified or plausible material concern is not selected as a priority finding, give it a visible home in suggested verification, follow-up/domain confirmation, review limitation, or another normal report section. Omit only when the concern was disproven or is low-confidence speculation that would not help the commit decision.

## Scope and Confidence Interaction

If a claim is strong but not fully proven because some context is missing:

- keep the claim narrow
- lower the confidence
- explain the missing context
- avoid overstating certainty

If the missing context itself is the main issue, prefer a `👁️` review limitation over a speculative correctness or security finding.

## Performance and Release Escalation

`📈` and `🧭` markers are not automatically non-blocking.

Promote them to blocking when the evidence shows a concrete path to:

- user-visible outage
- material latency regression
- material cost spike
- failed deployment
- unsafe migration
- broken rollback
- irreversible release damage

## Final Selection Checklist

Before finalizing the findings list, verify:

- each finding has exactly one primary marker
- each finding is backed by concrete evidence
- each blocking finding includes a blocking reason
- independent candidate risk points were dispositioned as a priority finding, suggested verification, follow-up/domain confirmation, review limitation, or omitted low-confidence speculation
- no independent priority-threshold risk was replaced by executive summary, commit guidance, or risk summary prose
- findings were merged only when the underlying risks share the same affected object, trigger condition, failure mode, root cause, and corrective action
- tally counts match the final set of findings
- speculative concerns have not been overstated as priority findings
- no runtime boundary failure, ignored contract parameter, side-effect validation gap, security TOCTOU residual, or material review-scope gap was removed merely to make the report shorter
- every material candidate omitted from priority findings has a visible non-priority disposition unless it was disproven or low-confidence speculation
