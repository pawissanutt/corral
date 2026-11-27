---
name: frontend
description: Design or change web interfaces, from focused component fixes to new responsive layouts. Use visual-qa when the task is to assess rendered output rather than design the change.
allowed-tools: read grep find ls edit write bash
---

# Frontend

Start with the interaction or content that must change. Read the nearest component, its existing states and applicable project guidance. Reuse local primitives, tokens and platform conventions. Determine whether any supplied reference is a requirement or an example; preserve required content and behavior, and explain intentional departures from a required reference.

For a small fix, keep the surrounding layout and unrelated states intact. For example, for an invisible focus state, use the local focus style where it works, then tab through the changed control in context. Verify that the focus indicator is visible and not hidden by sticky content or overlays. Check themes or neighboring states where the same change could affect visibility; do not redesign the screen.

## When designing a layout or flow

- Identify the primary task and required content. Choose a structure that serves it: a table for comparing records, a list for scanning items, or a detail view for one object. Plan how regions wrap or collapse and which region scrolls. Check relevant widths, zoom and long content rather than relying on a single ideal viewport.
- Use semantic controls and visible labels; give icon-only controls accessible names. Keep keyboard order aligned with the reading order. For dialogs, manage focus on entry and return, and let Escape dismiss the top dismissible overlay. Do not default to destructive actions.
- Where the flow loads or can fail, show meaningful loading, empty and error states. Keep pending input on recoverable errors and place actionable messages near the affected field or object. Avoid layout jumps when a button becomes busy.
- Use non-color cues for state and check contrast where styles change. For WCAG AA, normal text generally needs 4.5:1 contrast and large text 3:1. Aim for 44 by 44 CSS px touch targets where practical; that's a usability recommendation, not the WCAG 2.2 AA minimum (24 by 24 CSS px for pointer targets, with exceptions such as sufficient spacing). Use motion to clarify changes, and respect reduced-motion settings.

Stop when the changed interaction works in its affected states and the required content and behavior are intact, or report what could not be verified. Use `visual-qa` to choose and inspect captures when appearance matters; a compile or DOM assertion alone does not establish visual success. In read-only work, describe the proposed change and evidence without editing. The tool list does not grant permissions or require any particular tool.
