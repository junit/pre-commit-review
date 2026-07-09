# Finding Verification

This file defines the verification gate for strong findings in commit-readiness reviews.

It answers only these questions:

- Which findings require independent verification before they are reported?
- What evidence is required for negative, absolute, security, auth, data, framework, and blocking claims?
- When must an unverified concern be downgraded into a review limitation, suggested verification, or omitted?

Verdict selection, marker taxonomy, rendering templates, and coverage-led accounting are defined elsewhere.

## Purpose

Finding verification exists to prevent false confidence in the final report.

Coverage accounting proves that review units were inspected. It does not prove that every conclusion drawn from those units is true. Before a strong claim is surfaced, verify the claim itself.

## When To Use This Gate

Use this gate before emitting the final review whenever any of the following is true:

- the report will include one or more priority findings
- a finding is blocking or could drive `DO_NOT_COMMIT`
- the claim involves security, privacy, auth, authorization, injection, data loss, migration harm, or irreversible release impact
- the claim is negative or exhaustive, such as "no validation", "never written", "unused", "dead code", "all callers", or "no code path"
- the wording is absolute, such as "always", "never", "entirely", "completely", or "impossible"
- the claim depends on framework, library, ORM, transaction, annotation, middleware, interceptor, compiler, or runtime behavior
- the claim comes from delegated, grouped, reduced, or resumed review work rather than direct final-pass inspection
- the user or another reviewer challenges a previously accepted claim with concrete counterevidence

Low-risk, positive, narrowly scoped notes can use the normal risk-taxonomy evidence rules without this full gate.

## Verification Queue

Before final synthesis, classify candidate claims into these buckets:

- mandatory independent verification: blocking findings, negative or absolute claims, security/auth/privacy/data findings, and framework-behavior claims
- sampled verification: medium-or-lower positive findings that are narrowly scoped and do not change the verdict
- downgrade or omit: claims whose required verification cannot be completed

Do not adopt delegated or reducer findings solely because they are well written. High-impact claims must be checked against the underlying code, diff, documentation, or source behavior before they reach the final report.

## Gate 1: Location Claims

Any `file:line`, `file:line-line`, `file:function`, or equivalent object reference must be checked against the referenced source before it is reported.

Rules:

- verify that the referenced line or symbol actually contains the behavior described
- if the cited line only receives data, do not use it as evidence that validation, authorization, persistence, or side effects occur there
- if the line cannot be checked, use a less precise object reference and lower confidence

A dense set of citations is not evidence by itself. Citations become evidence only after the referenced code has been inspected.

## Gate 2: Negative and Absolute Claims

Negative or exhaustive claims require broader evidence than positive claims.

Before reporting a claim such as "never written", "no validation", "dead code", or "all callers":

- search the relevant module or repository scope, not just the changed directory
- include naming variants in the search pattern: field names, setters, database columns, config keys, snake_case/camelCase variants, and domain aliases where applicable
- check generated, migration, configuration, and test-adjacent paths when they could participate in the behavior
- record the actual searched scope in your internal reasoning

If search coverage is incomplete, do not report the claim as fact. Reword it narrowly, move it to suggested verification, or surface it as a review limitation.

## Gate 3: Auth and Trust-Boundary Claims

Security, auth, authorization, privacy, and injection findings must be traced to the execution point.

Rules:

- follow the path from input boundary to service/helper/middleware/aspect/interceptor and to the operation being protected
- identify where validation or authorization actually executes, not merely where data is accepted
- if a receive layer looks unsafe but an execution-layer helper enforces the boundary, withdraw the vulnerability finding
- if the enforcement point cannot be inspected, downgrade to a review limitation or suggested verification instead of reporting a confirmed vulnerability

For auth claims, "parameter accepted here" is not enough evidence. The report needs the missing or broken enforcement mechanism.

## Gate 4: Framework and Library Behavior

Do not infer framework or library internals from call-site shape alone.

When a finding depends on behavior such as transaction propagation, ORM optimistic locking, interceptor ordering, annotation semantics, compiler output, routing precedence, serialization defaults, or lifecycle hooks:

- verify against versioned source code, bundled sources, official documentation, or a focused test
- cite the framework behavior, not only the application call site, when that behavior is what makes the finding true
- if the installed version cannot be confirmed, say so and avoid a high-confidence finding
- if the behavior remains uncertain, downgrade to a review limitation or suggested verification

Training memory and general framework intuition are not sufficient evidence for these claims.

## Gate 5: Blocking and Impact Claims

Blocking status is a separate claim from issue existence.

Before marking a finding as blocking, verify:

- the trigger condition: how the bad path is reached
- the affected scope: users, data, tenants, API consumers, release path, or operational system
- the consequence: build failure, runtime failure, data corruption, auth bypass, privacy leak, irreversible migration harm, major contract break, outage, or material cost/performance regression
- the absence of a visible mitigating control in the reviewed context

Do not escalate reliability, idempotency, logging, or maintainability issues into blockers unless the trigger and consequence satisfy the main verdict rules.

## Gate 6: Challenge Reverification

If the user or another reviewer provides concrete counterevidence, reverify the original claim from primary evidence.

Prioritize reverification when the challenged claim depends on:

- negative or exhaustive search
- auth or trust-boundary execution flow
- framework or library behavior
- blocking impact calibration

A prior agreement or earlier pass is not a substitute for rechecking the evidence.

## Reporting Rules

After this gate:

- keep verified strong claims as priority findings
- report every independently verified priority-threshold risk as its own priority finding; do not drop it because the executive summary, commit guidance, or another finding mentions the topic
- merge verified claims only when they share the same affected object, trigger condition, failure mode, root cause, and corrective action
- narrow overbroad wording before reporting
- move plausible but unverified concerns to review limitations, suggested verification, follow-up, or domain confirmation needed
- when a material candidate concern is downgraded or omitted, keep the disposition visible unless it is low-confidence speculation that would not help the commit decision
- do not expose this internal gate in routine output unless the user asks for process detail

## Final Checklist

Before producing the final review, verify:

1. every blocking finding passed the applicable gates
2. every negative or absolute claim has adequate search coverage or has been narrowed
3. every security/auth/privacy/data claim was traced to the execution point
4. every framework/library behavior claim is backed by source, official documentation, or a focused test
5. every `file:line` style reference was checked against the referenced source
6. confidence levels match the evidence actually inspected
7. unverified material concerns are visible as review limits or suggested verification rather than overstated findings
