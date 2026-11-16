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
- [mockup/main-loop.html](mockup/main-loop.html): the adopted design of the main loop implemented by `apps/corral-app` (Now, Plans, Agents, Review, project scope, the Project page and Now's Closed section). One file; it reads the brand fonts from `mockup/css/fonts.css` and falls back to system fonts elsewhere. Review file rows show the directory instead of a per-file description because there is no data source for descriptions. Gate rows have no test-progress bar or activity strip because gate logs have no structured progress or per-line times. The Now digest uses coarse change kinds. Phone uses the mock's narrow layout rather than a separately designed phone form.
- [NEXT-STEP.md](NEXT-STEP.md): current implementation summary, hosted-infrastructure/share-target blockers and outstanding real-device verification.

The imported proposal remains historical; later owner corrections and implementation notes are maintained here. Originals remain in the external directory. The old architecture backup, Impeccable metadata and screenshot captures were not imported. Screenshot inventories describe the original directory; the runnable mockup is a visual reference, not a capability inventory. App theme corrections are mirrored into its CSS.

Corral's current [daemon guide](../guide/daemon.md), [daemon-first implementation record](../records/daemon-first.md), app [README](../../apps/corral-app/README.md), repo guidance and source describe shipped behavior. They take precedence over historical claims that a component is built or missing; owner requirements still govern the product target. Keep future integration decisions here in the repository rather than relying on edits to the external copy.

## Implementation status

The app remains a separate Cargo workspace and uses an app-local client on `corral-protocol`,
not a standalone `corral-client` crate. Enrolled native Unix and paired Noise WebSocket
clients share typed services and the same per-device authority model. Any admin device can
pair/revoke; self-raise is refused, admin grants need device-bound proof, and authority
changes notify active sessions. Confirm/Admin commands require a matching daemon preview
and tap/proof, not biometric confirmation. Unsupported/disconnected data never becomes
fixture facts. Preview uses the same screens with sample data, is explicit and is not
restored on restart. Disconnected screens offer Connect/Preview; local preferences remain
in Settings › General › This device.

The daemon hosts assistant sessions; Ask is one assistant/PM chat composer. Settings
supports model selection and optional `assistant.summary_profile` for quick-thread
compaction, effective at the next assistant-host start. Paid-API summary profiles are
refused without separate spending authority. Acknowledged transactional outboxes preserve
drafts/messages across restarts and concurrent tabs. Saved credentials permit offline boot
from complete last-known Board/Inbox/Plans/Builds/Health snapshots marked stale, not live.
Builds (`builds.v1`) is its own read-only destination for published build-session records;
see the screen map in [mockup/DESIGN.md](mockup/DESIGN.md).
Projects (`projects.v1`, `Topic::Projects`) make a workspace's configured projects visible:
a sidebar project list with Project pages, independent per-screen project filters, and the
project in scope as Ask context, so filed Ideas default to it. A complete empty project
list means no project management: the app offers only Ask, Agents, Usage and Settings.
One app connection still serves one daemon; filters are keyed by daemon store so a later
multi-daemon switcher can keep separate scopes.

Timeline reads use indexed half-open timestamp windows (including owner/PM points),
overlapping recorded sessions, and predecessor phase state. Completed historical
attempts do not consume the window's lifecycle budget. Crossing phases keep their
recorded start and session attribution; events at the exclusive upper bound are not
applied, so an unclosed phase is open as of that bound, not ended at the reader's clock.
Needs-me context includes decision receipts since the last unconditional close:
one answered decision does not imply that every pending decision was answered.
Usage, daily costs, Timeline and burn-up return bounded partial results with completeness,
coverage and unpriced-data notes rather than discarding an entire oversized read. Provider
failures remain explicit; unavailable quota limits, usage, windows and headroom stay unknown.
Usage accounting keeps rebuildable full-history UTC-day summaries in the validated cache
root (`usage-v1`). Task revisions and transcript file identities invalidate saved totals;
pricing overrides and catalog versions invalidate estimates. Closed transcript files are
folded incrementally without a row cap. Only live files are scanned with independent
per-session/PM bounds, so transcript volume cannot suppress task costs. Copied/resumed
responses retain collector deduplication, and undated usage stays undated. Missing,
unreadable or incompatible cache files rebuild from source; incomplete live scans still
report partial coverage. Removing this cache never removes accounting evidence.
Burn-up scopes cover workspace, project and plan and remain cumulative over real task
creation/landing events; the Timeline date range does not narrow their totals.

Private smoke has launched the native terminal and paired packaged Chromium against a private daemon, with matching trusted-side pairing codes and a live seven-task board and connected Settings. It does not establish the full browser journeys or root gate. GTK desktop and a physical phone have not been built or tested on this host; phone web layout remains in scope.

Shared record action tables serve inspector, menus, palette and phone surfaces, with
authority/revision checks and same-ID result recovery. Review defaults to Unified below
about 1100 pt content width. Pane sessions expose acknowledged bounded live terminal
windows (`stream.terminal.v1`); raw keys need Admin and fresh proof, and daemon-hosted
assistants have no raw terminal.

Opt-in direct LAN/Tailscale binding and browser Web Push are implemented. Remote browsers
need an HTTPS app and operator TLS/WSS (`daemon.web_endpoint`); there is no embedded TLS.
Relay/native push remain blocked on independently hosted infrastructure and publisher
provisioning. Inbound OS share targets are blocked by NSH 0.5.0; the app guide links the
host/packaging evidence. Native desktop global Ask shortcut and tray are implemented,
but unverified on this host.

The architecture's “Current state” and phases are historical proposals, not a current
inventory. Do not plan the implemented daemon, readers, chat and command router again.
Runtime migration and classic-UI retirement remain separate work. Phone layouts are
already part of the UI contract; later packaging is not permission for desktop-only UI.
Keep the sidebar available/collapsible and automatically folded at narrow widths.
Retire classic only after actionable parity and supported-device verification.
