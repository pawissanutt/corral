---
name: frontend
description: Build, style and polish web UI, HTML, CSS and SVG. Use for screens, components, responsive layouts, visual redesigns and accessibility work.
allowed-tools: read grep find ls edit write bash
---
{{marker}}

# Frontend

## Ground and brief

Use `read` for the project's AGENTS.md, DESIGN.md, wireframes and nearest styled siblings;
use `grep` or `find` to locate existing components and tokens. Respect the project's framework,
package manager and design authority. Do not install a new UI kit or rewrite the design system
for a small change. A supplied screenshot is a contract, not inspiration.

Before editing, state the primary action, information priorities 1–3, required content and what
is left out on purpose. For layout changes name the pattern, responsive collapse and scroll
owner of each region. For an existing surface capture the before states first, using visual-qa.

## Build with restraint

- Reuse tokens for colour roles, type, spacing and radii, in every supported theme. Make the
  important thing first and one action prominent. Choose tables for comparison, lists for
  ranking and detail views for one object; do not default to a grid of decorative cards.
- Keep required labels, inputs and data intact. Remove filler badges, duplicated summaries,
  unnecessary boxes, invented testimonials and vanity statistics. Copy says what a thing does.
- Use semantic HTML, visible labels, accessible names on icon controls and one coherent icon set.
  Keyboard order follows reading order; focus is visible and not hidden by sticky content.
  Escape dismisses the top overlay; destructive actions are never the default.
- Provide empty, loading, error and populated states. Keep button geometry stable while labels
  or spinners change. Put actionable errors beside their object. Preserve pending input.
- Make layouts work at narrow and wide widths, with long content, zoom and text spacing.
  Prefer flexible grids and deliberate wrapping over fixed widths. Avoid nested scroll traps.
- Aim for 44 CSS px touch targets; meet WCAG 2.2 AA target-size rules on pointer layouts.
  Check text contrast (4.5:1, large text 3:1), control contrast and non-colour state cues.
- Motion serves a state change, stays interruptible and does not block input. Respect reduced
  motion; avoid decorative loops, gratuitous gradients, shadows and accent side-stripes.

## Verify

Use `edit` for targeted changes, `write` for new files, and `bash` for the repository's cheapest
relevant checks. Load visual-qa with `read` and inspect the actual rendered result before claiming
visual success. Check required content, both shipped themes, keyboard focus, responsive reflow,
control states and reference fidelity. Report exact captures and any browser/accessibility checks
that could not run; a compile or DOM assertion alone is not visual verification.
