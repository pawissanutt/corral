# Agent guidance

## Scope

This app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain and is not a member of corral's workspace; corral's gate does not build or test it. Design sources are in `docs/app-design/` at the repository root. Read its `README.md` for authority and implementation-status caveats before using `REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

The app has three data states (`DataState`):

- **Fixture**: explicitly selected, launch-only sample preview through the same live view-model path; actions affect sample state only. Never restore it on restart or present it as daemon facts.
- **Daemon**: an enrolled native Unix session (`CORRAL_SOCKET`, peer-UID trust and private `0600` device-id file), or a paired browser session over an explicitly enabled numeric-interface WebSocket with pinned Noise IK. Typed reads/actions cover all current destinations. Retained/offline snapshots stay visibly stale; capabilities, authority and missing facts remain explicit.
- **Disconnected**: no enrolled/paired data. Never substitute fixture facts. Destinations offer Connect and Preview; Settings remains available, with local preferences in General › This device. Unpaired browsers open Connection.

Browser and native have the same per-device authority model: `reply`, `act`, `assistant`, `assistant_act`, `admin`, initially enabled on newly enrolled devices. Any admin device can pair/revoke and edit authority; self-raise is refused. Setting `admin=true` requires a fresh device-bound preview/proof. Authority-change notifications refresh the authenticated session and invalidate pending confirmations. Confirm and Admin command classes require a tap against the daemon's matching preview plus device proof, not Face ID/PIN. Use the canonical `Command::app_class` table in `corral-protocol/src/cmd/mod.rs`.

Web pairing requires an existing admin app or the owner CLI to compare/confirm the matching code. Credentials are encrypted in IndexedDB with a non-extractable wrapping key; NSH Storage contains only the non-secret `corral.paired` marker. Revocation stops reconnect; retryable Forget removes local identity/state, not the daemon record. No browser shell executor is enabled. Direct LAN/Tailscale binding is opt-in; remote browsers require operator TLS/WSS (`daemon.web_endpoint`) and an HTTPS origin. There is no embedded TLS or relay.

Ask is one composer for assistant and PM messages. The daemon hosts assistants; PM messages enter its inbox. Transaction completion must acknowledge local persistence before send acceptance, matching draft retirement or command submission. Per-device/store drafts, cancellable messages and same-ID result recovery survive reconnect; other offline actions refuse. Offline boot may restore complete Board/Inbox/Plans/Builds/Health snapshots only for saved credentials, marked stale. Revision-checked transactions preserve concurrent instances' updates. Images and bounded UTF-8 files upload from the client; links are opaque. Missing provider/quota/raw-terminal facts stay unknown.

NSH Storage keys: `corral.destination`, `corral.settings`, `corral.theme`, `corral.sidebar`, `corral.filters`, `corral.paired`. `corral.filters` holds each screen's project filter and Now grouping keyed by daemon store, so several daemons can keep separate scopes. `corral.data`, `corral.conn` and `corral.mode` are removed. Transactional outbox/cache uses IndexedDB `corral-local-state` v1 (`records`, `meta`), or native `corral-app/durable.json` with retained lock. Credentials use IndexedDB `corral-app` v1 (`credentials`) or native `devices.json`. See README › Local storage for record keys, migration and bounds. Cargo version `0.1.0` and NSH package version `1.0.0`/build `1` are diagnostic; negotiate capabilities, not version-string equality.

Projects come from `projects.v1` (`Feed.projects`), or from task rows on older daemons. A complete empty Projects page is `WorkspaceMode::AssistantOnly`: no project management, board destinations hidden rather than unavailable. Never infer assistant-only from loading, stale or missing-capability state.

## Module map

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust, with no `nsh_*` imports or I/O; holds data, wording, state, reducers and view builders for every form.
- `src/model/daemon.rs`: owned daemon feed and its pure updates, including typed domain data and command states.
- `src/model/session_feed.rs`: a watched session's open turn, its child-agent fleet (`session.fleet.v1`, replaced per page) and continued transcript items, with their pure updates; `src/model/agents/` holds the session action bar (`controls.rs`), the chat input (`live_reply.rs`), the keys bar under a foreign harness's pane (`keys.rs`), the children and agent-message lists (`fleet.rs`, drawn by `screens/agents/transcript/fleet.rs` with the builds wording and line) and the transcript replay (`replay*.rs`, `items.rs`).
- `src/client/`: shared frame/session core, native Unix adapter and device-id persistence, browser WebSocket/Noise adapter and encrypted IndexedDB credentials, wire-to-model mapping, and root-owned tasks updating the daemon feed.
- `src/client/{durable,durable_native,durable_web}.rs`: acknowledged revision-checked storage; `outbox.rs`: per-namespace drafts/messages/commands/tombstones; `offline.rs`: credential-checked stale bootstrap snapshots.
- `src/client/app/{local,devices,scopes,terminal}.rs`: local persistence lifecycle, authority/device operations, scoped subscriptions and terminal windows; `src/client/web/{identity,lifecycle,keystore,push,push_runtime}.rs`: browser identity, Forget, storage and opt-in Web Push/service-worker bridge.
- `src/model/hue.rs`, `text.rs`, `effect.rs`: semantic colours, rich text, environment and reducer effects.
- `src/model/glance.rs`: counts, loop bands, named stage rails and activity strips, each with its hint.
- `src/model/nav.rs`, `health.rs`, `commands.rs`: navigation, health and palette commands; `NavState.opened` and `opens` track records opened by name so narrow Plans and Review show the record; `src/model/preview.rs`, `preview/`: pure synthetic feed/sample simulation through the live models, never a client executor; `preview/closed.rs` supplies sample closed tasks.
- `src/model/disconnected.rs`: honest empty shell snapshot, without sample facts.
- `src/model/commands/keys.rs`: focused-list bindings and shared terminal legends; `src/model/settings/{qr,authority,connect,push}.rs`: pairing QR, authority editor, first-run instructions and push preferences.
- `src/model/board/`: shared board snapshot, tasks, changes and review hunk.
- `src/model/inbox/`, `work/`, `plans/`, `builds/`, `timeline/`, `agents/`, `usage/`, `review/`, `settings/`, `assistant/`: domain reducers and view logic; `work` covers Now and Changes; `builds` holds published build-session records (`builds.v1`), never tasks. `work/closed.rs` models the paged done/dropped list via `DetailRef::Closed`, gated on `closed.v1`; `work/task_page.rs` builds the read-only off-board task view from `TaskDetail` and the phone task page model. Sample preview uses the same live reducers/routes/screens, not separate per-screen fixture behavior.
- `src/model/work/now_asks.rs`: inbox owner moves and their panels; `now_digest.rs`: since-visit digest; `now_keys.rs`: list walk; `now_page.rs`: Now page, rows and side panel.
- `src/model/agents/roster.rs`: scoped session roster; `roster_panel.rs`: selected-session panel and its summary.
- `src/model/plans/outline.rs`: plan list and statistics; `brief.rs`, `brief_view.rs`, `awaiting_brief.rs`, `member_brief.rs`: executing and awaiting plan briefs and member facts; `path.rs`: Path to done.
- `src/model/review/outline.rs`: candidate list and header; `loop_page.rs`: review page; `brief.rs`, `brief_side.rs`: changes and asked-for facts; `readiness.rs`: readiness chain.
- `src/state.rs`, `src/state/`: `AppState`, signals, effect dispatch and persistence; `keys.rs` routes focused-list/global keys, `confirmation.rs` owns preview/proof lifecycle, `devices.rs` dispatches device effects and `sidebar.rs` owns sidebar preference.
- `src/state/selection.rs`: Escape handling for Now and Agents selections.
- `src/model/projects/`: project list, per-screen `ProjectFilter`s (`filters.rs`), `WorkspaceMode`, assistant landing and Project page (`page.rs`, `Route::Project`), which scopes Now's loop and digest to the project and opens rows as records; `src/state/projects.rs` persists filters and composer project context. List screens use the domain `*_filtered` builders; `src/ui/kit/project_filter.rs` draws the chip bar.
- `src/state/source.rs`: selects daemon/preview feed and assistant interaction state without copying retained caches; `src/state/preview.rs`: handles local sample effects and the real clock. Delayed draft/subscription cleanup stays bound to its original source.
- `src/ui/kit/`: shared pieces every screen is built from, including marks, rows, page scaffolds, notices, tables and overlays.
- `src/ui/kit/glance.rs`: count, loop, rail and activity renderers; `src/ui/kit/hint.rs`: hint wrapper.
- `src/ui/health_line.rs`: responsive health display; `src/model/health/line.rs`: condensed health and status facts.
- `src/ui/kit/page_phone.rs`: phone page scaffolds and helpers.
- `src/ui/`: tokens, atoms, icons, Ask and composer surfaces, palette and terminal-specific pieces in `term.rs`.
- `src/ui/ask.rs`, `src/ui/ask/`: docked composer, model list, entry illustrations and `unconnected.rs` for disconnected Ask.
- `src/ui/composer/{draft,pending,send,phone}.rs`: draft lifecycle, queued/cancellable sends, accepted-send retirement and compact phone composer.
- `src/shell/`: desktop, phone and terminal shells; `src/shell/phone/nav.rs` owns phone page stacks/navigation.
- `src/shell/terminal_bar.rs`: terminal top bar.
- `src/shell/lock.rs`: explicitly labelled phone lock-screen push illustration.
- `src/shell/projects.rs`: the desktop/terminal sidebar's PROJECTS section (rows `project-<id>`, folded rail mark `nav-projects`); shells list destinations from `nav::navigation`, phone tabs from `nav::phone_navigation`.
- `src/os_integration.rs`, `src/os_integration/tray.rs`: native desktop global Ask shortcut, close-to-hide and tray recovery/Quit; not installed for browser/terminal/headless targets.
- `src/screens/`: destination renderers and page routing; look in the destination's module and its subdirectory for its screen. `src/screens/now/closed.rs` renders the Closed section at the bottom of the Now list.
- `src/screens/now/`: list, rows, digest and selection side panel, phone pages and Closed section; `src/screens/agents/`: roster, roster rows, summary and terminal panel, session and transcript.
- `src/screens/plans/`: list, executing detail, document, graph, Path to done and decide bar; `src/screens/review/`: candidate list, brief, diff, evidence and decide bar.
- `src/screens/projects.rs`, `src/screens/projects/list.rs`: the Project page (`Route::Project`, all forms), with scoped Now sections, plans, sessions, builds, burn-up and project facts over the digest, and the phone More › Projects list; `src/screens/landing.rs`: the assistant-only landing shown in Now's place.
- `tests/shell.rs`: headless shell tests.
- `theme.json`, `nsh.json`: theme and app configuration.

## How a screen is built

Each domain's model supplies a `State`, reducers and pure view builders. Read the tracked state and environment in a view, render the resulting content with `src/ui/kit/`, and dispatch actions with `AppState::act(signal, reducer)`, which updates the signal and runs returned `Effect`s. Keep wording, numbers and branching in the model. Screens support desktop/web, phone and terminal forms; phone pushed pages use `PhonePage`.
A count is a `glance::Stat` (icon and number alone, sentence as hint); a row's explanation is its hint, with one explanatory line kept on phone.

Give pressable controls and testable regions IDs in these shapes: `task-<id>`, `plan-<id>`, `build-<id>`, `project-<id>`, `inbox-<key>`, `agent-<id>` (including Project page session rows), `agent-<session>-child-<id>`, `agent-<session>-msg-<seq>`, `thread-<id>`, `stat-<key>`, `loop-<key>`, `seg-<group>-<value>` (project filters: `seg-project-<all|unassigned|id>`), `btn-<action>`, `opt-<n>`, and `page-title`. Preserve the shell IDs `tab-<slug>`, `nav-<slug>`, `settings-<slug>`, `ask-open`, `ask-close`, `health`, `health-source` and `count-<slug>`.

The explicit preview offers Demo controls; normal palette entries use the served data. List keys bind only with list focus or no focused control, never over overlays/confirmations; Tab and Space are not list bindings. Keep the sidebar available/collapsible, auto-folding when narrow. Render real dates, not mockup dates. Review defaults to Unified below about 1100 pt of content width.

## Conventions

- Rust 2024; at most 800 SLOC per file.
- No `unwrap()`, `expect()` or `panic!` outside tests; no `todo!()`.
- No dead or commented-out code. Comments say why, not what.
- Route UI, UX and visual work to the `ui` agent.

## Shared host and toolchain

Run only one cargo or NSH build at a time. Never generate load. Never build the `desktop` feature or the Linux or GPUI targets on this host; GTK 4 development packages are unavailable. Do not run `cargo run`, `nsh run` without `--target`, or `nsh run --target linux` or `gpui` here. The available targets are headless, `tui` and `web`.

`rust-toolchain.toml` pins rustc 1.97.1 and `wasm32-unknown-unknown`; NSH 0.5.1 needs rustc 1.97. Keep the pin and `.cargo/config.toml`, which disables the crashing global sccache wrapper and points the wasm compiler tools at the host's clang 18 installation.

If another process holds `~/.cargo/.package-cache`, builds can use `CARGO_HOME` pointing to a directory with `registry`, `git`, `bin` and `config.toml` symlinked from `~/.cargo`, together with `CARGO_NET_OFFLINE=true`.

## NSH API

Use the [NSH documentation index](https://nsh.dev/llms.txt); documentation pages are available as `.md`. The NSH 0.5.1 crate sources are under `~/.cargo/registry/src/*/nsh-*-0.5.1/`. Read the relevant documentation and source or a real call before using an API.

## Tests

Keep tests few and name each for the defect it catches. Test at the lowest layer that can fail. Do not pin wording, styles or incidental layout.

