---
name: Corral Pi native workbench
description: Native panel presentation over the real bound Pi session
colors:
  panel: "#191b20"
  text: "#e0e3e9"
  muted: "#9ba2b0"
  focus: "#83a8ef"
  rule: "#414752"
  selected: "#2c3548"
  success: "#9ac7a3"
  attention: "#e3bd7c"
  error: "#e699a3"
typography:
  body:
    fontFamily: "monospace"
    fontWeight: 400
  emphasis:
    fontFamily: "monospace"
    fontWeight: 700
---

# Design System: Corral Pi native workbench

## Overview

**Creative North Star: "The panel workbench"**

A real native coding session stays primary. Filled graphite inspection regions and restrained blue focus make related state accessible without replacing Pi's transcript, editor or authorization lifecycle.

**Key Characteristics:**
- Native character-cell panels, not a web dashboard.
- Persistent context with a separate filled composer.
- Shared keyboard/pointer inspection without a second conversation runtime.

## Colors

The normative palette above comes from panels-view.ts and panels-theme.ts. Focus blue marks selection and native editor borders; neutral graphite owns inspection surfaces. Success, attention and error retain explicit words/signs alongside color. Existing syntax colors remain inherited where not overridden.

**The State Rule.** Displaying progress, a tool result or a diff does not imply a passing gate, review or owner acceptance.

## Typography

The terminal owns font, size, cell dimensions and shaping. No application font is installed or forced. Color, weight, spacing and literal state labels form the hierarchy. Wide glyphs are measured before clipping; controls in data are sanitized.

## Layout

Fullscreen main width reserves a 34-column rail plus one separator at 105 physical columns or more. Below that—or when manually hidden—the native inspector is a single unboxed navigation row at rest; focus adds two detail rows and explicit expansion adds up to four. Pi owns transcript scrolling/follow, selections and editor height. Inspector content scrolls independently; SDK overlays own modal input.

**The Continuity Rule.** Resize and inspection preserve draft, selected tab/file and bound session.

## Elevation & Depth

Tonal surfaces and one-cell rules create depth, without CSS effects or terminal background mutation. Read-only details and existing prompts are native Pi overlays/selectors, not extra browser windows.

## Shapes

Square terminal boxes with one-cell rules and two-column content inset. Existing Pi user-message and composer regions share the graphite theme; the composer keeps the SDK's real editing behavior.

## Components

### Session rail and inline inspector
Context, Files, Agents and Plan labels share one stable tab model. Blue indicates the active tab/focused border. Counts are measured or explicitly unknown. F6/arrows/Enter and native pointer controls inspect rather than mutate.

### Composer
The Message field has a filled graphite body, inset native input and model/effort in its bottom control bar. A CustomEditor subclass uses public border/padding/render hooks; stock editing, model/effort shortcuts, paste, submission, scroll indicators and owner-input hooks remain intact.

### Status ownership
Branded header: binding, role, generation and repository. Composer: model and effort. Wide Context overview: measured usage, changed files, agents/jobs and plan; when that overview is not visible, usage moves to the compact footer. The stock multi-row footer is replaced, not stacked. Refusals and extension statuses are retained.

### Read-only details
Bounded native overlays show Git diff or full metadata, with visible scroll/close controls. Foreground SDK prompts disable background panel interception.

## Do's and Don'ts

- Do preserve the real editor, transcript and bound identity.
- Do show unknown, stale and unavailable state explicitly.
- Do relocate metadata instead of hiding it on narrow terminals.

- Don't introduce a browser runtime or another agent loop.
- Don't make local UI state look like canonical progress, evidence or authority.
- Don't change native fonts or promise full terminal/accessibility certification.

Not canonized: summary ellipsis and small-terminal footer truncation are current limitations, not desired future rules. Low-color terminals and Windows/IME/accessibility have not been comprehensively certified.
