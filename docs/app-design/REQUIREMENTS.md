# Corral interaction surface redesign: requirements

Status: owner-answered requirements, 2026-11-11; assistant round added 2026-11-12 (D12 to D24,
R24 to R35). D17 corrected by the owner on 2026-11-15: browser and native have the same
authority. Ask and Idea are one chat; assistants live in Agents and are never idle-stopped.
This is not an implementation/verification inventory or task contract; it grants no authority.
Companions: `README.md` (current state), `ARCHITECTURE.md` (historical design), `mockup/` (visual reference).
Predecessor: `~/corral-board-opus/` (terminal-only board proposal; its model carries over, its
ratatui build does not).

## 1. Why

The owner's verdict on a better TUI: not enough. Every driver below was confirmed.

| Driver | Evidence |
|---|---|
| Work stalls with no board open | Sync runs only in the board, a herdr hook, or `corral sync --watch`. `docs/records/pm-runtime-redesign.md` (Git history) records 4 stalls of 4 to 8.6 h ended only by the owner asking. |
| Owner cannot act away from the desk | No surface outside a herdr pane on the host. |
| Terminal cells limit understanding | "Visualization is not there": no graph, timeline, real diff review, rendered artifacts. |
| Questions arrive without context | The owner cannot decide from what a PM or worker question shows today. |
| Corral should be a product | A TUI inside Herdr TUI is too high a barrier for others. Own use and product, both. |
| Agents on other machines | Wanted; multi-machine scaling required later, not immediately. |
| Provider usage limits | The owner wants to balance usage across providers; only visibility is in scope now. |
| No quick way in | Asking a question or filing an idea needs a herdr pane on the host: `corral pi` refuses `not_in_herdr` elsewhere, and capture is a board popup (`docs/guide/assistants.md:24-59`, `docs/guide/tui.md:398-402`). The first version of this design had no assistant surface at all. |

## 2. Decisions (owner answers)

| # | Topic | Decision |
|---|---|---|
| D1 | Engine | Corral runs as an always-on daemon. It owns sync, autopilot and the PM loop; no UI needs to be open. |
| D2 | Audience | One owner, many devices. Teams are a non-goal. Events keep their actor/via envelope so identity can grow later. |
| D3 | Clients | A custom Corral app, not a Herdr mod. One codebase on NSH (`longbridge/nsh`): desktop (macOS, Linux, Windows), web, iOS, Android, and a terminal target. |
| D4 | Phone scope | Full parity with desktop, including settings, grants, plan work, and live agent terminals inside the app. |
| D5 | Agent session view | Structured transcript first (messages, tool calls, questions, reply box); the raw terminal is one tap away. Same on every device. |
| D6 | Terminal runtime | herdr-core (`hc`) behind a runtime port, installed and supervised by the daemon. A native PTY adapter stays possible. Herdr TUI is the legacy adapter during migration only. |
| D7 | Reach | Direct first (loopback, LAN, Tailscale/SSH). Then an end-to-end-encrypted relay: a dumb pipe plus push sender, hosted by the owner for himself, self-hostable by others. Devices pair by QR. The relay never sees content. No accounts in the first releases. |
| D8 | Home | Phone opens on Inbox ("needs you", then what changed). Desktop opens on the Now workspace (who-acts-next list, inspector, context) with visual views as dockable panels. |
| D9 | Terminal board | The `board-next` ratatui crate is not built. Its model (one-line rows, rail, Changes feed, seen-cursor, BoardView projection) moves into the daemon. The terminal is one more NSH target. Classic `corral-tui` stays until parity, then is retired. |
| D10 | Usage balancing | Visibility only: quota and headroom per provider and profile. The daemon must leave room for a quota-aware router later; none is built now. |
| D11 | First slice | Daemon owns sync/autopilot. First client slice is "Needs you" with full question context, on desktop and web from one codebase. Phone packaging follows. |
| D12 | Quick assistant | The app has an assistant the owner can open at once to ask a question or send an idea. Default answerer is an assistant session (the existing `A-<n>` role), run by the daemon with no pane. A switch in the composer sends the message to the PM instead. |
| D13 | One chat | Asking and sending an idea are the same act: a message to the agent. There is one text field and one Send, no mode. The assistant answers, or files the Idea itself with a title, project, criteria and a duplicate check, keeping the owner's words. Supersedes the earlier "instant capture, refine after" answer; its one surviving part is that a message is never lost when no model can run. No daily cap on this work. |
| D14 | Assistant authority | Same as the pane assistant today: read, file, and owner actions on a clear instruction. Confirm-class and high-risk actions need confirmation on the device. |
| D15 | Entry points | In the app (one key or tap from anywhere); "Ask about this" on any item; a desktop global hotkey that works with the main window closed; the phone share sheet; `corral ask` in a terminal. Not now: home-screen widget, lock-screen shortcut, voice. |
| D16 | Conversation | One rolling quick thread per workspace, shared by all devices, compacted automatically. "New session" starts a separate one for bigger work such as co-work. |
| D17 | Devices that may instruct | Browser and native have the SAME authority, gated by each device's `reply`, `act`, `assistant`, `assistant_act`, `admin` flags. Any admin device pairs/revokes and edits authority; admin grants require device-bound proof, self-raise is refused, and active sessions receive authority-change notifications. |
| D18 | Instructions that need a tap | Land/approve, plan acceptance, revert and finish require device confirmation; dispatch, queue, park and verdicts run on the owner's word. Implemented command classes are canonical in `corral-protocol/src/cmd/mod.rs::Command::app_class`: Confirm also includes session termination and permission revoke; Admin includes config, mode changes and raw keys. Confirm/Admin require a fresh matching daemon preview and device-bound proof, not an asserted biometric check. |
| D19 | Where the assistant lives | Asking is a chat to an agent, so there is no Assistant destination: assistant sessions are listed in Agents, the quick thread first. Phone tab bar: Inbox, Now, Agents, Plans, More. Ask stays one tap away from every screen without owning a tab. |
| D20 | Host lifetime | The assistant is not stopped for being idle. It runs until its work is done or it stops on its own. |
| D21 | Who filed it | An idea filed by an assistant is marked as assistant-filed on both paths: from the app and from an assistant opened in a pane. |
| D22 | Model | The quick thread uses the assistant profile for now. The composer has a quick model selector over a configurable list of models; switching keeps the conversation. |
| D23 | Thread roll | One summary call may use optional `assistant.summary_profile`; unset uses the session model. Implemented selection applies at the next assistant-host start. Paid-API summary profiles are refused without separate spending authority. |
| D24 | Write grants from a phone | Standing write grants (one hour, this session) can be given from a phone with a fresh device confirmation; the implemented check is the owner's tap plus daemon-validated device-bound proof, not Face ID/PIN. |

## 3. Functional requirements

### 3.1 Daemon

- R1. Runs with no client attached, across client crashes and upgrades; restarts cleanly after reboot under a service manager.
- R2. The only process that drives sync, admission, autopilot, gates, review and landing. Existing authority rules (Assist/Autopilot grants, holds, owner-only settings, candidate freshness) are unchanged.
- R3. Health is visible: a dead or degraded daemon, runtime, or PM must show as such in every client, never as a stale board.
- R4. Must not die of its own bounded stores. Known caps (receipts 16,384, generations 64, bridge input keys 4,096, budget requests 16,384) need eviction or rollover; a design that still lets `corral pi --pm` refuse with `hello_refused: receipt_limit` at startup is insufficient.
- R5. `corral <verb>` keeps working for agents, the PM and scripts. Attribution (`actor`, `via`) stays correct for every surface, with new `via` values for the app and each device.
- R6. Agents run on the daemon's machine now. The design must not block several machines later (herdr-core machines over SSH).

### 3.2 Clients

- R7. One codebase; each device in its own form (phone sheets and tab bar, desktop panels, terminal cells). No feature exists on only one device, except where the platform cannot do it.
- R8. Nothing slow blocks input. Clients render a precomputed projection and receive deltas; detail loads lazily.
- R9. Works offline for reading the last known state; actions queue or refuse visibly, never silently.
- R10. Connection state, paired devices and the reach path (direct or relay) are always inspectable.
- R11. Push notifications on phone for: needs you, plan awaiting acceptance, stuck, escalated, daemon unhealthy. Kinds are configurable.

### 3.3 Attention and questions

A question (PM or worker) must be decidable on one screen. Required on that screen:

- R12. Each option with its consequence: what happens next, what it unblocks, what it costs.
- R13. Place: the task's goal, its position in the plan graph, what is waiting on the answer.
- R14. Evidence inline: the diff hunk, file excerpt, gate log tail or screenshot the question is about.
- R15. A thread: ask the asker a follow-up before answering.
- R16. History: what was already tried and the owner's earlier related decisions.
- R17. One answer wins across devices and the PM pane; the others show it answered.

Not required: the transcript at the moment of asking as a separate view (the session is one tap away), voice-specific answering (system dictation suffices).

### 3.4 Visualization

- R18. Plan/task dependency graph with live state: what blocks what, per-member rail.
- R19. Run timeline: who worked when; gate, review and land phases; idle gaps and stalls.
- R20. Live agent wall: what each agent is doing now, on which machine.
- R21. Usage: cost and quota per provider and profile over time, with limit headroom.
- R22. Code review: side-by-side or unified diff, inline comments, rendered reports and screenshots, accept or request changes.
- R23. Goal progress: burn-up per goal or project.

### 3.5 Carried over from the Opus board proposal

Default screen answers "what changed, and who moves next". One line per thing with fixed anatomy.
Group by who acts next (You, Corral, Workers, Queued, Held, Parked). The rail
`run · gate · review · land` with a distinct stale glyph. Collapse repetition, never information.
Goals are context and a filter, not a tab. Deltas since last visit are first class. The vocabulary
table (`needs you`, `gating`, `held · reason`, never bare `ready` or `blocked`).

### 3.6 Assistant and capture

Names: the composer is **Ask** (one field, one Send, no modes); its target is **Assistant** or
**PM**; threads and sessions are listed in **Agents** (D19); the rolling thread is the
**quick thread**.

- R24. Opens at once. One key or tap from any screen; on desktop a global hotkey opens a small
  window even when the main window is closed. The composer accepts typing before any network or
  model work finishes.
- R25. A message is never lost. The daemon stores it and every device shows it before any model
  runs. Offline it queues visibly on the device and is delivered exactly once; a message delivered
  late can be answered and filed but cannot instruct an action. When the assistant cannot run
  (host down, machine load, provider quota), the message waits in the thread with the reason, and
  the owner can file it as an Idea as written, with no model.
- R26. The assistant files ideas itself. When a message is an idea it creates the Idea with a
  title, project, labels and acceptance criteria, keeps the owner's words verbatim in the body,
  and checks for duplicates first; when one is likely it asks before filing. The thread shows what
  was filed, with edit, merge and discard. Acceptance criteria can make the Idea eligible for
  promotion (`docs/guide/automation.md:665-667`). Changing an Idea the owner filed himself still
  needs his acceptance (`docs/guide/sprint-review.md:60-65`).
- R27. Answers are grounded and streamed. Every task, plan, run, file or record an answer relies on
  is a link into the app. The owner can stop an answer.
- R28. Context travels with the question. "Ask about this" attaches the item on screen as a
  removable chip; the assistant receives a bounded brief of it, as `corral pi --task` does today.
- R29. The PM is one switch away. A message sent to the PM never blocks supervision: it is
  delivered when the PM next takes input, the composer shows PM state (`live`, `relaunching`,
  `absent`), and the reply returns to the same thread.
- R30. Authority is unchanged. A message from a paired device is owner input with the device as
  its provenance; an owner action cites it exactly as it cites pane input today. The app never
  exceeds the CLI; confirm-class and high-risk actions wait for a fresh confirmation on the device.
- R31. One quick thread per workspace on every device, plus named sessions. Sessions opened in a
  pane with `corral pi` appear in the same list; a session has one live host at a time.
- R32. What the assistant asks reaches the owner. Its `ask_user` questions and write-permission
  prompts are answerable in the thread, and in Inbox when the owner is elsewhere. One answer wins.
- R33. Cost is visible per ask and per session, and counted in Usage.
- R34. An answer survives the client. Closing the app or losing the connection does not stop it; a
  push "assistant answered" is a configurable kind, off by default.
- R35. `corral ask "<text>"` does the same from any terminal on the host. With the daemon down it
  refuses and names `corral new --idea`, which needs neither daemon nor model.

## 4. Non-goals

- Teams, roles, guests, accounts, billing.
- A hosted Corral cloud that runs agents or stores board content.
- A quota-aware router or cross-provider scheduler.
- Replacing agent harnesses (OmO, oh-my-pi, Claude Code) or building a chat model runtime.
- A Herdr app mod as the primary surface.
- Weakening gates, independent review, holds or owner-only settings to make remote use easier.
- A general chat product, a voice assistant, or a second PM. The assistant reads and files; it acts only on the owner's instruction.
- Home-screen widget and lock-screen capture (deferred, not rejected).

## 5. Constraints

- Rust 2024, MSRV 1.94, 800 SLOC module ceiling; dependencies flow `corral -> {tui, ops} -> {core, herdr, git}`.
- The SQLite store is canonical; one write path (`StoreBackend` operations), history-independent hot paths.
- Session ownership rules (`docs/session-ownership.md`): never act on panes or terminals corral does not own; never start a runtime server the owner did not configure, except the one the daemon is explicitly given to supervise.
- Shared host: no load generation; tests never touch the owner's live runtime.
- Remote control of a machine that runs code is a high-value target: pairing, device revocation and end-to-end encryption are requirements, not polish.

## 6. Risks named during grilling

| Risk | Note |
|---|---|
| NSH maturity | 2.5k stars, 148 open issues, young. It does ship the needed widgets (`Terminal`, `DiffView`, `TextView`, `Plot`, `DataTable`, `DockArea`, `Command`, `Tree`). Keep the client a thin renderer over a protocol so the framework is replaceable. |
| herdr-core maturity | v0.1.x, 160 open issues, Apache-2.0. The runtime port is the hedge. |
| Two Herdr runtimes | Herdr TUI and herdr-core share no agents. Migration must be per-run, never a flag day for live work. |
| Parity on phone | Full parity is the stated goal; plan authoring and settings on a phone are the costliest part. |
| Scope | Daemon, protocol, app, relay, runtime migration. The first slice (D11) is deliberately small. |

## 7. Open for the architecture

Process model and single-writer story; protocol (transport, schema, versioning, subscriptions);
projection, revisions and per-device seen-cursors; command path and attribution; device identity,
pairing, revocation; relay and push; runtime port and the Herdr TUI to herdr-core migration;
terminal and transcript streaming; alignment with the PM runtime redesign's loop owner;
client structure in NSH (Rust, TSX or mixed); usage data sources; phasing.

Added with section 3.6: how an assistant host runs without a herdr pane; how device input becomes
journaled owner input; the quick thread's identity, lifetime and compaction; how a message survives
with no model (offline queue, late delivery, filing as written); delivery of a message to the PM; streaming of an
answer; where `ask_user` and write-permission prompts surface; the global-hotkey window and share
sheet on NSH.
