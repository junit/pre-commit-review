# Visual Review Rules

This reference file defines guidelines for performing visual and user interface reviews (visual review matrix, screenshots, interactive states, design consistency, accessibility, etc.).

## Visual Review Criteria / 视觉审查准则

Visual Review is triggered when the changes contain UI components, CSS styles, screenshot updates, design system adjustments, layout changes, images, PDFs, binary assets, or other UX changes that cannot be fully judged by text diffs alone.

A visual review must clarify:
1. Which pages, components, viewports, or states have been reviewed;
2. Which states remain unreviewed;
3. Whether screenshots, Storybook, Playwright, or manual design check is required.

---

## Visual Review Matrix / 视觉审查矩阵

When conducting a visual review, evaluate the following six dimensions:

| Dimension (领域) | Target & Signals (信号) | Evidentiary Basis (依据) | Conclusion & Action (结论) |
|---|---|---|---|
| **Layout & Responsive**<br>(布局与响应式) | Pass / Issue / Under-verified | Mobile vs desktop layout, alignment, flex/grid wrappers, padding, overflow. | UI breakdown impact, required viewport testing. |
| **Interactive States**<br>(交互状态) | Pass / Issue / Under-verified | Focus ring, hover shadow, active click, disabled opacity, loading spinner, error text, empty state. | Actionability, usability blockers, missing states. |
| **Accessibility**<br>(可访问性) | Pass / Issue / Under-verified | Screen reader labels (ARIA), keyboard focus order, semantic tags (button vs div), color contrast. | WCAG blockers, screen reader accessibility. |
| **Text & Localization**<br>(文案与本地化) | Pass / Issue / Under-verified | Translation files, dynamic text overflow, text wrapping, font-family, RTL language layout. | Typo blockers, truncated text fixes, i18n support. |
| **Design Consistency**<br>(设计一致性) | Pass / Issue / Under-verified | Design tokens (colors, margins, borders, typography) matching design specs. | Off-spec styles, custom overrides. |
| **Regression Risk**<br>(回归风险) | Low / Medium / High | Shared UI components, global layout wrapper updates, style snapshot updates. | Visual regression check, affected sibling pages. |

---

## Screenshot & Visual Verification / 截图与视觉验证

- Do not guess UI layout when visual assets are unavailable. If a stylesheet is modified but no screenshots or live preview is provided, mark it as a review limitation.
- Verify screenshots under multiple themes (light, dark) and screen sizes (mobile, tablet, desktop) where appropriate.
- Check user interaction flows (e.g., popup showing and dismissing) rather than only static elements.
