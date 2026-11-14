# Product

<!-- impeccable:product-schema 1 -->

Source: inferred from `REQUIREMENTS.md` (owner-answered, final) and the lead's assignment brief.
No interview round was possible in this session (subagent, no answer mechanism); every fact
below is taken from those two documents, not from the owner directly. [INFERENCE] marks
anything derived rather than stated.

## Platform

web

The mockup is static HTML. The shipped product is one NSH codebase targeting desktop
(macOS, Linux, Windows), web, iOS, Android and a terminal target (REQUIREMENTS D3).

## Stack

Mockup: plain static HTML/CSS/JS, no build step, opens from `file://` (assignment).
Product: NSH (`longbridge/nsh`); controls must be expressible with NSH elements.

## Users

One owner, many devices (D2). A developer who runs Corral, an autonomous delivery engine that
drives coding agents through run, gate, review and land, on his own workstation. He checks it at
the desk, from a phone away from the desk, and from a terminal. Teams are a non-goal.

## Product Purpose

Let the owner see what changed and who moves next, and decide anything that needs him, from any
device, without the work stalling while no UI is open (D1, D8). Success: questions are decidable
on one screen (R12-R17); a dead daemon never looks like a quiet board (R3).

## Positioning

Corral is the loop owner (`corrald`): sync, admission, autopilot, gates, review and landing run
without a client. The app is a thin renderer over a projection. Its unique content is the rail
`run · gate · review · land` with candidate-bound verdicts, and "who acts next" grouping.

## Operating Context

- Desk: large monitor, alongside terminals and editors; long sessions, glance-and-return.
- Away: phone, short sessions triggered by a push ("needs you"), often one-handed. [INFERENCE]
- Terminal target: the legacy `corral-tui` user base; retired after parity (D9).
- Agents run in herdr-core (target) or Herdr TUI (legacy) on the daemon's machine (D6, R6).

## Capabilities and Constraints

- Vocabulary from the Opus board: `needs you`, `gating`, `held · <reason>`, never bare
  `ready`/`blocked`. Rail glyphs `✓ ◐ ✗ ◌ ○ ·`.
- Destinations: Inbox, Now, Changes, Plans, Agents, Review, Timeline, Usage, Settings
  (Devices, Machines).
- Health line always visible; reach state always inspectable (R3, R10).
- Phone parity: no feature on one device only (R7, D4).
- Offline reads; actions queue or refuse visibly (R9).
- Authority rules unchanged: gates, independent review, holds, owner-only settings (R2).
- Usage is visibility only, no router (D10).

## Evidence on Hand

- Real board snapshot: `~/corral-board-opus/data.js` (2026-11-07), including T-868/R1/Q1.
- No real data for: terminal tails, transcripts, quota numbers, machines, devices. These are
  illustrative and must be labelled.

## Product Principles

1. Default screen answers "what changed, and who moves next".
2. One line per thing, fixed anatomy; collapse repetition, never information.
3. Health before content: stale must never pass for current.
4. Decide in place: a question carries its consequences, place, evidence and history.
5. Same product on every device, each in its own form.

## Accessibility & Inclusion

WCAG AA contrast in light and dark themes; full keyboard operation on desktop. [INFERENCE: no
stated standard beyond "accessible contrast in both themes".]
