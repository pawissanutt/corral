# Corral app integration: design handoff

Next increment: [app connection contract](CONNECTION-CONTRACT.md), covering independent client compatibility, authenticated native/browser transport, and the boundary between a feasibility proof and production device access. The [completed browser transport proof](TRANSPORT-PROOF.md) records real Rust/wasm Noise interoperability, read-only daemon delivery, rejection cases and what remains unimplemented.

Imported from `/home/pawis/corral-app-design/` into branch `app-daemon-integration`, based on Corral commit `6f91ec1175e1ac1edc5ff5a568dd6620e4de824b`. This is a design handoff and next-step recommendation, not an implementation task contract or authorization to run agents, change automation, or install services.

## Contents and authority

- [REQUIREMENTS.md](REQUIREMENTS.md): owner-answered product decisions and requirements. Preserve the one-chat model, assistant sessions in Agents, no idle stop, per-device authority, and full device parity as the target.
- [ARCHITECTURE.md](ARCHITECTURE.md): proposed target architecture, historical source citations, and dated implementation notes. Section 11 records later deviations; section 14 defines delivery phases. Proposed types and paths are not proof they exist.
- [PRODUCT.md](PRODUCT.md): inferred product context for the original static mockup, not an additional owner interview or override of the requirements.
- [mockup/DESIGN.md](mockup/DESIGN.md) and [mockup/index.html](mockup/index.html): visual/interaction reference and its runnable static implementation. The mockup's illustrative data and simulated actions are not daemon capabilities. Open `mockup/index.html` locally; its scripts, styles and fonts are included.

The imported files are preserved verbatim. Originals remain in the external directory. The old architecture backup, Impeccable metadata and screenshot captures were not imported. Screenshot inventories in the design document describe the original directory; use the runnable mockup as visual truth, especially for the later model-selector changes.

Corral's current [daemon guide](../guide/daemon.md), [daemon-first implementation record](../records/daemon-first.md), app [README](../../apps/corral-app/README.md), repo guidance and source describe shipped behavior. They take precedence over historical claims that a component is built or missing; owner requirements still govern the product target. Keep future integration decisions here in the repository rather than relying on edits to the external copy.

## Baseline and design drift

The preceding readiness investigation exercised the installed daemon and the NSH terminal app without sending mutations:

- The daemon reported healthy, its Herdr TUI runtime available, and the PM live.
- A board subscription delivered 186 rows across two acknowledged pages. The app rendered live health, Now and selected-task board facts.
- The daemon negotiated `board.v1`, `cmd.v1`, `read.v1` and `classic.v1`. `detail.get` returned `method_not_found`; an Inbox subscription returned `unsupported topic`. The app's Inbox displayed its unavailable state rather than fixtures.
- The current app is a separate Cargo workspace at `apps/corral-app/`, with an app-local native client on `corral-protocol`; it has no standalone `corral-client` crate. Do not recreate working pieces merely because the proposal names a different layout.
- Commands already have a typed router and an idempotency journal. The app does not use them. Both command admission and served reads currently require the daemon's executable path/build identity. The app session currently accepts CONTROL frames only; command outputs and served read documents require DATA handling.
- The web build compiles out the socket client. Browser transport, device enrolment and authenticated app command access are still integration work.

These are a handoff snapshot, not a claim about future live task counts or daemon health. In particular, the architecture's section 1 is a historical baseline; section 11's `welcome.caps = board.v1 only` row and section 15's original direct-write decision are superseded by later daemon-first notes and the current protocol. Do not plan the command router or daemon loop again from scratch.

## Recommended next milestone: Phase 2a

The owner's D11 and architecture section 14 require **Needs you on desktop and web**, not just a native read-only board. A native intermediate increment is useful, but does not complete that milestone.

Sequence the work around one end-to-end journey:

1. **Agree the app-facing connection contract.** Reuse the existing protocol, router, result journal and conflict handling. Define independent app version/capability compatibility, local device enrolment and provenance without disabling the CLI's executable/build safety check globally. Prove the loopback WebSocket + Noise path works in NSH's wasm build; retain origin restrictions and the same authority rules on every transport.
2. **Serve Inbox and question context.** Add bounded typed subscriptions/readers for worker and PM questions, with revision, options and consequences, goal/plan position, inline evidence, follow-up thread and history. Identify which facts already exist and which require canonical fields; do not fabricate context from fixture text. PM questions need the canonical mirror described in section 6.
3. **Wire the existing question surface.** Replace the fixture source for this journey across the appropriate shells. Submit answers and follow-ups through the shared service path. Display pending, refused, stale and answered-elsewhere states; keep other unserved destinations explicit. A second surface must converge on the winning answer.
4. **Prove the milestone.** Answer a worker question and a PM question from the desktop app and from a browser. Show all required context, one winning answer across surfaces, conflict behavior on a stale revision, and no second effect after an uncertain-result reconnect. Use private fixtures; never answer a live owner's question for a smoke test.

The first implementation assignment should settle the connection contract and prove the native/browser transport seam, not turn on fixture action buttons. Then deliver the question journey against that contract. Scope the checks to each actual change; the app and root workspaces have separate toolchains and gates.

## After Phase 2a

Phase 2b is **Ask as a complete chat**, not an idea-capture substitute: durable messages, pane-less supervised assistant host, streamed grounded answers, model selection, assistant filing with verbatim owner input and duplicate checks, PM targeting, permissions/confirmations and unavailable-provider behavior. Architecture section 14 explicitly forbids shipping a store-only composer as this milestone. It does not depend on the herdr-core migration.

Keep runtime migration, remote phone reach, relay/push and classic-UI retirement in their later phases. Phone layouts remain part of the UI contract; later phone packaging is not permission to create a desktop-only product. Keep the sidebar collapsible and available at narrow widths. Retire classic only after actionable parity, not after a successful board connection.
