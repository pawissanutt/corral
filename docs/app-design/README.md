# Corral app integration: design handoff

The [app connection contract](CONNECTION-CONTRACT.md) describes the implemented independent client and authenticated native/browser transport boundaries. The [earlier transport proof](TRANSPORT-PROOF.md) is historical feasibility evidence. The [app guide](../../apps/corral-app/README.md) describes current setup and limits.

The app now wires typed reads and authorized actions across its current destinations, including worker/PM questions and the shared assistant/PM composer. This is implementation status, not a claim that every private journey or the full gate has passed.

Imported from `/home/pawis/corral-app-design/` into branch `app-daemon-integration`, based on Corral commit `6f91ec1175e1ac1edc5ff5a568dd6620e4de824b`. This is a design handoff and next-step recommendation, not an implementation task contract or authorization to run agents, change automation, or install services.

## Contents and authority

- [REQUIREMENTS.md](REQUIREMENTS.md): owner-answered product decisions and requirements. Preserve the one-chat model, assistant sessions in Agents, no idle stop, per-device authority, and full device parity as the target.
- [SESSION-CONTROL.md](SESSION-CONTROL.md): owner decisions of 2026-11-15 for controlling agent sessions from the app (chat, Cancel, Pause, Terminate, PM launch, browser authority). Written in this repository, not imported; it overrides the architecture's session proposals.
- [ARCHITECTURE.md](ARCHITECTURE.md): proposed target architecture, historical source citations, and dated implementation notes. Section 11 records later deviations; section 14 defines delivery phases. Proposed types and paths are not proof they exist.
- [PRODUCT.md](PRODUCT.md): inferred product context for the original static mockup, not an additional owner interview or override of the requirements.
- [mockup/DESIGN.md](mockup/DESIGN.md) and [mockup/index.html](mockup/index.html): visual/interaction reference and its runnable static implementation. The mockup's illustrative data and simulated actions are not daemon capabilities. Open `mockup/index.html` locally; its scripts, styles and fonts are included.

The imported files are preserved verbatim. Originals remain in the external directory. The old architecture backup, Impeccable metadata and screenshot captures were not imported. Screenshot inventories in the design document describe the original directory; use the runnable mockup as visual truth, especially for the later model-selector changes.

Corral's current [daemon guide](../guide/daemon.md), [daemon-first implementation record](../records/daemon-first.md), app [README](../../apps/corral-app/README.md), repo guidance and source describe shipped behavior. They take precedence over historical claims that a component is built or missing; owner requirements still govern the product target. Keep future integration decisions here in the repository rather than relying on edits to the external copy.

## Implementation status

The app remains a separate Cargo workspace and uses an app-local client on `corral-protocol`, not a standalone `corral-client` crate. Native enrolled Unix and paired loopback Noise WebSocket sessions use the same typed app services. Device authority and negotiated capabilities govern actions; disconnected or unsupported data is never replaced with fixture facts. Fixture preview is explicit.

The daemon hosts assistant sessions; Ask also targets the PM inbox. Settings exposes configured profiles and the assistant model selection, while provider availability and quotas remain reported only where known. The app guide documents attachments, reconnect and result recovery.

Timeline reads use indexed half-open timestamp windows (including owner/PM points),
overlapping recorded sessions, and predecessor phase state. Completed historical
attempts do not consume the window's lifecycle budget. Crossing phases keep their
recorded start and session attribution; events at the exclusive upper bound are not
applied, so an unclosed phase is open as of that bound, not ended at the reader's clock.
Needs-me context includes decision receipts since the last unconditional close:
one answered decision does not imply that every pending decision was answered.
The existing task/window/context safety budgets still fail explicitly for genuinely
oversized selections. Burnup remains cumulative over real task creation and landing
events; a Timeline date range does not narrow its totals.

Private smoke has launched the native terminal and paired packaged Chromium against a private daemon, with matching trusted-side pairing codes and a live seven-task board and connected Settings. It does not establish the full browser journeys or root gate. GTK desktop and a physical phone have not been built or tested on this host; phone web layout remains in scope.

The architecture's section 1 and its phases are historical proposals, not a current inventory. Its proposed alternate rollover summary profile is not a canonical supported setting. Do not plan the already implemented daemon and command router again from scratch.

Keep runtime migration, remote phone reach, relay/push and classic-UI retirement in their later phases. Phone layouts remain part of the UI contract; later phone packaging is not permission to create a desktop-only product. Keep the sidebar collapsible and available at narrow widths. Retire classic only after actionable parity, not after a successful board connection.
