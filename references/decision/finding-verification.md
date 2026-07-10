# Finding Verification

This file defines the verification gate for strong findings in commit-readiness reviews.

It answers only these questions:

- Which findings require independent verification before they are reported?
- What evidence is required for negative, absolute, security, auth, data, framework, and blocking claims?
- When must an unverified concern be downgraded into a review limitation, suggested verification, or omitted?

Verdict selection, marker taxonomy, rendering templates, and coverage-led accounting are defined elsewhere.

## Contents

- Purpose
- When To Use This Gate
- Verification Queue
- Candidate Harvest Gate
- Priority Threshold Gate
- Gate 1: Location Claims
- Gate 2: Negative and Absolute Claims
- Gate 3: Auth and Trust-Boundary Claims
- Gate 4: Framework and Library Behavior
- Gate 5: Blocking and Impact Claims
- Gate 6: Challenge Reverification
- Reporting Rules
- Final Checklist

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

## Candidate Harvest Gate

Before verification and final synthesis, proactively harvest candidate concerns from the changed behavior. Do not rely only on issues that were already phrased as findings.

Add a candidate to the ledger when a change touches any of these material patterns:

- construction or rewriting of externally observable values or cross-boundary identifiers, such as resource locators, paths, selectors, generated IDs, cache or object keys, callback targets, or third-party operation descriptors, especially when optional inputs, platform/environment configuration, default values, or fallback behavior changed
- cross-boundary I/O, such as network, file-system, process, browser, IPC, plugin, database, message-bus, upload/download, callback, redirect, proxy, or third-party API interactions
- side-effecting operations such as create, update, delete, migrate, retain, rollback, configuration mutation, or irreversible external calls that accept validation, intent, mode, guard, or safety inputs
- access, identity, ownership, capability, scope, visibility, sandbox, workspace, account, or isolation-boundary behavior
- asynchronous, concurrent, lifecycle, scheduler, cache, retry, timeout, rate-limit, batching, queueing, or execution-context propagation semantics
- runtime prerequisites such as data model, schema, configuration, generated artifacts, binary artifacts, lockfiles, build output, deployment settings, environment variables, platform capabilities, or feature flags required by the changed code
- compatibility-sensitive fallback, default, empty, missing, error, or platform-specific behavior where old and new paths must remain equivalent for the same input

For each harvested candidate, either verify and classify it, disprove it, or keep it as suggested verification/domain confirmation/review limitation. Do not let a changed behavior category disappear merely because no one had already named it as a finding.

Maintain a candidate disposition ledger from verification through final synthesis. The ledger is internal by default and should track:

- affected object or contract
- risk class, such as runtime boundary, ignored intent parameter, data validation, security residual, test gap, framework behavior, or review scope
- evidence status, such as verified, plausible but unverified, disproven, or low-confidence speculation
- disposition, such as priority finding, suggested verification, follow-up/domain confirmation, review limitation, disproven and omitted, or low-confidence omitted
- final report location for every material candidate that is not disproven or low-confidence speculation

Do not emit the final verdict until this ledger has no material candidate without a final report location. The user-facing report does not need to show the ledger itself, but it must show each material candidate's disposition in a normal section.

## Priority Threshold Gate

Before spending finding slots on a candidate concern, decide whether it belongs in the priority findings list at all.

Priority findings are not limited to blockers. They are the high-signal concerns that materially affect the commit decision, follow-up risk, or required verification. A verified priority-threshold concern must not be moved out of the findings list merely because the final verdict remains commit-safe.

Promote a concern to a priority finding only when it has a concrete commit-readiness risk, such as:

- correctness, runtime, build, or data behavior impact
- security, privacy, auth, authorization, or trust-boundary impact
- public API, schema, migration, release, rollback, or downstream compatibility impact
- material performance, scalability, cost, or operational impact
- a test gap that covers high-risk changed behavior

Do not promote pure clean-code smells into priority findings by default. Examples include unused private helpers, spelling or naming issues, internal signature polish, minor duplication, or structure/style concerns with no demonstrated behavior, contract, release, or testing impact.

Do not classify a concern as a clean-code smell when it can change runtime behavior or break an explicit contract. These are priority-threshold candidates when verified:

- a boundary fallback is missing and can produce an invalid externally observable value, resource locator, cross-boundary identifier, request descriptor, or persisted state at runtime
- a caller-visible parameter, flag, mode, or option is ignored and that input controls validation, mutation, authorization, isolation, persistence, preview/simulation, rollback, or retention semantics
- state-changing or configuration-changing code silently bypasses a validation contract and can affect live objects, persisted state, derived outputs, or downstream consumers
- a framework/library assumption hides a reachable null, blank, missing-config, platform-specific, environment-specific, or isolation-boundary condition

Move those lower-priority concerns to follow-up items, suggested cleanup, or omit them when they would not help the commit decision. Escalate a clean-code concern only when you can state the concrete failure mode, affected object, trigger condition, and why it changes the commit decision.

Do not use "no blockers" as evidence for "no priority findings." If a non-blocking concern meets the priority threshold, keep it in the findings list and omit the blocking-reason line.

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
- include naming variants in the search pattern: field names, setters, stored or serialized field names, config keys, snake_case/camelCase variants, and domain aliases where applicable
- check generated, migration, configuration, and test-adjacent paths when they could participate in the behavior
- record the actual searched scope in your internal reasoning

If search coverage is incomplete, do not report the claim as fact. Reword it narrowly, move it to suggested verification, or surface it as a review limitation.

## Gate 3: Auth and Trust-Boundary Claims

Security, auth, authorization, privacy, and injection findings must be traced to the execution point.

Rules:

- follow the path from input boundary to the component, helper, middleware, hook, plugin, interceptor, adapter, or framework layer and to the operation being protected
- identify where validation or authorization actually executes, not merely where data is accepted
- if a receive layer looks unsafe but an execution-layer helper enforces the boundary, withdraw the vulnerability finding
- if the enforcement point cannot be inspected, downgrade to a review limitation or suggested verification instead of reporting a confirmed vulnerability

For auth claims, "parameter accepted here" is not enough evidence. The report needs the missing or broken enforcement mechanism.

SSRF, redirect, proxy, and other outbound-interaction claims must account for time-of-check/time-of-use behavior. A pre-use validation helper is not sufficient evidence that the trust boundary is closed when the later operation performs DNS lookup, redirect follow, proxy resolution, path resolution, process launch, or connection to a different address family. If rebinding, redirect-to-private-target, path/target drift, or post-validation destination drift remains plausible in a reachable path, keep it as a visible security residual: report it as a priority finding when the trigger and impact are concrete, or as follow-up/domain confirmation when environmental controls make it non-blocking.

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
- the affected scope: users, data, isolated accounts or workspaces, API consumers, release path, or operational system
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
- before final synthesis, reconcile the material candidate concern ledger: every boundary-condition bug, ignored contract parameter, side-effect validation gap, security residual, and review-scope gap that was investigated must either appear as a priority finding or have a visible non-priority disposition
- a candidate that is removed from priority findings for severity, confidence, or noise reasons still needs a final report location unless it was disproven or is low-confidence speculation
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
8. no priority-threshold boundary, contract, data, or security residual was hidden as a clean-code smell or removed for brevity
9. every material candidate in the internal disposition ledger has a user-visible final report location or a justified disproven/low-confidence omission
