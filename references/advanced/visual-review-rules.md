# Visual Review Rules

This file defines how to conduct a visual or UI-oriented commit-readiness review.

It answers only these questions:

- When is visual review justified?
- Which visual dimensions should be checked?
- What evidence is acceptable for visual conclusions?
- When should missing visual context become a review limitation?

Rendering layout belongs in the rendering templates. General verdict and finding rules belong in the decision files.

## When To Use Visual Review

Use visual review when the change meaningfully affects any of the following:

- UI components
- CSS or styling behavior
- screenshots
- layout or responsive behavior
- images or visual assets
- design-system tokens
- interaction states
- accessibility-relevant presentation
- localization-sensitive layout or copy behavior

If the change is mostly logic and visual meaning is incidental, stay on the normal review path and mention visual impact only if it is concrete.

## Core Expectation

A visual review must clarify:

- which pages, components, or flows were reviewed
- which states were reviewed
- which states, themes, or viewports remain unreviewed
- whether screenshots, Storybook, live preview, Playwright, or manual inspection are required

Do not imply that a UI is correct if visual evidence was unavailable.

## Primary Visual Dimensions

Evaluate these six dimensions when relevant.

### 1. Design Consistency

Check whether the change remains aligned with visible design conventions such as:

- tokens
- spacing
- color usage
- typography
- border and radius patterns
- component hierarchy

Look for off-spec overrides, hardcoded values, or visual drift from adjacent components.

### 2. Layout and Responsive Behavior

Check:

- alignment
- flex or grid wrappers
- spacing collapse
- overflow or clipping
- wrapping behavior
- mobile, tablet, and desktop breakpoints when relevant

Pay special attention to shared layouts or wrapper components, because a local diff can create broad downstream visual regressions.

### 3. Interactive States

Check whether the component or page accounts for states such as:

- hover
- focus
- active
- disabled
- loading
- error
- empty
- expanded or collapsed states

If the diff changes interaction logic or styling but only the default state is visible, mark the missing states as under-verified or as a review limit, depending on materiality.

### 4. Accessibility

Check for:

- semantic HTML usage
- keyboard reachability
- focus visibility
- ARIA labels or roles
- color contrast
- readable state communication
- screen-reader implications where visible evidence supports them

Treat accessibility as a real correctness dimension, not cosmetic polish.

### 5. Text and Localization

Check for:

- copy changes
- truncation
- wrapping
- overflow from longer strings
- placeholder or label clarity
- multi-language layout sensitivity
- RTL sensitivity when relevant

If the UI is text-sensitive and only one language or one content size was inspected, state that boundary explicitly.

### 6. Regression Risk

Estimate regression risk based on:

- how shared the component is
- whether tokens or globals changed
- whether snapshots changed
- whether layout wrappers moved
- whether interaction states were verified

Shared components, top-level layouts, and token changes usually deserve a wider blast-radius discussion than one-off page styling.

## Acceptable Evidence

Good evidence for visual review includes:

- screenshots
- Storybook states
- live preview inspection
- Playwright or browser automation output
- visible diff in CSS or component structure
- snapshot updates when paired with source change review
- accessible state definitions visible in code

Use the shortest direct evidence that supports the conclusion.

## Evidence Boundaries

Do not guess visual correctness when evidence is missing.

Examples:

- if CSS changed but no screenshot, preview, or state exercise exists, say so
- if only desktop was checked, do not imply mobile is correct
- if only light theme was checked, do not imply dark theme is correct
- if only static layout was visible, do not imply hover, focus, or loading states are correct

Missing evidence should become either:

- under-verified commentary, or
- a `👁️` review limitation when the missing state could change the verdict

## Screenshot and Preview Guidance

When screenshots or previews are available:

- inspect more than one viewport if the component is responsive
- inspect both themes if the product supports them and the diff can affect theme behavior
- inspect the states most likely to regress based on the diff
- prefer actual changed paths over generic gallery browsing

When screenshots or previews are not available:

- do not fabricate layout conclusions
- limit claims to what the code and diff actually show
- explicitly name the missing visual evidence

## Review Limit Interaction

Escalate missing visual coverage into a review limitation when:

- a key UI state was not exercised
- a screenshot-dependent conclusion cannot be supported
- shared design tokens or layout wrappers changed without visual confirmation
- a binary or asset change cannot be inspected
- accessibility-sensitive behavior depends on runtime state you cannot observe

If the missing visual context could change the commit decision, the limitation is potentially blocking.

## Common Findings

Typical visual-review findings include:

- token drift or hardcoded visual values
- layout overflow or breakpoint breakage
- missing focus-visible treatment
- low contrast in a changed state
- missing loading, error, or disabled state handling
- copy truncation or localization overflow
- broad regression risk from shared component or wrapper changes

These are examples of review topics, not automatic findings. Always tie them to evidence.

## How To Write Visual Findings

A good visual finding should identify:

- where the issue appears
- which state or viewport triggers it
- what the user sees or fails to see
- how to fix it
- how to verify the fix

Avoid vague phrasing like:

- looks off
- may have alignment issue
- probably fine on desktop

Prefer concrete phrasing such as:

- focus ring disappears on hover for the danger button
- mobile width below 375px causes two-line CTA overflow
- dark theme contrast drops below readable threshold for secondary text

## Final Checklist

Before finalizing a visual review, verify:

1. the reviewed visual scope is explicitly named
2. unreviewed visual states are explicitly named
3. no unsupported visual claims are presented as fact
4. shared-component or shared-token blast radius is discussed when relevant
5. accessibility-sensitive changes are not dismissed as polish
6. missing screenshots or preview evidence are surfaced honestly
