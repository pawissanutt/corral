# Corral app integration: design handoff

The [app connection contract](CONNECTION-CONTRACT.md) describes the implemented independent client and authenticated native/browser transport boundaries. The [earlier transport proof](TRANSPORT-PROOF.md) is historical feasibility evidence. The [app guide](../../apps/corral-app/README.md) describes current setup and limits.

The app now wires typed reads and engine operations across its current destinations, including the one chat composer. This is implementation status, not a claim that every private journey or the full gate has passed.

Imported from `/home/pawis/corral-app-design/` into branch `app-daemon-integration`, based on Corral commit `6f91ec1175e1ac1edc5ff5a568dd6620e4de824b`. This is a design handoff and next-step recommendation, not an implementation task contract or authorization to run agents, change automation, or install services.

## Contents and authority

- [REQUIREMENTS.md](REQUIREMENTS.md): owner-answered product decisions and requirements. Preserve the one-chat model, assistant sessions in Agents, no idle stop, per-device authority, and full device parity as the target.
- [SESSION-CONTROL.md](SESSION-CONTROL.md): owner decisions of 2026-11-15 for controlling agent sessions from the app (chat, Cancel, Pause, Terminate, PM launch, browser authority). Written in this repository, not imported; it overrides the architecture's session proposals.
- [ARCHITECTURE.md](ARCHITECTURE.md): proposed target architecture, historical source citations, and dated implementation notes. Section 11 records later deviations; section 14 defines delivery phases. Proposed types and paths are not proof they exist.
- [PRODUCT.md](PRODUCT.md): inferred product context for the original static mockup, not an additional owner interview or override of the requirements.
- [mockup/DESIGN.md](mockup/DESIGN.md) and [mockup/index.html](mockup/index.html): visual/interaction reference and its runnable static implementation. The mockup's illustrative data and simulated actions are not daemon capabilities. Open `mockup/index.html` locally; its scripts, styles and fonts are included.
- [mockup/main-loop.html](mockup/main-loop.html): the earlier design of the main loop, which the redesign below replaces (Now, Plans, Agents, Review, project scope, the Project page and Now's Closed section). One file; it reads the brand fonts from `mockup/css/fonts.css` and falls back to system fonts elsewhere. Review file rows show the directory instead of a per-file description because there is no data source for descriptions. Gate rows have no test-progress bar or activity strip because gate logs have no structured progress or per-line times. The Now digest uses coarse change kinds. Phone uses the mock's narrow layout rather than a separately designed phone form.
- [REDESIGN.md](REDESIGN.md) and [mockup/redesign.html](mockup/redesign.html): the redesign proposed after the owner interview of 2026-11-19 (four places, one page per thing, one next step) and its standalone reference. The owner accepted it on 2026-11-19, so it is the design authority for `apps/corral-app`; [REDESIGN-IMPL.md](REDESIGN-IMPL.md) records how the app implements it.
- [NEXT-STEP.md](NEXT-STEP.md): current implementation summary, hosted-infrastructure/share-target blockers and outstanding real-device verification.

The imported proposal remains historical; later owner corrections and implementation notes are maintained here. Originals remain in the external directory. The old architecture backup, Impeccable metadata and screenshot captures were not imported. Screenshot inventories describe the original directory; the runnable mockup is a visual reference, not a capability inventory. App theme corrections are mirrored into its CSS.

Corral's current [daemon guide](../guide/daemon.md), [daemon-first implementation record](../records/daemon-first.md), app [README](../../apps/corral-app/README.md), repo guidance and source describe shipped behavior. They take precedence over historical claims that a component is built or missing; owner requirements still govern the product target. Keep future integration decisions here in the repository rather than relying on edits to the external copy.

## Implementation status

The app remains a separate Cargo workspace and uses an app-local client on `corral-protocol`,
not a standalone `corral-client` crate. It serves the simple engine through the daemon
`corrald-next` (see [simple-daemon-app.md](../plans/simple-daemon-app.md)). Enrolled native Unix
and paired Noise WebSocket clients share typed services and one authority: a paired device is the
owner, and browser and native clients are equal. There are no authority classes, previews, proofs
or command journal. Every engine operation goes out as `op.submit { op, force }` and the engine
decides. A refusal opens a sheet with its reason and one button per typed next step; the forced
step reads "… anyway", says what it skips and is its own confirmation. Drop, Stop, Approve and
land, Retry with a fresh start, a move to Done or Dropped, anything forced, a device revoke and
Forget ask once on the device; nothing else asks. This replaces the per-device authority, the
previews and proofs, and the Inbox and Builds destinations described in REQUIREMENTS.md and
ARCHITECTURE.md (owner decisions of 2026-11-18).

Destinations are Now, Changes, Plans, Agents, Review, Timeline, Usage, Artifacts and Settings.
Now's first group, Needs you, is the old Inbox; Agents holds what Builds held. A plan is a parent
task with children and a Markdown document, with no separate plan review. Every task row, plan
member, review candidate and closed row has one Actions menu with all fifteen engine operations;
every run has a Session menu (Cancel turn, Pause, Resume, Rewind, Terminate, Relaunch, Reply, Focus
pane, then its task's Actions). The artifact finder is one filter over every task's artifacts.
Settings holds Devices, This daemon, Connection, Notifications, Configuration and General.
Configuration edits `corral.toml` through `config.get` and `config.put`: the daemon writes the
file only after the engine's config check passes, and a refusal's reasons land on the field they
name. Unsupported or disconnected data never becomes fixture facts. The sample preview is explicit
and launch-only, runs the same screens against a small local engine and is never restored on
restart. Disconnected screens offer Connect and Preview; local preferences remain in Settings ›
General › This device.

Ask is one composer for the one chat agent; there is no PM/assistant switch. A quick thread can be
ended and using it afterward starts a new session; named sessions have the same transcript.
Conversation, launch and relaunch depend on no automation setting. Acknowledged transactional
outboxes preserve drafts and messages across restarts and concurrent tabs. Saved credentials permit
offline boot from the last complete Tasks, Plans, Projects and Health snapshots marked stale, not
live. Operations and replies are refused offline.

Projects (`projects.v1`, `Topic::Projects`) make a workspace's configured projects visible:
a sidebar project list with Project pages, independent per-screen project filters, and the
project in scope as Ask context, so filed Ideas default to it. A complete empty project
list means no project management: the app offers only Ask, Agents, Usage and Settings.
One app connection still serves one daemon; filters are keyed by daemon store so a later
multi-daemon switcher can keep separate scopes.

Usage totals the engine's usage rows (one per model request, written by `corral usage add` from
agent hosts) per profile, model, task and day, beside run counts from `runs`. Its header says how
many runs the totals cover and that the rest ran on a harness that reports no usage; quota stays
unknown. Days are the device's local calendar days. Timeline reads `runs` and `events`; the burn-up
counts created, landed, dropped and reopened events and does not narrow with the Timeline range.

Pane sessions show pane text and process sessions the tail of `session.log`; there is no raw
terminal for chat sessions. Review defaults to Unified below about 1100 pt of content width.

Opt-in direct LAN/Tailscale binding and browser Web Push are implemented. Remote browsers
need an HTTPS app and operator TLS/WSS (`--web-endpoint`); there is no embedded TLS.
Relay/native push remain blocked on independently hosted infrastructure and publisher
provisioning. Inbound OS share targets are blocked by NSH 0.5.0; the app guide links the
host/packaging evidence. Native desktop global Ask shortcut and tray are implemented,
but unverified on this host.

Verified so far: `cargo test` in the app, builds for the headless, `tui` and `web` targets, and
terminal walks against the sample preview and against a real daemon on a scratch workspace. Parity
with the classic terminal board is tabulated in section 5 of
[simple-daemon-app.md](../plans/simple-daemon-app.md). The GTK desktop and a physical phone have
not been built or tested on this host; phone web layout remains in scope.

The architecture's “Current state” and phases are historical proposals, not a current
inventory. Do not plan the implemented daemon, readers, chat and command router again.
Classic-UI retirement remains separate work. Phone layouts are
already part of the UI contract; later packaging is not permission for desktop-only UI.
Keep the sidebar available/collapsible and automatically folded at narrow widths.
Retire classic only after actionable parity and supported-device verification.
