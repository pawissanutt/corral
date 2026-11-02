---
name: Corral corral-pi workbench
description: Native panel presentation over the real bound Pi session
colors:
  canvas: "#0f1012"
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

# Design System: Corral corral-pi workbench

## Overview

**Creative North Star: "The panel workbench"**

A real native coding session stays primary. Filled graphite inspection regions and restrained blue focus make related state accessible without replacing Pi's transcript, editor or authorization lifecycle.

**Key Characteristics:**
- Native character-cell panels, not a web dashboard.
- Persistent context with a separate filled composer.
- Shared keyboard/pointer inspection without a second conversation runtime.

## Colors

The normative palette above comes from panel-canvas.ts, panels-view.ts and panels-theme.ts. Focus blue marks selection and native editor borders; neutral graphite owns inspection surfaces. Success, attention and error retain explicit words/signs alongside color. Existing syntax colors remain inherited where not overridden.

**The State Rule.** Displaying progress, a tool result or a diff does not imply a passing gate, review or owner acceptance.

## Typography

The terminal owns font, size, cell dimensions and shaping. No application font is installed or forced. Color, weight, spacing and literal state labels form the hierarchy. Wide glyphs are measured before clipping; controls in data are sanitized.

## Layout

Fullscreen main width reserves a 34-column rail plus one separator at 105 physical columns or more. Below that—or when manually hidden—the native inspector is a single unboxed navigation row at rest; focus adds two detail rows and explicit expansion adds up to four. Pi owns transcript scrolling/follow, selections and editor height. Inspector content scrolls independently; SDK overlays own modal input.

**The Continuity Rule.** Resize and inspection preserve draft, selected tab/file and bound session.

## Elevation & Depth

Explicit canvas and raised panel cell backgrounds plus one-cell rules create depth. The alternate-screen adapter supplies default foreground/background through SGR, not terminal-palette/default-color setters; the surrounding terminal and normal screen retain their theme. Read-only details and existing prompts are corral-pi overlays/selectors, not extra browser windows.

## Shapes

Square terminal boxes with one-cell rules and two-column content inset. Existing Pi user-message and composer regions share the graphite theme; the composer keeps the SDK's real editing behavior.

## Components

### Session rail and inline inspector
Context, Files, Agents and Plan labels share one stable tab model. Blue indicates the active tab/focused border. Counts are measured or explicitly unknown. F6/arrows/Enter and native pointer controls inspect rather than mutate.

### Composer
The Message field has a filled graphite body, inset native input and model/effort in its bottom control bar. A CustomEditor subclass uses public border/padding/render hooks; stock editing, model/effort shortcuts, paste, submission, scroll indicators and owner-input hooks remain intact.

### Status ownership
Branded header: binding, role, generation and repository. Composer: model and effort. Wide Context overview: measured usage, changed files, agents/jobs and plan; when that overview is not visible, usage moves to the compact footer. The stock multi-row footer is replaced, not stacked. Refusals and extension statuses are retained.

### Read-only main views
F7 Changes and F8 Activity replace only the main reading area through the optional typed SDK mainView hook; F9 restores the same transcript instance. The real input dock remains SDK-owned. Changes retains scope, selection and captured hunk provenance, exposes meaningful comparison/search/refresh controls, and appends references without submission. Activity distinguishes local job/monitor observations from canonical settlement. Additions/removals retain +/− signs as well as semantic colors. Foreground prompts and active selection suppress background interception.

### Candidate and evidence inspection

Bound task/run sessions default Changes to the committed candidate, using exact immutable comparison-base and HEAD SHAs for paths, diffstat and hunks. `c` still selects the independent working/index comparisons; `o` is an explicitly unbound repository inspection, not a session rebind. Retained diffs keep their captured identities, selected-file status and diffstat until the new patch is explicitly applied; the current file-list metadata is not substituted into an older displayed hunk.

F10 (Proof on compact navigation) and `/panels evidence` show the canonical read-only evidence workbench. Wide rows use a matrix; narrow rows stack the same implemented/tested/reviewed/accepted fields. `j/k` selects a criterion; pointer selection uses the same state; paging/wheel scrolls the reading area. Implemented is self-reported progress, test observations are worker assertions with canonical admission failures, and review applies candidate-wide. No per-criterion acceptance record exists, so acceptance stays not recorded. Legacy/no-manifest, missing, failed, not-run, stale, unavailable and superseded states are explicit.

The adapter returns a versioned projection, not full task/history JSON. Refreshes coalesce and cache for five seconds, never run from render, and fail closed at input/output bounds. A tool outcome invalidates the cache but does not grant freshness; only a new canonical read reconciles it. Failed/overlapping reads cannot relabel retained proof as current. No annotation editing, receipt mutations or acceptance toggles are exposed.

### Attention and decision inbox
F11 Inbox and `/panels inbox` read the launch-bound workspace, independently of the Changes repository selector. Exact task/container/key/revision, state, scope and reason remain visible; `h` reveals retained history. Enter opens read-only detail. `a` revalidates and opens the existing answer-flow instructions, never a new response or owner-input path. Closed/stale/ineligible requests and failed reads stay explicit; removed selections never silently become another task. F10 Evidence retains its existing binding. See `docs/guide/pi-inbox.md`.

### Images
Click an inline terminal image to fit its existing bytes into the main reading area. Click again (including a quick second click), Esc or F9 restores Conversation. Draft and transcript instance remain intact. Image placement is bounded by the actual main viewport, not guessed composer height. This uses the optional packaged SDK image activation hook; no fetch, external viewer or model call. Native rendering still requires a supported graphics terminal. Do not layer expanded images in SDK overlays: captions over existing inline graphics are not reliably composited.

## Do's and Don'ts

- Do preserve the real editor, transcript and bound identity.
- Do show unknown, stale and unavailable state explicitly.
- Do relocate metadata instead of hiding it on narrow terminals.

- Don't introduce a browser runtime or another agent loop.
- Don't make local UI state look like canonical progress, evidence or authority.
- Don't change native fonts or promise full terminal/accessibility certification.

Not canonized: summary ellipsis and small-terminal footer truncation are current limitations, not desired future rules. Low-color terminals and Windows/IME/accessibility have not been comprehensively certified.
