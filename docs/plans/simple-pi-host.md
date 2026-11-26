# Simple pi host: phase 3 plan

Reworks corral-pi onto the engine in [simple-engine.md](simple-engine.md), which stays authoritative. Work happens in `.worktrees/simple-pi`. Citations shorten three paths: `pi/` is `packages/corral-pi/src/`, `ops/` is `crates/corral-ops/src/agent/`, `guide` is `docs/guide/corral-pi.md`.

## 1. What this is, and what changes for the owner

**corral-pi is the agent program corral ships**: the open-source coding agent Pi plus corral's additions (extra abilities, the screen you see in a pane, the chat feed the app shows).

Other agents (omp, Claude) start as a plain command and finish by typing `corral done` in a shell. corral-pi does not. Today it talks to corral in a private language:

- **55 kinds of message** (`ops/call.rs:110`). The agent cannot run a corral command; it must send one of these.
- **Each message is checked against a password** made when the agent starts, **and stored as a receipt** so a repeat is answered from the record (`ops/admission.rs:1-3`, `ops/receipts.rs:1-6`).
- **Starting needs paperwork**: a read-once file with a secret, a fingerprint of the installed files, a "GO" verdict file and an exact runtime version (`ops/launch.rs:1-5`).
- **Every restart gets a number**, and the previous number is locked out (`ops/recovery.rs:1-5`).

This is the part that refuses to start, needs `corral agent resume` after a crash, and fills up: a session's record holds 16,384 receipts (`crates/corral-core/src/agent/records.rs:12`), and past that its messages are refused with `receipt_limit` (`ops/call.rs:346`).

After this phase:

- corral-pi starts like any other agent: one command with flags. It finishes by running the commands you run (`done`, `verdict`). Nothing is stored per message.
- The app and corral reach a running agent through one local connection: type, cancel, pause, watch.
- A crashed agent is started again and continues its conversation. Nothing is numbered or locked out.
- The PM and the assistant become one optional chat helper. A paid API key has one spending limit.
- The pane screen, the app chat and the agent's abilities stay, including rewinding a session to an earlier turn.
- Gone: question keys, the inbox, plans as a special flow, the PM's wake-ups, and the memory tools (the notes themselves stay, as plain files the agent reads and edits).

## 2. Terms

- **Host**: the TypeScript program that wraps Pi; `corral-pi` is its one executable.
- **Session**: one conversation: an id, a working directory (worktree), a transcript and a host process.
- **Transcript**: the file Pi appends the conversation to. **Turn**: one stretch of agent work between inputs.
- **Session socket**: a local Unix socket the host listens on. **Op**: one of today's 55 message kinds.

## 3. Where each of the 55 ops goes

The host runs engine commands by argv through the runner build sessions already use (`pi/build/corral-cli.ts:9`), with the binary path from `--corral`. Three typed tools remain: `corral_done`, `corral_verdict`, `corral_artifact`. Every role that needs more has bash (`pi/roles/worker/index.ts:20`, `pi/roles/reviewer/index.ts:17`, `pi/roles/assistant/index.ts:20`). A refusal's `reason` and `next` reach the model unchanged.

| Ops today (count) | Becomes | Note |
|---|---|---|
| `done`, `verdict`, `gate`, `artifact` (4) | engine `done`, `verdict`, `gate`, `artifact add` | A repeated `done` is harmless |
| `reply`, `comment`, `capture`, `idea_file` (4) | engine `reply`, `comment`, `new` (`--idea`) | |
| `wait` (1) | engine `wait --task <id> --for done,dropped` in the shell | Channel waits and wake messages go |
| `read_show`, `read_content`, `read_list`, `read_search`, `read_brief`, `read_next`, `read_artifacts` (7) | engine `show`, `list --json`, `artifact list`, `artifact show` | PM worksets (brief, next) go; the agent filters `list` |
| `pm_action`, `owner_action` (2) | the engine command the action named (`dispatch`, `approve`, `retry`, ...) | Action brokering and confirmations go |
| `event`, `todo`, `job`, `job_monitor`, `owner_ask`, `owner_prompt`, `checkpoint` (7) | host-local | Build sessions already answer most of these in the host (`pi/build/control-plane.ts:86`, `pi/host/build.ts:220-222`); turn reports become socket events; the worker's resume note (`pi/roles/worker/tools.ts:55-65`) is kept in the transcript beside the turn checkpoints (section 6) |
| `hello`, `receipt_lookup` (2) | dropped: generations, receipts | The paid-continuation notice `hello` returned (`pi/host/session.ts:2005-2006`) becomes a host flag (section 9) |
| `ask`, `withdraw`, `question_answer`, `question_escalate`, `question_cancel` (5) | dropped: questions with keys | The agent asks in its session (section 6) |
| `progress`, `evidence` (2) | dropped: run progress ledger, criteria evidence | |
| `gate_triage` (1) | dropped: gate tiers and repairs | |
| `inbox_read`, `inbox_resolve`, `peer_send`, `peer_roster` (4) | dropped: inbox, cross-session agent messages | Messages inside one session's fleet stay |
| `plan_write`, `plan_submit`, `plan_verdict`, `assistant_plan`, `assistant_plan_review`, `assistant_handoff` (6) | dropped: plans | A plan is a parent task and a Markdown file |
| `notes`, `memory_list`, `memory_read`, `memory_put`, `memory_remove` (5) | dropped as ops: PM notes, retained memory | The content stays as Markdown files under `.corral-next/memory/`, edited with ordinary file tools (section 7) |
| `pm_shadow`, `read_route`, `read_lessons`, `read_annotations`, `write_permission` (5) | dropped: PM shadow, route preview, lessons, annotations, write grants | |

## 4. Launch

Today dispatch writes a one-shot `bootstrap.json` holding a bind nonce and an asset digest (`ops/launch.rs:49-53`, `:72`, `:79`). `corral agent launch` reads and deletes it, checks the `compat.json` verdict, the digest and the Bun range (`ops/launch.rs:665-694`, `:786-826`), makes two secrets and writes `ctl.cap` (`ops/launch/secrets.rs:18-44`). The host reads them from an inherited pipe (`pi/host/main.ts:50-61`), re-checks Bun, the lock digest and the Pi package (`pi/host/runtime.ts:73-96`) and binds by calling `hello` (`pi/host/session.ts:1987-1999`).

| Today | Replacement |
|---|---|
| One-shot bootstrap file | Flags and environment. Built-in harness `pi`: `command = "corral-pi run {model_args} --prompt-file {prompt_file} --corral {corral}"`. Role comes from `CORRAL_ACTOR`, task and run from `CORRAL_TASK` and `CORRAL_RUN`. The runner adds `--dir` (the run directory) and `--socket` |
| `host_cap`, `ctl_cap`, bind nonce | Nothing. Host to corral is the CLI; corral to host is guarded by the socket's file mode |
| Asset digest, managed install | Nothing. `corral-pi` is one compiled executable (the build exists: `ops/bridge/assets.rs:15-17`); development runs `bun pi/cli.ts` |
| `compat.json` verdict and ability list (`packages/corral-pi/compat.json:8`, `:64-93`) | File deleted. Each role has a default ability list in the host, as build has (`pi/host/build.ts:28-31`); settings (web, MCP servers, LSP) come from an optional `.corral-next/pi.toml` |
| Bun, lock and Pi pins | Exact versions in `package.json` and `bun install --frozen-lockfile` at install (`ops/install.rs:5-9`) |
| `corral agent login`, `corral pi` | `corral-pi login`, `corral-pi chat`, `corral-pi build`; the Rust login only spawns the TypeScript module (`ops/credentials.rs:343-357`) |

The version check that stays: an uncompiled host refuses a Bun outside the range in `package.json` (one comparison, today `pi/host/runtime.ts:75-78`), and the runner refuses a host whose socket `hello` reports a protocol other than `2` (today `1`, `pi/host/main.ts:181`) with `reinstall corral-pi`.

## 5. The session socket

One JSON request and one reply per line, at most 64 KiB (`pi/bridge/protocol.ts:6`).

| Commands | Fate |
|---|---|
| `hello`, `get_state`, `submit_input`, `cancel`, `events_since`, `live_since`, `shutdown`, `queue_state`, `queue_withdraw`, `set_model` (`pi/bridge/protocol.ts:16-20`) | Stay. `hello` returns protocol, role, session id, transcript path and pid. `submit_input` loses its idempotency key, takes optional attachments and an `origin` of `owner` or `auto`; `get_state` reports `paused` |
| `pause`, `resume`, `rewind {turn}` | New (section 6). While paused, `auto` input is answered `queued` and held; `owner` input resumes |
| `answer {question, option or text}` | New: resolves a pending owner question; the app's `prompt.answer` uses it |
| `questions`, `handoff_drain`, `owner_turn` | Go, with question keys, assistant workflows and journaled input ids (`pi/host/remote-owner.ts:17-35`) |

Access control is file permissions: the socket is mode 0600 (`pi/bridge/server.ts:275`) in a 0700 directory the runner creates and verifies as today (`ops/launch.rs:617-623`). The `ctl_cap` and `generation` request fields and the peer-uid lookup go (`pi/bridge/protocol.ts:81-84`, `pi/bridge/server.ts:220-228`, `:242-247`). The socket lives at `$XDG_RUNTIME_DIR/corral-next/<16 hex of sha256(dir)>.sock`, else under `/tmp/corral-next-<uid>/` as today (`ops/launch.rs:535-540`), because socket paths are short.

| Cap today | After |
|---|---|
| 4,096 input keys per generation (`pi/bridge/protocol.ts:10`, `pi/bridge/server.ts:141-144`) | Gone with the keys. The 8 KiB text limit (`pi/bridge/protocol.ts:7`) rises to the frame; the runner writes longer text to a file in the run directory and sends its path |
| 1,024 queue operation receipts (`pi/host/queue.ts:12`) | Gone. `queue_withdraw` answers `withdrawn`, `already_started` or `unknown_item` from the queue itself (`pi/host/queue.ts:36`) |
| 64 generations (`crates/corral-core/src/agent/records.rs:13`) | Gone. Restarts are not counted |
| 16,384 receipts (`crates/corral-core/src/agent/records.rs:12`) | Gone. Startup cannot fail with `hello_refused` (`pi/host/session.ts:2000-2003`): the host no longer asks corral for permission to start |
| 512-event rings with an explicit gap (`pi/bridge/events.ts:38`, `:71-74`); 4 requests in flight (`pi/bridge/protocol.ts:8`) | Stay: they bound host memory |

## 6. The session model

- **Id**: `<task>/<run>` for a worker (the engine's run), `<task>/reviewer` for the task's one reviewer, `A-<n>` for chat, `B-<id>` for build.
- **Directory**: `runs/<task>/<run>/` for a worker, `runs/<task>/reviewer/` for a reviewer (one per task, not per round), `.corral-next/chat/A-<n>/`, or today's build directory (`docs/guide/build-sessions.md:29-32`). It holds `session.json` (id, role, worktree, transcript path, pid, `paused`, held input), the record build sessions already write (`pi/build/session-store.ts:49-57`), and `spend.json`.
- **Process**: one host. On start, if `--socket` answers `hello` the host exits with `session already live; corral stop <id>`; otherwise it removes the stale file and binds (`pi/bridge/server.ts:271`). This replaces fences.
- **Restart**: the host opens the transcript `session.json` names. A new worker run with none takes the newest sibling run's transcript, unless the engine set `CORRAL_FRESH=1` (`retry --fresh`). A reviewer always finds its task's transcript in its directory; only a new task starts a fresh reviewer conversation. The refusal for a restart without a recorded path (`pi/host/lifecycle.ts:67`) and the recovery notice (`ops/recovery.rs:1-8`) go; Pi already closes tool calls a dead host left open (`pi/host/lifecycle.ts:5-6`). The "read your prompt file" message, sent only to fresh sessions today (`pi/roles/worker/index.ts:75-77`), is sent whenever this run's prompt is not yet in the transcript.
- **Questions**: every role asks with the host's `ask_user`, kept in the transcript (`pi/host/pending-questions.ts:1-2`) and listed in `get_state` (`pi/host/session.ts:2280`). `sync` shows the task Blocked with the question; `corral reply` or `answer` resolves it.
- **Pause**: a `paused` flag in `session.json`, nothing else: no task hold, no pause ledger. Pause cancels the current turn and keeps the host alive. While the flag is set the host holds automated input (send-backs carrying gate or reviewer notes, and anything else the engine sends itself) in `session.json`, so a host restart keeps it; the sender is told `queued` and succeeds. Resume, or any message the owner types (pane, app chat, `corral reply`), clears the flag and delivers the held input in order, before the owner's message.
- **Checkpoints and rewind**: the snapshots do not change. At each turn's start and end the host commits the worktree through a private index to a per-worktree ref and appends a `corral_checkpoint` entry to the transcript (`pi/capabilities/checkpoints.ts:1-6`, `:64-66`, `:79-80`); only the id loses its generation (`:74`). Today rewinding is `corral agent rewind` in Rust: staged records, a fence, a next generation opened at the checkpoint's transcript entry, and a refusal when a path also changed outside the agent's turns (`ops/rewind.rs:1-11`, `pi/host/lifecycle.ts:57-61`). After, `/rewind <turn>` in the pane or socket `rewind {turn}` does it inside the running host: cancel the turn, save the current worktree under a rescue ref (`refs/worktree/corral/rescue/<time>`) so nothing is lost, restore the checkpoint's tree and the head it recorded (`pi/capabilities/checkpoints.ts:28`), cut the transcript back to that turn's entry, continue. No per-path conflict logic: whatever was overwritten is in the rescue ref, whose name the host prints. Workers only, as today (`pi/host/session.ts:885-886`).

Controls from `docs/app-design/SESSION-CONTROL.md:25-32`:

| Control | Maps to |
|---|---|
| Cancel | Socket `cancel` (`pi/host/session.ts:2170-2184`). The host stays alive and idle |
| Pause | Socket `pause`: cancel the turn, set `paused`. The host stays alive; automated input is held. Offered for workers, chat and build; reviewers keep Cancel and Terminate (`docs/app-design/SESSION-CONTROL.md:36-37`) |
| Resume | Socket `resume`, or any owner message (`docs/app-design/SESSION-CONTROL.md:16`): clear the flag, deliver held input in order |
| Terminate | Engine `stop <id>`: the runner sends `shutdown` (`pi/bridge/server.ts:194-196`), then closes the pane or kills the process. Task Blocked `stopped by owner`; worktree and branch stay. Chat: the daemon stops it; the transcript stays |
| Relaunch | Worker `retry <id>`, reviewer `review <id>`, chat: start again on the same directory. The transcript continues |
| Launch PM | "Start chat": the daemon starts `corral-pi chat`. It depends on no automation setting. An ended quick thread is not restarted; the next use starts a new session |

## 7. Roles

A role is a tool list and a prompt (`pi/roles/types.ts:132-161`). Four remain of today's seven (`pi/roles/types.ts:12`):

| Role | Tools | Finishes with |
|---|---|---|
| Worker | Pi's file and shell tools, abilities, `ask_user`, todo | `corral_done` |
| Reviewer | Read-only tools and bash; one session per task, across rounds | `corral_verdict`, once per round |
| Assistant (chat) | Everything; runs `corral-next` in bash as `CORRAL_ACTOR=assistant` | Nothing: it owns no state |
| Build | A worker without a task or `corral_done`; exits Keep, Iterate or Land (`docs/guide/build-sessions.md:147-151`) | The owner's exit choice |

**PM and assistant merge (decided by the owner, 2026-11-18).** The PM already has edit and shell tools like an assistant (`pi/roles/pm/index.ts:20-21`). Costs: its typed, receipted tools (`pi/roles/pm/tools.ts:47-141`) become shell commands; corral no longer wakes it for each question (`guide:1884-1891`), so it acts when the owner writes, or runs `corral-next wait` as a background job when asked to watch; write grants and action confirmations (`pi/roles/assistant/index.ts:15-16`) go. Keeping both would have cost a second role and prompt, a singleton with an approved scope (`guide:1865-1870`), wake delivery through an inbox the engine does not have, an `Actor` the contract does not list, and cards that wait on the PM.

**A reviewer keeps its context between rounds (decided 2026-11-18).** After a `changes` verdict the reviewer session stays alive, and the engine sends the next round's re-review request to it as automated input. If its host died, the engine starts a reviewer again and the host reopens the task's reviewer transcript. Today's reviewer also continues a retained conversation (`pi/roles/reviewer/index.ts:3-5`); its duplicate-request marker (`pi/roles/reviewer/index.ts:48-62`) stays, keyed by the round instead of the generation.

**Planner and plan reviewer** go with plans; their prompts are inline (`pi/roles/planner/index.ts:19-35`, `pi/roles/plan-reviewer/index.ts:19-36`). Their rules (write one plan and do not implement it; judge the plan, not code) move to a `corral-plan` skill shipped with worker and reviewer. A planning task is an ordinary task whose body asks for the document; children are made with `new --parent`.

**Memory.** Today corral projects retained memory into the prompt from a file it writes at launch (`ops/launch/message.rs:29-31`, `pi/host/session.ts:1180`), and roles change it through four ops (`pi/roles/types.ts:34`). After: plain Markdown files under `.corral-next/memory/`. Each role's prompt names the directory; a role reads and edits the files with its ordinary file tools. PM notes become one of these files. A build session outside a workspace has none.

## 8. The engine side

`crates/corral-engine/src/runner/pi.rs` adds `pi::Client` (the socket) and `PiRunner`, which wraps the herdr or process runner for placement. `SessionRef` (`crates/corral-types/src/lib.rs:228`) gains `Pi { socket, inner }`.

| Method | Behavior |
|---|---|
| `start` | Add `--dir` and `--socket`, call the inner `start`, wait up to 30 s for `hello`, check the protocol. For a reviewer `--dir` is `runs/<task>/reviewer/`, not the run directory, so a restarted reviewer reopens its transcript |
| `status` | `get_state`: open turn → `Working`; pending question or failed turn → `Blocked { excerpt }`; else `Idle`. Socket refused → the inner `status`, so `Gone` once the pane or process is gone |
| `send` | `submit_input` as a steer read at the next turn boundary (`guide:1682-1684`). `Origin::Owner` (`crates/corral-types/src/lib.rs:244`) is sent as `owner`; `Origin::Auto` (send-backs, re-review requests) as `auto`, which a paused session answers `queued` while the call succeeds |
| `stop`, `focus` | `shutdown`, wait 10 s, then the inner `stop`; `focus` is the inner `focus` |

**Daemon live stream.** `session.v1` serves the existing scoped `SubDelta::Session` page.
One reader per watched scope polls `live_since` and `events_since` through `pi::Client` at
10 Hz while the host is quiet and every 30 ms while it returns events. Either ring can force a transcript reconciliation. A cursor beyond the host's
`last` resets both cursors and changes the page's in-memory `generation`; nothing is stored.

The local and paired owner APIs use the same scopes: `T-001_R1` (the Agents row id),
`T-001/reviewer` (the current reviewer), or `A-1` (chat). Payloads are in
`corral_protocol::{session,assistant}`:

| RPC | Request → reply |
|---|---|
| `session.cancel` | `SessionCancel` → `SessionCancelReply` |
| `session.pause`, `session.resume` | `SessionTarget` → `SessionPauseReply`, `SessionResumeReply` |
| `session.terminate`, `session.relaunch` | `SessionTarget` → null, `SessionRelaunched` |
| `session.rewind` | `SessionRewind` (`list` or a `turn`) → `RewindReply` |
| `prompt.answer` | `SessionPromptAnswer` (scope, Pi question, option and/or text) → `SessionPromptAnswered` |
| `assistant.send`, `assistant.cancel` | `AssistantSend` → `SendAck`; `AssistantCancelRequest` → `CancelAck` |
| `assistant.stop`, `assistant.warm`, `assistant.session` | `AssistantStop` → `StopAck`; `AssistantWarm` → null; `AssistantSessionOp` → `AssistantRow` |

Chat runs in a detached process and needs a Pi harness or profile selected in `corral.toml`,
not automation. Each `.corral-next/chat/A-<n>/` holds `chat.json` (thread/profile/end state),
the host's `session.json`, the Pi transcript, `spend.json`, process log, and runtime files.
Terminate preserves the scope for relaunch; End leaves it readable but the next quick send
uses a new id. The `assistant` topic lists sessions; `thread` is a scoped recent transcript
projection. `views/transcript.rs` pages Pi files for `detail.get` kinds `session` and
`session_item`. Use `op.submit` with `Op::New` at `Idea` to file an owner message as a task.

## 9. Money

Today every provider request reserves the model's whole context in a Rust ledger before it is sent (`pi/capabilities/budget.ts:30-45`, `:90`, `ops/ops/budget.rs:1-2`); build asks `corral build budget` (`pi/build/paid.ts:12`).

After: a profile may pass `--max-usd <n>` in its `args`. The host enforces it where every provider stream already passes (`pi/capabilities/budget.ts:1`, `:119`): refuse when spent ≥ cap, then add each response's priced usage (`:46-53`) to `spend.json`, so a restart does not reset it. A refusal uses the existing stop path (`:72-78`) and the session reports `Blocked: spending cap reached`, cleared by `corral retry --profile <p>`. Overrun is at most the requests in flight. Subscription sessions have no cap. The Claude SDK lane is unbounded (`guide:2381-2383`), so `--max-usd` with it is refused at start. At `done` the host attaches a `usage` artifact.

**Paid continuation stays, in the host.** Today Rust decides it and hands the successor a notice through `hello` (`pi/host/continuation.ts:1`, `guide:1198-1201`), and the work continues as a new generation or linked run (`guide:1207-1214`). After: a profile may also pass `--paid-route <provider/model>` in its `args`. When the host classifies a failure as subscription quota (`pi/host/provider-failure.ts:66-69`), it continues the same transcript on that API key and writes one line saying so. That spending counts against the same `--max-usd` and `spend.json`; `--paid-route` without `--max-usd` is refused at start. No notice protocol, no Rust ledger, no successor run.

## 10. Untouched

Code mode, subagents and fleet, oracle, web, browser, LSP, background jobs (`pi/extensions/`), MCP (`pi/capabilities/mcp.ts`), todo, skills, hooks and bash rules, the model catalog, `pi/auth/`, `pi/claude/`, the upstream patches (`packages/corral-pi/package.json:27-32`) and the Pi terminal UI.

Three keep working after losing a corral call: jobs record locally (`pi/extensions/jobs/runtime.ts:214`), session export uses `artifact add` (`pi/host/export.ts:33`), subagents lose the PM shadow card (`pi/extensions/subagent/runtime.ts:349-370`). Turn checkpoints stay as they are and rewind moves from Rust into the host (section 6). Removed with their features: cross-session peers (`pi/extensions/peers/transport.ts:40`), annotations (`pi/host/annotations.ts:107`).

## 11. Steps

Steps 1 to 9 and 12 are non-UI. Step 10 and the app half of step 11 are UI work and go to a UI specialist (`ui-easy` or `ui-hard`), never to `task`, `task-easy` or `task-hard`; so would a turn picker for rewind in `pi/ui/`.

| # | Step | Files | Check | Agent |
|---|---|---|---|---|
| 1 | Engine runner | Add `crates/corral-engine/src/runner/pi.rs` and tests; `SessionRef::Pi` in `crates/corral-types/src/lib.rs`; built-in harness `pi` | `cargo test -p corral-engine pi`: start, status, send, stop against a fake socket; an `Origin::Auto` send to a paused fake succeeds as queued; a second reviewer start gets the same `--dir`; protocol 1 is refused | `task-hard` |
| 2 | One executable, flags | Add `pi/cli.ts`; rewrite `pi/host/config.ts`, `pi/host/main.ts`, `pi/host/build.ts`, `pi/build/launch.ts`; cut `pi/host/runtime.ts` to the Bun range; delete `packages/corral-pi/compat.json` | `bun pi/cli.ts run --dir <tmp> --socket <tmp>/s.sock --prompt-file p.md` with the fixture provider goes idle and writes `session.json` | `task-hard` |
| 3 | Socket v2 | `pi/bridge/protocol.ts`, `pi/bridge/server.ts`, `pi/host/remote-owner.ts`; delete `pi/host/peer.ts`, `pi/bridge/source-drain.ts` | Bridge tests: a frame without a secret is answered; a second host on a live socket exits; `answer` resolves a question; after `pause` an `auto` input is `queued` and an `owner` input delivers it first | `task-hard` |
| 4 | Engine commands replace ops | Replace `pi/tools/corral.ts`, `pi/tools/define.ts`; move `pi/build/control-plane.ts` to `pi/host/local.ts` for every role; rewrite `pi/host/remote-prompts.ts` to raise prompts as socket events; delete `pi/delivery/`, `pi/extensions/peers/`, `pi/host/annotations.ts`, `pi/host/owner-ask-mirror.ts` | A fixture worker's `corral_done` makes a fake `corral-next` receive `done T-1 --summary ...`; `rg '"agent", "call"' packages/corral-pi/src` is empty | `task-hard` |
| 5 | Sessions without generations | `pi/host/session.ts`, `pi/host/lifecycle.ts`, `pi/host/queue.ts`, `pi/host/pending-questions.ts`, every `generation` use; `paused` and held input in `session.json` | Kill the host mid-turn, start it on the same `--dir`: the same Pi session continues, still paused if it was, with its held input; `rg -w generation packages/corral-pi/src` is empty | `task-hard` |
| 6 | Rewind in the host | `pi/capabilities/checkpoints.ts`, new `pi/host/rewind.ts`, `rewind` in `pi/bridge/protocol.ts`; replaces `ops/rewind.rs` and `ops/rewind_git.rs` | A fixture worker changes a file in turns 1 and 2 and the test edits another file by hand; `rewind {turn: 2}` restores the tree from the start of turn 2, the rescue ref holds both later changes, and the next message follows turn 1 in the transcript | `task-hard` |
| 7 | Roles and prompts | Delete `pi/roles/pm/`, `pi/roles/planner/`, `pi/roles/plan-reviewer/`, `pi/roles/common/memory.ts`; rewrite the four roles and `packages/corral-pi/assets/*-contract.md` (each names `.corral-next/memory/`); add the `corral-plan` skill | `ROLES` lists four; a fixture assistant runs `corral-next list --json` and edits a memory file; a fixture reviewer's verdict reaches the fake binary, a re-review request then arrives in the same transcript, and after a kill the reviewer reopens it | `task-hard` |
| 8 | Spending cap and paid continuation | `pi/capabilities/budget.ts`, `pi/host/continuation.ts`, `pi/roles/common/continuation.ts`; delete `pi/build/paid.ts` | With a priced fixture and `--max-usd 0.01` the second request is refused; a restart does not reset `spend.json`; a fixture quota error with `--paid-route` continues on the API route and its cost appears in `spend.json` | `task-hard` |
| 9 | Build exits | `pi/build/exit.ts`, `pi/build/tools.ts`, `pi/build/record.ts`, `pi/build/board.ts` | On a scratch repository verify runs the project's `gate` lines and Land fast-forwards the base; no `corral build` argv remains | `task-hard` |
| 10 | Terminal UI pruning | `pi/ui/` only: remove the receipt inspector, inbox, decision and proof views; read task and artifact views from `show --json` and `artifact list --json`; render the three tools | UI tests pass; pane screenshots of worker, reviewer and chat show header, transcript and task view with no dead panel | `ui-hard` |
| 11 | Daemon live stream and chat | `crates/corral-daemon/src/topics/app/session.rs`, `crates/corral-daemon/src/assistant.rs`; app session controls | The app shows text arriving from a pi worker and a chat session; Cancel stops the turn; Pause holds a send-back and Resume delivers it; Rewind from the app restores a turn | `task-hard`; app controls `ui-easy` |
| 12 | End to end and docs | A journey test with the fixture provider; rewrite `docs/guide/corral-pi.md`, `docs/guide/build-sessions.md`, the controls in `docs/app-design/SESSION-CONTROL.md` | A pi worker and a pi reviewer land a one-line change on a scratch repository through `corral-next`, with one `changes` round reviewed in the same reviewer session, and the app chats with both | `task-hard` |

Order: 1 and 2 to 4 can run in parallel, since sections 4 and 5 fix the wire between them; 5 to 9 follow 4, with 6 after 5; 10 follows 7; 11 follows 1 and 3; 12 is last.

Step 9 detail (local Land decided by the owner, 2026-11-18): `corral build gate`, `land`, `record` and `budget` go (`pi/build/exit.ts:153`, `:162`, `pi/build/record.ts:177`, `pi/build/paid.ts:12`). Every build lands by the local rebase and fast-forward the host already has (`pi/build/exit.ts:128-134`); verify runs the project's `gate` lines with `sh -c` in the worktree; follow-on work is filed with `corral-next new` instead of `queue add` (`pi/build/board.ts:149`).

## 12. Risks and what could not be determined

- **Size of step 5.** `generation` is on 319 lines in 81 non-test files under `pi/`, and `pi/host/session.ts` is 2,310 lines (`rg` and `wc`, 2026-11-17). Land it one directory at a time.
- **Old transcripts.** Queue ids embed the generation (`guide:1678`). Sessions are not resumed across cutover.
- **One guard.** Without the peer-uid check, the socket's mode and its directory's are the only protection.
- **Cap overrun.** Subagents send requests in parallel (`docs/guide/build-sessions.md:104`); a plain cap overshoots by what is in flight. The ledger's "resume never replenishes" rule (`docs/guide/build-sessions.md:64-67`) is kept only by `spend.json`.
- **Rewind restores the whole tree.** Today a path changed outside the agent's turns refuses the rewind (`ops/rewind.rs:9-11`); after, such an edit is overwritten and survives only in the rescue ref. Rescue and checkpoint refs are never pruned by this plan.
- **Still needing the owner's yes.** Paid continuation loses its per-role opt-in list and successor runs (`guide:1200-1214`): naming `--paid-route` on a profile is the opt-in.
- **Contract gaps filled.** `SessionRef::Pi`, `CORRAL_FRESH`, `CORRAL_ACTOR=assistant`, `runs/<task>/reviewer/` as the reviewer's directory, chat sessions as directories under `.corral-next/chat/` (no table), `.corral-next/memory/` and `.corral-next/pi.toml`.

Not determined from the code:

- Whether a transcript cut mid-write by a kill reopens cleanly: `openSession` checks only the header line (`pi/host/lifecycle.ts:41-49`).
- Whether the 4,186 lines of upstream patches depend on anything removed; only their file lists were read.
- Whether Bedrock login's profile inspection in Rust (`ops/aws_profile.rs:1`) has a full TypeScript equivalent in `pi/auth/aws.ts`; `corral-pi login` needs one.
- Whether the host's own herdr status reports (`pi/host/herdr.ts:300`) are still wanted once `status` reads the socket; they are left in place.
- Where the app's transcript replay parser lives once `corral-ops` is deleted; this plan only guarantees the transcript path in `hello` and `session.json`.
- Whether one host process can move to another provider: a provider change needs a new generation today (`pi/host/session.ts:2159`) and the network guard is built per provider at startup (`pi/host/main.ts:70-71`, `:103`). If it cannot, the host restarts itself on the same transcript with the paid route.
