# Review Risk Taxonomy

This file defines the taxonomy used for priority findings in commit-readiness reviews.

It answers only these questions:

- Which marker should a finding use?
- How are tally counts calculated?
- What fields must each finding contain?
- What counts as acceptable evidence?

Verdict selection rules, rendering templates, visual review layout, coverage-led workflow, and grading compatibility are defined elsewhere.

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

### Review limits

Count as review limits:

- every `👁️` finding, whether blocking or non-blocking

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

### 8. Blocking reason

Include this field only for blocking findings.

It must explain why the issue must be fixed before commit, not just repeat the title.

## Evidence Rules

Always prefer direct evidence from the reviewed diff.

Use surrounding context only when it materially helps determine impact or correctness.

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

In those cases, use one of:

- review limitation
- suggested verification
- domain confirmation needed
- non-priority supporting analysis

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
- tally counts match the final set of findings
- speculative concerns have not been overstated as priority findings
