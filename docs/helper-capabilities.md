# `collect_diff_context.sh` — Full Capability Reference

This is the deep-integrator reference for the read-only helper at `scripts/collect_diff_context.sh`. Most users only need the summary in [README.md](../README.md); read this when you are building automation on top of the helper's structured output.

The helper is the source of truth for diff source, review boundaries, and snapshot identity. It never fetches, stages, resets, installs, or modifies files, and it never runs, rewrites, or skips tests.

## Control Plane Gateway

The review workflow starts with `scripts/collect_diff_context.sh --control-plane`. This bounded gateway:

- emits a compact `--control-plane` JSON gateway with an authoritative full-scope content fingerprint, per-unit fingerprints, bounded units/groups, work order, and reusable command templates
- supports `--expect-scope <fingerprint>` on follow-up retrieval so stale group/path output fails closed
- disables external diff and textconv drivers for both fingerprints and emitted review bytes, keeping snapshot identity and inspected content semantically aligned
- is authoritative only when its collection-start and collection-end fingerprints match

## Diff Source Resolution

- detects whether the current directory is a Git repository
- prefers staged changes when present
- falls back to unstaged changes or branch-vs-base comparison
- reports diff stats, file lists, and status
- identifies truncation, path/content high-risk candidates, generated-like files, lock files, and top-churn files
- records rename, delete, binary, mode-only, and submodule pointer changes as manifest units

## Coverage-Led and Reducer Automation Output

For large or fragmented diffs, the helper emits structured sections so a reducer or subagent can review every unit without Markdown table parsing:

- Review Manifest and Review Groups for coverage-led commit-readiness workflows
- Review Plan JSON for reducer-friendly automation
- Split Suggestions for review groups that exceed the hard budget
- Split Unit Diff Preview blocks for hunk-level review
- Coverage Ledger Template with pending review units
- Group Review Result templates for reducer-ready group findings
- Reducer State Snapshot Template for long multi-step reviews
- Coverage Validation Checklist for reducer preflight
- Full Review Execution Plan with ordered split/review steps
- Group Review Work Packets for serial or delegated group review
- Reducer Finalization Template for final synthesis gates
- best-effort Dependency Summary for cross-file reduction
- bounded Semantic Context Queries from project-provided read-only grep patterns
- Test Selection Hints for changed test files that look environment-dependent, including common JVM/Spring/Quarkus/Micronaut, Maven/Gradle integration naming, JUnit tags, Testcontainers, Docker Compose, WireMock/MockServer, pytest markers, Playwright/Cypress/Node e2e, Go build tags, Rust ignored/integration tests, and database/cache/broker/search service configuration
- a suggested review queue for large or truncated diffs

## Safety Semantics

- omits the global raw diff from default output when it exceeds the inline budget, while keeping the structured plan visible
- truncates explicitly requested or inlined diffs safely when needed
- Test Selection Hints are read-only guidance for choosing focused verification commands and for distinguishing sandbox failures from code failures. A `no-known-env-heavy-marker` hint is not proof that a test is isolated; it only means the helper did not match a known environment-heavy marker.

Reducer and subagent automation should prefer authoritative `Review Control Plane JSON`; the older Review Plan/Manifest/Ledger sections remain compatibility output. Automation must not reconstruct scope from direct `git status` or `git diff --name-only` after the helper has emitted a manifest.
