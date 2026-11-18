# Simple daemon and app: phase 2 plan

Phase 2 of [simple-engine.md](simple-engine.md): the daemon, the wire protocol and `apps/corral-app` move onto the new engine. Work happens in `.worktrees/simple-app`, branched from phase 1. Names, statuses, operations and force rules come from the contract and are not repeated here. Paths are relative to the repository root; `app/` means `apps/corral-app/src/`.

## Kept owner decisions

- The app can force anything: every engine operation is on every card and session, and the forced form is offered when the normal form is refused.
- Browser and native clients have the same authority. A paired device is the owner: authority classes, previews and proofs are removed, and destructive actions ask once (confirmed 2026-11-18).
- A plan is a parent task with children and a Markdown document. There is no separate plan review (confirmed 2026-11-18).
- Inbox merges into Now and Builds merges into Agents (confirmed 2026-11-18; section 4).
- Ask and Idea are one chat. The transcript shows what the terminal UI shows. A quick thread can be ended; using it afterward starts a new session. The assistant's conversation, launch and relaunch depend on no automation setting.
- `nsh run --target tui` launches the terminal UI. The sidebar stays, is collapsible and folds when narrow. The new UI applies in all modes.
- The pairing page is real and pairing survives a refresh. Dates are real. Usage says what its partial totals represent and why records are missing.
- `crates/corral-tui` is retired once the app's `tui` target reaches parity (section 5).

## Gaps closed here (simplest choice; change by owner decision)

1. Commands are answered on the connection. The idempotency journal, `cmd_id` replay and `Expect::TaskRevision` (`crates/corral-protocol/src/cmd/mod.rs:262-282`) go; the engine's status rules refuse a stale action.
2. The CLI never routes through the daemon. `cmd.submit`, `Caller` and `CallerEnv` (`cmd/mod.rs:270,400,417`) go; the daemon notices CLI changes by polling `max(events.seq)` once a second.
3. Device records move from the old record store (`crates/corral-daemon/src/devices.rs:28`) to `devices.json` in the daemon state root, beside the Noise identity (`crates/corral-daemon/src/identity.rs:17`).
4. Settings edits are `config.get` and `config.put`: the daemon writes `corral.toml` after the engine's `config check` passes. This is not an engine operation.
5. Chat is served in phase 3. Phase 2 leaves the chat wire types and chat screens untouched and does not advertise `assistant.v1` until the pi host lands ([simple-pi-host.md](simple-pi-host.md)).

## 1. Protocol

### Commands

One method, `op.submit`, replaces `act.preview`, `act.submit`, `act.result` (`crates/corral-protocol/src/app.rs:7-9`), `cmd.submit` and `cmd.result`. The protocol carries `corral_types::{Op, Next, Refusal, Task, Run, Check, Blocked, Usage}` directly, from the wasm-safe leaf crate `crates/corral-types`; it defines no copies of them and no mirrored row structs. It adds only the envelope:

```rust
// corral_types: Op { New, Edit, Comment, Move, Dispatch, Reply, Stop, Retry, Done, Gate, Review, Verdict, Changes,
//     Approve, Drop, Reopen, ArtifactAdd, UsageAdd, Sync }; Next { op: Op, force: bool }; Refusal { reason, next: Vec<Next> }
pub struct OpSubmit { pub op: Op, pub force: bool }          // force defaults to false
pub enum OpReply { Done { seq: u64, task: Option<String> }, Running { seq: u64 } }
```

- **Force** travels as `OpSubmit.force`. The daemon passes `op` and `force` unchanged to `Engine::apply(op, force)`, the single entry point. The engine reads `force` only for `move`, `dispatch` and `approve`.
- **Refusal** travels as the JSON-RPC error `refused` (`crates/corral-protocol/src/rpc.rs:57`) with the `Refusal` in `error.data` (`rpc.rs:43`). `Blocked { reason, detail, next }` rides on task rows unchanged. Either `next` is a list of `Next`; the app renders one button per entry, the first as the recommended one, and a pressed button submits that entry's `op` and `force`.
- `approve`, `gate` and the pipeline can take minutes. The daemon answers `Running` after 5 s; the outcome arrives as events and the task's new status. A late failure is a Blocked card, never a lost error.
- The daemon calls the engine as `Actor::Owner` for every local or paired caller.

### Command families (`crates/corral-protocol/src/cmd/`, enum at `cmd/mod.rs:57-83`)

| Family | Decision | Engine operation, or what the app shows instead |
|---|---|---|
| `task` | keep | New, Idea → `new`; Comment, Edit, Move, Dispatch, Retry, Reply → same name; Route → `edit` (profile, kind, size); Focus → pane focus (section 5). Hold, Release → `move` to Backlog or Ready; Unstick → `stop` then `retry`; WaiveReview → `approve`; RecoverInfrastructure → `gate` or `retry`. Revert drops: the task page shows the landed sha and `reopen` |
| `review` | keep | Approve → `approve`; RequestChanges → `changes`; Verdict → `verdict`; Discard → `drop` |
| `session` | reshape | Send → `reply`; Stop → `stop`; Relaunch → `retry`. Keys stays as a pass-through to the herdr pane (`crates/corral-herdr/src/lib.rs:255`). Cancel, Pause, Resume, PmOpen drop: chat sessions get input and cancel in phase 3 |
| `config` | reshape | Set, ProfileAdd, ProfileRemove → `config.put` |
| `question` | drop | A question is a Blocked card with `detail`; Answer, AnswerGroup, Followup → `reply`. Escalate, Cancel, Deadline have no successor |
| `queue` | drop | Add → `move` to Ready; Remove → `move` to Backlog; Admit → `dispatch` with force; Pause, Resume → the `auto_dispatch` switch |
| `attention` | drop | Park, Unpark → `move` to Backlog plus `comment`; Attend has no successor |
| `plan` | drop | Accept → `move` each child to Ready; RequestChanges → `comment` on the parent; AddMember, MoveMember → `edit` parent; Cancel → `drop` the parent; Policy has no successor |
| `annotation` | drop | Review comments become one `changes` note that quotes file and line |
| `mode` | drop | Two switches per project, `auto_dispatch` and `auto_land`, plus `limits.running`, all in Settings |
| `assistant` | drop | Create, Summary → chat session start in phase 3; PermissionsRevoke has nothing to revoke |
| `message` | drop | FileAsIdea → `new` as Idea with the message text; Withdraw removes an unsent draft locally |
| `pm` | drop | No PM role. The assistant runs owner commands; its notes are `comment` |
| `inbox` | drop | PM inbox plumbing; nothing shown |
| `knowledge` | drop | Not in the engine; nothing shown |
| `meta` | drop | Pins, dashboards and artifact tiers; the artifact finder replaces the shelf |
| `repair` | drop | A red gate sends work back; the owner uses `gate`, `retry` or forced `approve` |
| `run` | drop | Wait → CLI `wait`; Decision, Progress, Checkpoint, CriteriaRegister → `comment` |
| `setup` | drop | `corral-next init` and the Settings config editor |

New on the wire because the engine has them: `done`, `gate`, `review`, `stop`, `drop`, `reopen`, `sync`, artifact add, usage add.

### Reads and topics (`subscription.rs:14-31`, `detail.rs:29-92`, `read.rs:10-18`)

| Today | Decision | New source |
|---|---|---|
| Method `read` with `PmNext`, `PmBrief`, `AgentPmBrief`, `AgentPmNext`, `ClassicFrame` | drop | Agents use CLI `list` and `show --json`; the classic frame dies with the classic board |
| Topic `health` | reshape | Daemon state, runner reachable, last `sync` time and error. PM state (`health.rs:25`) goes |
| Topic `board` (`board.rs:5-30`) | reshape to `tasks` | One row per open task, holding `corral_types` values as they are: the `Task` with its `Blocked`, the live `Run`, the newest gate `Check` for HEAD and review `Check` for the round, and whether `ready` was appended. The app derives groups and the rail |
| Topic `classic` | drop | — |
| Topic `inbox` | drop | Now's Needs-you group reads `tasks` |
| Topic `plans`, detail `plan` | reshape | Parent tasks with child counts by status; detail is the parent, its `body`, the children and their `depends_on` edges |
| Detail `plan_diff`, `question`, `build`, `filings`; topic `builds` | drop | No plan revisions, questions or build records; filed ideas are `created` lines in Changes |
| Topic `projects` | keep | `[projects.*]`, open-task counts and up to 100 local branches in tip committer-date order |
| Topic `agents` | reshape | Live `runs` with `SessionStatus`, `quiet_since` and the Pi host's pause flag where available |
| Topics `assistant`, `thread`, `session`; details `session`, `session_item`, `attachment`; `assistant.*`, `attachment.*`, `prompt.answer` | keep, unchanged | Served by phase 3 over the per-session socket |
| Topic `terminal`, detail `terminal` | keep | Pane text for herdr sessions (`crates/corral-herdr/src/lib.rs:288`); tail of `session.log` for process sessions |
| Topic `usage`, detail `usage_series` | reshape | Section 4, Usage |
| Topic `settings` | reshape | Parsed `corral.toml`, listener, devices. Mode and grants go |
| Detail `task` | reshape | `Task`, runs, checks, artifacts, last events |
| Details `review`, `review_file` | keep | The diff and summary stored at `done`, plus checks |
| Details `timeline`, `burnup` | keep | `runs` and `events` |
| Detail `artifact` | keep, plus list | Blob by ref; new `artifacts` list with a text filter for the finder |
| Detail `closed`; `changes.page`, `seen.advance` | keep | Done and Dropped tasks; `events`; the seen cursor stays in the device record |
| `hello`, `ping`, `nudge`, `health.get`, `daemon.stop`, `sub.*`, `device.*`, `pair.*`, `push.*` | keep | Unchanged, except authority (section 2) |

## 2. Authority

Goes:

- Protocol: `ActClass` (`app.rs:67-75`), `Command::app_class` (`cmd/mod.rs:148`) with `cmd/app_class_tests.rs`, `DeviceAuthority` and `DeviceSetAuthority` (`app.rs:43-55`), `AppSession.authority` (`app.rs:60`), `ActPreviewRequest`, `ActPreview`, `ConfirmProof` (`app.rs:78-100`), `ActSubmit.confirm` (`app.rs:110`), `device.authority_preview` and `device.session_authority_changed` (`device.rs:17-18`), `DeviceAuthorityChange`, `SessionAuthorityChanged` (`device.rs:124,135`), `prompt.preview` (`assistant.rs:18,154`), `ErrorCode::ConfirmRequired` (`rpc.rs:58`), `Principal` and `ClientBuild`.
- Daemon: the class policy, previews, proofs and authority edits in `commands/app.rs` (`:39,137,164,184`), their routes (`app/dispatch.rs:179,217-226`), the admin gate on pairing and revoke (`app/dispatch.rs:232`), the `authority` fields of a device record (`devices.rs:46-50`), `commands/app_authority_tests.rs`, `devices_authority_tests.rs`.
- App: the preview and proof lifecycle (`app/state/confirmation.rs:20-88`), the authority editor (`app/model/settings/authority.rs`, whole file) and its Settings panel, `Confirmable::DeviceAuthority` (`app/model/act.rs:261`), `authority_reason` (`app/model/work/verbs.rs:429-443`), the class on every action (`verbs.rs:191`), the simulated previews in `app/model/preview/effects.rs`.

A destructive action asks once, locally: one sheet with the operation, its arguments and, for a forced one, what it skips; one Confirm; then `op.submit`. No daemon round trip, expiry or proof. It applies to `drop`, `stop`, `approve`, `retry` with `fresh`, `move` to Done or Dropped, anything forced, device revoke and Forget. Everything else runs on one press.

Pairing keeps: the Noise session and pairing handshakes (`device.rs:29-30`), single-use offers (`crates/corral-daemon/src/pairing.rs`), the per-workspace identity, the device store (name, platform, key, enrolled, revoked, last seen, reach, push registration, seen cursor), revocation that closes live sessions (`device.rs:33`), peer-UID trust on the Unix socket and Web Push. Any attached device or the local socket confirms a pairing. The browser keeps `corral.paired` and its encrypted credentials (`app/client/mod.rs:31`), so a refresh stays paired.

## 3. Daemon (`crates/corral-daemon`)

It depends on `corral-engine`, `corral-protocol`, `corral-herdr`; `corral-core` and `corral-ops` leave `Cargo.toml` (`:13-14`). It gets its own binary `corrald-next` (`run`, `status`, `stop`, `pair`), because the old `corral daemon` entry is deleted with `crates/corral`. The engine exposes read functions for every table, so the daemon never writes SQL.

| Stays (imports change only) | Rewritten | Goes |
|---|---|---|
| `server.rs` (socket, hello, framing), `conn.rs`, `app/mod.rs`, `app/ws.rs`, `app/session.rs`, `pairing.rs`, `identity.rs`, `devices_reach.rs`, `devices_push.rs`, subscription cursors and paging in `topics.rs` and `topics/app.rs`, nudge and wait in `state.rs` | `views.rs`, `views/rows.rs`: read models over the engine's read functions for `tasks`, `runs`, `checks`, `artifacts`, `events`, `usage`. `devices.rs`: file store, no authority. `health.rs`. `lib.rs`: the loop below. `topics/app/terminal.rs`. `app/dispatch.rs`: routes. New `ops.rs` (pass `op` and `force` to `Engine::apply`, return its `Refusal` as the `refused` error) and `config.rs` | `commands.rs`, `commands/app.rs`, `commands/assistant.rs`, `commands/journal.rs`, `classic.rs`, `reader.rs`, `projector.rs`, the pass modes and pacing in `lib.rs:43-81,266-311`. `assistant.rs` and `topics/app/session.rs` leave the build and return in phase 3 |

One loop thread owns an engine handle and calls `sync`:

- **On a nudge**: the wire `nudge` (`server.rs:348`). The herdr plugin hook sends it today (`crates/corral/src/cmds/session_cmd.rs:24-26`). The new CLI's `hook` takes that role: it nudges the daemon when one is running, else runs one `sync` pass itself, and always exits 0. `corral-herdr` has no event stream, so "herdr events" means this hook.
- **After every `op.submit`**, so a freed slot or a new Ready task is acted on at once.
- **On a timer**: every 30 s, a constant.

`advance(task)` is called directly after an `op.submit` of `done`, `verdict` or `gate`, on a worker thread with its own engine handle, so a gate run never blocks the socket. `approve` runs the same way. The pipeline lock makes overlap with a CLI-started pipeline safe, and `sync` step 4 resumes anything stalled. After each engine call, and whenever the one-second poll sees a higher `events.seq`, the read models refresh and open subscriptions flush.

## 4. App destinations (`app/model/nav.rs:33-54`)

| Destination | Decision | Reads |
|---|---|---|
| Inbox | merge into Now | Needs-you is Now's first group. The `i` key and the phone's first tab open Now at that group |
| Now | keep, home | `tasks`. Groups by who moves next: You (Blocked; Review with `ready`), Corral (Review in gate or review), Workers (Running), Queued (Ready), Backlog and Ideas. A Blocked row shows `reason`; its panel shows `detail` and the `next` buttons. Closed lists Done and Dropped |
| Changes | keep | `events`; a `forced` line names what was skipped |
| Plans | keep | Parent tasks. Document is the parent's Markdown `body`; members are the children laid out by `depends_on`; Path to done is the children not yet Done. Proposal, revision and plan-reviewer panes go |
| Agents | keep | Live runs: role, profile, model, status, quiet time, terminal. Chat, the quick thread and assistant sessions arrive with phase 3 |
| Review | keep | Review tasks: diff and summary from `done`, gate check for HEAD, review check for the round, round n of `max_rounds`. A reviewer session persists across the rounds of a task: the engine re-requests review from the live reviewer session. Phase 3 adds Rewind to the pi session menu |
| Timeline | keep | `runs` (start, end, role, outcome) and `events`; burn-up from `created`, `landed`, `dropped` and `reopened` |
| Usage | keep, reshape | The engine's `usage` rows (one per model request, written by `corral usage add` from agent hosts), totalled per profile, model, task and day, beside run counts and durations from `runs`. The header reads "covers N of M runs" and says the rest ran on a harness that reports no usage. Quota stays unknown |
| Builds | merge into Agents | A build session is a worker without a task (phase 3); landed work shows in Changes |
| Settings | keep | Config editor (limits, defaults, the two automation switches, profiles, projects with gate lines and reviewer, routing, artifact globs, harnesses), devices and pairing, push, connection, this device. Authority editor, modes and grants go |

The `tasks` topic and plan child rows expose `resolved_profile`, the worker profile
dispatch selects with no override. An assistant row in `Waiting` exposes the `question_id`
of the pending prompt it shows. `usage_series` accepts an optional IANA `time_zone` for
day boundaries, falling back to the supplied `utc_offset_minutes` when no zone is given.
`app.session` reports `local` for Unix and loopback peers so the app can offer pane focus
without treating remote owners differently for authorization.

Where the actions sit:

1. **Row menu.** Every task row has one Actions menu (the `…` button, right-click, long-press, `.` in the terminal): Now rows, Plans member cards, Review candidates, Project page rows, Closed rows and the task page header. It is the existing shared table (`app/model/work/verbs.rs:288-314`), rebuilt to list all operations in a fixed order: Edit, Comment, Move to…, Dispatch…, Reply, Mark done, Run gate, Start review, Record verdict, Request changes, Approve and land, Retry…, Stop, Drop, Reopen. The palette lists the same for the selected task.
2. **No client-side rules.** The app stops predicting refusals (`verbs.rs:359-427`). Every item is enabled; the engine decides.
3. **Refusal sheet.** A refused operation opens a sheet with `reason` and one button per `next` entry, the first one primary. An entry with `force` is the "anyway" form ("Dispatch anyway", "Move anyway", "Land anyway"); it lists what it skips and is the single confirm. The Move picker lists all eight statuses.
4. **Blocked card.** One button per `next` entry sits on the card panel; when the first entry is a reply the reply box is shown in place.
5. **Review decide bar.** Approve and land, Request changes, Run gate, Start review. "Land anyway" appears when the gate check is red or Approve was refused.
6. **Agents.** Each session row and the session page header have a menu: Reply, Stop, Retry, Focus pane, then Task ▸ with the full row menu of that session's task.

## 5. Classic TUI parity (`crates/corral-tui/src`)

| Classic board | App today | Left to do |
|---|---|---|
| Capture (`n`, `c`; `board/input.rs:139`, `overlays.rs`) | Covered: Ask, and a New task form with title, body, project (`app/model/work/verb_forms.rs:46-50`) | Add priority, kind, size, parent, depends-on, Idea or Ready |
| Dispatch sheet with profile choice (`d`; `board/input.rs:110`, `choice.rs`) | Missing: dispatch sends no profile (`app/model/work/verb_build.rs:27-34`) | Sheet with the profile list, base and the resolved default |
| Reply (`r`; `board/input.rs:97`) | Covered: chat input (`app/model/agents/live_reply.rs:165`) and Inbox answers | Rewire to `reply`; reply box on Blocked cards |
| Review and merge (`review.rs:455-479`) | Covered: decide bar (`app/model/review/decide.rs:115-123`) | Rewire; add Run gate, Start review, Land anyway |
| Artifact finder (`A`; `board/input.rs:160`, `artifacts.rs`) | Missing: artifacts show per task only (`app/model/work/inspector.rs:80`) | Finder over all artifacts, opened from the palette and the sidebar |
| Settings (`settings.rs:423-430`: General, Projects, Profiles, Harnesses, Automation, Artifacts) | Part: config keys and profiles (`app/model/settings/live.rs:286`, `profiles.rs:164-191`) | Projects, routing, globs, harnesses, the two switches |
| Task detail: runs, events, comments (`detail.rs`, `detail_runs.rs`) | Covered: task page (`app/model/work/task_page.rs`) | Re-read from the new detail |
| Move and drop a card (`H`, `L`, `X`; `board/input.rs:129-131`) | Covered: Move, Discard (`verbs.rs:93,103`) | Rewire to `move`, `drop` |
| History and search (`history.rs`, `search.rs`) | Covered: Closed (`app/model/work/closed.rs`), palette | — |
| Jump to the agent's pane (`a`; `board/input.rs:87`) | Missing | Focus pane; local herdr sessions only |
| Recover and unstick sheets (`recovery.rs`, `detail/unstick.rs`) | Replaced by force, `stop` and `retry` | — |
| Pin shelf, gallery, tour, operating view, `corral pi` launcher | Not carried: the finder replaces shelf and gallery; the launcher is phase 3 | — |

`crates/corral-tui` is deleted in phase 4, once every row is covered on `nsh run --target tui`.

## 6. Steps

UI rule: non-UI agents never edit `apps/corral-app`. Every app step is UI work and goes to `ui-easy` or `ui-hard`. The app is its own Cargo workspace; run one build at a time and only the headless, `tui` and `web` targets.

| # | Step | Touches | Check | Agent |
|---|---|---|---|---|
| 1 | Protocol cut: depend on `corral-types`; the `OpSubmit` and `OpReply` envelope; rows, details and topics that carry its types; delete `cmd/`, the authority types, `read.rs`, `classic.rs` | `crates/corral-protocol` | `cargo test -p corral-protocol`; a wire round-trip test of `Refusal` and `Blocked` with their `next` lists; `rg ActClass crates/corral-protocol` is empty | `task-hard` |
| 2 | Daemon core: dependency swap, `corrald-next`, the loop, `ops.rs`, file device store, authority removal, delete the modules that go; `hook` nudges a running daemon | `crates/corral-daemon`, `crates/corral-next` (`hook`) | `cargo test -p corral-daemon`: over a socket with a fake `Runner` and a temporary repository, new, dispatch, done and approve land a change; a refused dispatch returns `refused` with `next`, and the same call with `force` succeeds and appends `forced`; a revoked device is closed; devices survive a restart; `hook` nudges a running daemon and syncs itself without one; `cargo tree -p corral-daemon` shows neither old crate | `task-hard` |
| 3 | Read models, topics, details, `changes.page`, artifact list, terminal, `config.get` and `config.put` | `crates/corral-daemon/src/{views,topics,config}*` | One test per topic: an engine call produces a delta; a Blocked row carries `reason`, `detail`, `next`; `config.put` with an unknown profile is refused and leaves the file unchanged | `task-hard` |
| 4 | App base: client and feed on the new wire, the shared Actions menu, refusal sheet, single confirm, removal of previews, proofs and the authority editor | `app/client`, `app/state`, `app/model/{act,daemon,work,settings/authority}*`, `app/ui` | App `cargo test`; a headless test that a refused dispatch shows its forced `next` entry as a button and sends `force`; `rg 'ActClass\|ConfirmProof' apps/corral-app/src` is empty | `ui-hard` |
| 5 | Now with Inbox merged, Blocked cards, capture fields, dispatch sheet with profile choice; remove the Inbox and Builds destinations | `app/model/{work,inbox,builds,nav}*`, `app/screens/{now,inbox,builds}*`, `app/shell` | On `tui` against a private daemon on a scratch repository in `process` mode: capture, dispatch with a chosen profile, reply to a Blocked card; sidebar folds when narrow | `ui-hard` |
| 6 | Review and Plans | `app/model/{review,plans}`, `app/screens/{review,plans}` | On `tui` and `web`: approve lands; with a red gate "Land anyway" lands after one confirm; a parent task shows its document and members | `ui-hard` |
| 7 | Agents: live runs, session menu, terminal, Focus pane | `app/model/agents`, `app/screens/agents` | On `tui`: stop and retry from a session row; the terminal shows `session.log` | `ui-hard` |
| 8 | Changes, Timeline, Usage | `app/model/{timeline,usage}`, `app/screens/{changes,timeline,usage}*` | Real dates; Usage header states coverage and the reason for missing records | `ui-easy` |
| 9 | Settings: config editor, devices and pairing without authority | `app/model/settings`, `app/screens/settings` | On `web`: pair, refresh, still paired; revoke from another device; edit a project gate line and see it in `corral.toml` | `ui-hard` |
| 10 | Artifact finder | `app/model`, `app/screens` (new `artifacts`), palette | On `tui`: find a report from another task and open it | `ui-hard` |
| 11 | Parity pass: walk section 5 and the phase 2 "done when" on `tui` and `web`; fix gaps; update `apps/corral-app/AGENTS.md`, `README.md` and `docs/app-design/README.md` | `apps/corral-app`, `docs/app-design` | Every row of section 5 ticked; every forced form driven once | `ui-hard` |

Order: 1, then 2, then 3. Step 4 needs 1 and a running 3. Steps 5 to 10 follow 4 and may run in parallel, because each owns its destination directories; step 5 alone edits `nav.rs` and the shells.

## Risks and what could not be determined

- **The engine was still being written.** Only its foundation existed: `Refusal` implements `Error` (`crates/corral-engine/src/error.rs:6-21`), so `downcast_ref` recovers it, and `Engine::open(root)` (`crates/corral-engine/src/lib.rs:59`) allows one handle per thread. `crates/corral-types` and `Engine::apply` were not in the tree yet. Unknown: whether `sync` and `advance` are safe from two handles at once.
- **Chat spans phases.** Phase 2 removes `assistant.rs` and `topics/app/session.rs` from the daemon build and phase 3 brings them back, so both branches touch the same files. Phase 3 owns them; phase 2 must not reshape the chat wire types.
- **Outside the engine's operations:** config writes, pane keys and pane focus are daemon pass-throughs. Revert has no successor.
- **Lost connection during a command.** With no journal, the app refetches the task instead of replaying. A repeated `new` could create a duplicate, so the app does not resend `new` after an unknown outcome.
- **Push triggers.** `devices_push.rs` uses notice types from `corral-core` (`devices_push.rs:442-449`); they are re-derived from `blocked` and `ready` events. The effort was not measured.
- **Not verifiable on this host:** the GTK desktop target and a physical phone. Parity is checked on `tui` and `web` only.
