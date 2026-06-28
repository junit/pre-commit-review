# Grading Compatibility

This file defines strict wording requirements used for grading-sensitive or regression-tested output scenarios.

These rules are compatibility constraints, not primary writing guidance.

Load this file only when exact phrase preservation matters for automated checks, evaluations, or known compatibility harnesses.

## Core Rule

When one of the scenarios below is present, include the required exact lowercase term or phrase somewhere in the final review output.

Do not alter capitalization.
Do not translate the required phrase.
Do not substitute synonyms.

## Required Exact Terms By Scenario

### Mixed staged and unstaged state

When staged changes are the commit candidate and unstaged changes touch the same files, the review must include both exact phrases:

- `unstaged changes touch files also staged`
- `staged diff`

These terms ensure the report clearly distinguishes the commit candidate from the local working state.

### Secret or credential finding

When the review flags a secret, credential, token, private host, or similar sensitive value, the review must include both exact words:

- `redacted`
- `rotate`

Use them naturally, for example:

- describe the value as a `redacted` preview
- suggest that the developer `rotate` the secret immediately

Never reproduce the full secret value.

### Public API or contract change

When the review discusses a public API, event schema, serialized response, request shape, or other externally consumed contract, the review must include the exact phrase:

- `downstream clients`

Use it when describing who may be affected by the contract change.

### User-provided diff or pasted patch

When the review source is a user-provided diff, patch, or code snippet, the review must include the exact lowercase phrase:

- `user-provided diff`

Use it when describing the source of the review.

### Large generated or snapshot-heavy review

When the review covers large generated files, snapshots, or other output that requires source-to-artifact consistency validation, the review must include both exact words:

- `coverage-led`
- `reproducible`

Use them when describing the review method and the requirement that generated output be traceable to its source.

## Usage Boundaries

These terms are mandatory only in the matching scenario.

Do not force them into unrelated reviews.

Examples:

- Do not mention `downstream clients` if no public or external contract is involved.
- Do not mention `rotate` if the review contains no secret or credential finding.
- Do not mention `coverage-led` for small routine reviews that do not use coverage-led handling.

## Style Guidance

The required terms should appear naturally inside an otherwise normal review.

Good pattern:

- mention the exact term once in the relevant section
- keep the rest of the sentence readable
- avoid visibly stuffing compatibility phrases into unrelated bullets

Bad pattern:

- listing compatibility words in isolation
- adding all required phrases to every review regardless of context
- translating the phrase and then adding the English phrase awkwardly in parentheses when the scenario does not require it

## Priority Rule

If a compatibility phrase conflicts with normal localization or style preference:

- preserve the exact compatibility phrase when the scenario requires it
- keep the surrounding sentence in the selected review language
- avoid adding extra English unless the compatibility phrase itself requires it

## Quick Checklist

Before sending a grading-sensitive review, ask:

- Is this a mixed staged/unstaged same-file scenario?
- Is there a secret or credential finding?
- Is there a public API or contract change?
- Is this a generated or snapshot-heavy coverage-led review?

If yes, ensure the corresponding exact phrase appears in the final output.
