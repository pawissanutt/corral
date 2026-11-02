# Native Corral Pi workbench

The rich `corral pi` host has Conversation, Changes and Activity main views. It retains the actual Pi transcript, editor, input journal, dialogs, runtime and role/generation guards. This is not the earlier browser mock. Bare, plain and headless modes remain separate.

## Install a checkout build

```sh
scripts/build-pi-host.sh /tmp/corral-pi-host
corral agent install --from /tmp/corral-pi-host
corral pi
```

Use a new owner pane/session; existing processes do not hot-reload. Explicit `[agent] assets_dir` overrides still win; `corral agent doctor` reports effective assets. Do not use `--bare` or `--pm` just to preview the UI.

## Main views and real composer

- **F7 / Changes:** persistent diff main area, not modal hopping. The real composer stays available below.
- **F8 / Context activity:** focus the separate Agents and Jobs lists within Context. The sidebar tabs are Context, Files and Plan; there is no separate Agents tab. ↑/↓ or j/k selects; Enter opens that agent's native conversation or that job's output directly. Below 105 columns the same list opens as a compact drawer above the composer.
- **F9 / Conversation:** returns to the same transcript/scroll-view instance, independently of review/composer focus.
- Click the main view labels. Enter while reviewing returns keyboard input to the composer; clicking it also releases review focus. Ordinary typing returns to the actual editor. Esc returns to Conversation when the review body has focus; it retains stock behavior while composing or inside a prompt.
- **F6:** sidebar/inline inspector focus; arrows navigate. **Ctrl+B:** wide rail/inline layout or explicit compact-detail expansion.
- 105+ terminal columns reserve a 34-column rail; narrower layouts use one navigation row at rest. Draft and binding survive relocation.

## Input delivery and model controls

The composer is Pi's real editor. **Enter** sends while idle and steers while busy. The follow-up shortcut shown on its top border queues after the current work (normally Alt+Enter; Ctrl+Q on Windows/WSL). Editing, paste, history and configured shortcuts stay native. View changes, inspection and resize keep the same draft; they do not submit it.

**Last typed input** reports upstream preflight: pending means admission is unknown, queued is not execution, started is admission rather than completion, handled means a command/extension consumed it, and rejected retains the refusal reason. The pending-input row shows known steering/follow-up counts only. Inbox observation/delivery journals are not proof of execution or completion. Structured notices are never restored into the owner editor by dequeue or Esc; queued input has no edit, cancel or reorder controls. Esc retains native current-operation abort behavior without replacing the draft.

Click the **model ▾** label to open Pi's existing model picker (keyboard: Ctrl+L). It offers the host's available catalog and commits through the existing provider/credential preflight. Busy or missing-credential refusals keep the current model/session and show the recovery action. Click **effort ↻** to cycle supported levels (keyboard: Shift+Tab); models without thinking have no effort action. `/thinking` remains the native level picker. These controls do not replace the native session. The bound worker/assistant/PM/reviewer role is a label, never a Build/Plan/Review switch.

## Packaged appearance and action discovery

Press **F2** while composing, or run **/actions**, to search the host action palette. Type to filter, use ↑/↓ then Enter to choose, or Esc to cancel. Entries show their shortcut, bound role/project scope and any disabled reason. Model and effort entries use the same native handlers described above; answer-flow inspection retains the existing role/scope checks and never sends an answer.

Search **Appearance** for packaged **dark**, **light** or **contrast** presentation; search **Density** for **comfortable** or **compact**. Compact removes blank inspector rows and reduces editor inset without changing data, permissions or input delivery. Choices are session-local, not saved to ambient Pi settings. Active tabs are underlined and bold, states retain text labels, and diffs retain +/− markers. Plain/headless modes are unchanged. The palette contains explicit host actions, not a CLI runner or ambient theme/extension loader. Native Ctrl+P model cycling is unchanged.

## Stable reading, focus and compact paging

- **Follow pauses while you read.** Scrolling the conversation back (PgUp, wheel, Ctrl+Home) stops it following streamed output. New output then arrives below without moving your reading position, and Pi shows **↓ Jump to latest message · Ctrl+End**. Only Ctrl+End or a click on that control returns to live output.
- **Jumping never steals focus.** Pi's viewport keys (PgUp/PgDn, Ctrl+Home/Ctrl+End, prompt jumps) scroll the conversation even while the inspector has focus, and the inspector keeps it. Clicking the jump control does not move focus either. Ordinary typing still returns to the composer.
- **Relocation keeps your place.** While follow is paused, the 105-column relocation, Ctrl+B and terminal resizes keep the same transcript passage at the top of the view, even though Pi re-wraps it at the new width. The draft, the inspector tab, its current item and the inspector focus are kept too.
- **Short terminals page metadata.** When the inspector or rail has more items than rows, it shows a pager such as `‹ 2/7 ›`. ↑/↓ and clicks on `‹`/`›` move the same item, and the wheel scrolls the same list. A foreground dialog owns input: pager clicks, F6 and Ctrl+B do nothing underneath it.
- **The minimum width stays legible.** The main view bar contains only Chat, Diff, Proof and Inbox (Conversation, Changes and Evidence at wide widths). Agent/job selection lives in the sidebar or F8 drawer, not a duplicate main tab; the selected entry's title appears below the main view bar. A launch directory outside Git reads `Not a Git repository` rather than a raw Git command error.

## Practical change review

Bound task/run sessions retain their canonical committed-candidate comparison. Other repository-bound sessions use working changes. Workspace assistant/PM sessions without a project directory show **Choose project**, never workspace-wide Git changes or a misleading clean-workspace result.

Use **o** in Changes or a focused inspector, click the Files project row, or run **/panels project** to choose a configured project or one of this assistant's scratch worktrees. The chosen scope is visible in the main view bar and rail title. A selected directory that is not a repository root is refused rather than widened to its parent workspace. Project selection clears the previous file/filter/diff metadata; an explicit Files scope offers only working/staged/unstaged comparisons, not a different task's canonical candidate. Browsing never rebinds the session, changes its tools, model cwd, inbox scope or canonical plan.

Files emphasizes the filename first, then status, known line deltas and directory on separate rows. The Context overview shows separate actionable Agents and Jobs groups, not helper-slot counters or a Plan summary. These Context lists show this parent's actual retained calls and background commands; they do not represent board-wide sessions. Successful completions are collapsed under Completed; interrupted calls and failed jobs stay visible. Plan remains available in its dedicated tab: it identifies its session, reports done/open/dropped counts, separates phases and wraps long steps, criteria and waits. Waiting steps use warning color; completed steps recede. The plan remains the session ledger, not a project-wide progress claim.

While review has keyboard focus:

| Key | Action |
|---|---|
| `/` | Filter changed paths by space-separated search terms |
| `o` | Choose a project or scratch worktree (read-only Files scope) |
| `c` | All uncommitted → unstaged → staged comparisons |
| `j` / `k` | Next / previous file, without closing the view |
| arrows / PgUp / PgDn | Scroll the main view |
| `n` / `p` | Next / previous hunk |
| `a` | Append selected hunk context to the existing draft; never submit |
| `r` | Refresh and apply the latest selected diff |

Filename clicks in the overview or Files inspector select that path in Changes. The list includes clear statuses, known line deltas/binary state and a visible 500-path bound. Untracked files have actual new-file patches; renames retain old/new paths. All/unstaged/staged meanings are explicitly labeled. No staging, discarding, committing or deletion is exposed.

Refreshes are coalesced after tool results and every two seconds only while Changes is open. Failed reads retain explicit stale/unavailable state. Refresh does not reorder selection. A changed selected patch shows an update indicator rather than silently replacing the inspected hunk; `r` applies it. Captured root/path/comparison/HEAD and patch text form one immutable provenance snapshot. Draft references include that captured scope, observation time and diff SHA256.

## Read-only history

Click **History** in the main view bar, or use **/panels history**. Choose Tasks / runs or Assistant sessions, search recorded records, then select a subject. This is inspection, not Pi's `/resume` picker: the bound session, generation, role, tools and unsent draft remain unchanged.

Task/run recaps label the recorded outcome, effective pending questions, and checkpoint next action (or first in-progress/pending ledger step) with provenance. Historical steps are context, not instructions or proof of checkpoint compatibility. Assistant recaps use the saved summary, questions and ledger; an ended assistant has no effective pending questions. Missing outcome, ledger, checkpoint or generation facts remain explicitly unknown.

In a task/run recap, **t** opens bounded raw JSONL transcript pages. **n/p** read next/previous pages; byte-limit continuation uses the backend's exact resume record, not a guessed offset. Each backend page is at most 200 records and 24 KiB. **c** reads available native checkpoint/rewind history. Legacy or missing locators report unavailable. Assistant transcripts are explicitly unavailable in this navigator because the existing pageable endpoint supports task runs only. There are no restore controls or impact previews.

Reads stay at the launcher's workspace root, independently of Files scope. Task details use `corral show T-001 --json --recorded`, which does not refresh session, usage or gate state. Each read has a deadline and 4 MiB response bound; oversized/unavailable reads fail visibly. Selectors show at most 200 returned entities, with counts; refine the search rather than treating the slice as exhaustive. Detail displays are capped at 64 KiB with an explicit truncation marker. Esc closes inspection without submitting the draft. Native new/resume/fork/import replacement guards are unchanged.

## Click-to-expand images

Click an inline image to fit it into the main work area. Click again, press Esc or F9 to close it. The same transcript/reading position returns and the real draft is preserved. The expanded image is bounded by the actual remaining viewport, including a tall composer. Uses existing bytes only: no new download, external viewer, file write or model call. Native image rendering requires a supported graphics terminal (Kitty in the current fullscreen SDK); this does not add graphics support to text-only terminals.

## Background activity and monitoring

A selected job refreshes local host snapshots once per second while visible—no model/control-plane polling loop. It shows locally observed job state, real elapsed time, command, exit code/signal, process-group cleanup state, output byte count and bounded recent output. Click its sidebar row, or use F8 then ↑/↓ and Enter, to open it. Selection uses stable job IDs even when newer jobs arrive.

Existing line-monitor observations show their pattern, recorded deadline, local arm/match and report-queued states. Monitor attempts are **not** active-wait counts. Deadline elapsed is labeled outcome unknown; queued reports and local exit are **not canonical settlement, task completion or gate PASS**. Exit-only waits do not currently expose retained canonical outcomes here. Agent rows show actual retained calls, not occupied/queued semaphore slots or invented worker execution history. Missing runtimes/telemetry are unavailable, not zero.

Workspace board task/run monitoring is separate. This view does not start, cancel or kill jobs, create monitors, dispatch workers or change authority.

## Helper conversations

Click an agent in Context or press **F8** to focus Context's activity lists and choose one with **↑/↓ (j/k), Enter**. **/panels agents**, **/panels helpers** and **/panels jobs** open this same sidebar selector. No intermediate Activity screen or Jobs/Helpers mode switch is needed. On narrow terminals the list opens above the composer and closes when an item opens. The selected agent replaces the main reading area; arrows and PgUp/PgDn scroll its native conversation. F9 returns to the same parent conversation. Returning to an agent restores its reading position; new activity never switches your view automatically. The composer remains the parent's editor and is explicitly labeled Message main conversation while inspecting agents or jobs.

The inspector shows the supplied prompt, live visible response text, requested/running tools, finalized text tool results, returned result, model, generation and locally observed outcome. Queued/not-started calls are distinct from running calls; interruptions and failed terminal accounting remain uncertain. Inspecting never calls a model, resumes/cancels a child, changes its loadout, or alters the parent identity. Private reasoning, signatures, image bytes and tool-specific details are excluded.

The conversation body reuses Pi's `UserMessageComponent`, `AssistantMessageComponent` and `ToolExecutionComponent`, not flattened text rows. Its presentation comes from the same `InteractiveMode` instance as the parent: the active Markdown theme, code indentation, output padding, Markdown/Mermaid transformers and native tool renderer resolution. Click a tool card or use **Ctrl+O** to expand/collapse it. Only read-only tool renderers are resolved; executable tool definitions are not passed to this view. A returned result already represented by the assistant transcript is not repeated.

History belongs only to this running parent host: at most 32 recent helpers, 64 message blocks and 32,768 logical transcript characters per helper, with individual prompt/result/message fields capped at 4096 characters. Assistant text retains up to 16 text-block boundaries within its character cap. Native tool cards retain Pi's collapse/expansion behavior; history omissions are explicitly marked, and observed errors and unrepresented returned results remain accessible. Selection stays on the same request when newer helpers arrive; evicted selections are explicitly unavailable. History is not restored from disk after restarting the host and never reads another parent's session files. Unavailable telemetry is not reported as an empty successful history. Subscription search can show its prompt/result, but does not expose live internal tool activity through this adapter.

### Explore Fast tier

After upgrading Corral and the Pi host, Codex Explore can use:

```toml
[agent.capabilities.assistant.subagents]
model = "gpt-6-luna"
service_tier = "priority"
```

The optional tier accepts `default` or `priority`; leaving it unset preserves existing behavior. Priority is supported only for `openai-codex` Explore, not the parent, Oracle or subscription search. Fast is a request tier, not a `-fast` model alias. The SDK's own priority rate multiplier applies to conservative USD reservations and settlement; token counts are unchanged, and interrupted requests retain their tier-priced reservation. Settings bind on the next host generation.

## Native integration seam

`patches/@earendil-works%2Fpi-coding-agent@1.3.0.patch` adds an optional typed `InteractiveModeOptions.mainView(transcript, presentation)` hook to the pinned SDK's fullscreen layout factory. The original transcript instance is supplied to the host's public-component composition; the original fixed input dock remains outside it. The live presentation getter delegates to Pi's existing theme/transformer/tool-renderer methods and supplies its existing TUI, so chat presentation changes carry over without a parallel styling implementation. The assistant renderer accepts presentation-only content/stop/error fields; the inspector does not fabricate model or usage metadata. An optional `readOnlyQueue` mode renders pending counts and prevents raw queued input being restored into the owner editor; the rich host enables it. Defaults are unchanged when these options are absent. The patch is tracked through Bun patchedDependencies/frozen lock and compat lock digest. The additional pi-tui@1.3.0 patch supplies removable image activation registration; unregistered images retain stock behavior. No runtime private-field casts, root discovery/reparenting, replacement editor loop or second agent runtime.

The Terminal adapter supplies default cell colors only in the alternate screen, without OSC palette/default-color setters. Explicit widget/selection colors and opaque clipboard/image protocols are retained. Foreground prompts and active selection suppress background interactions.

## Verification

```sh
cd packages/corral-pi
bun run typecheck
bun test test/helper-history.test.ts test/helper-chat-host.test.ts test/auxiliary.test.ts test/auxiliary-host.test.ts test/budget.test.ts src/ui/helper-view.test.ts src/ui/helper-chat.test.ts src/ui/activity-sidebar.test.ts
bun test test/panel-composer.test.ts test/host-session.test.ts test/bridge.test.ts test/subscriptions-host.test.ts src/ui/image-viewer.test.ts src/ui/workbench.test.ts src/ui/panels.test.ts src/ui/panel-shell.test.ts src/ui/panel-canvas.test.ts test/native-panels.test.ts test/panel-host-proof.test.ts test/panel-shell.test.ts test/jobs.test.ts test/jobs-host.test.ts test/package.test.ts
cd ../..
python3 scripts/qa-pi-workbench.py /tmp/corral-native-workbench-evidence
python3 scripts/qa-pi-image-viewer.py /tmp/corral-native-image-evidence
python3 scripts/qa-pi-panel-proof.py /tmp/corral-panel-proof-evidence
python3 scripts/qa-pi-panel-shell.py /tmp/corral-panel-shell-evidence
```

`test/panel-shell.test.ts` and `qa-pi-panel-shell.py` (T-688) drive the real host with a long streamed reply that contains wide graphemes. They read back during the held stream, then check four things. First, the same paragraph stays at the top through stream completion, 140→80→52→140 resizes and Ctrl+B. Second, Ctrl+End and a click on the jump control return to live output without taking inspector focus. Third, at 80×14 the pager moves identically by ↓ and by `‹`/`›` clicks. Fourth, a dialog isolates the inspector. The PTY script uses real key sequences and SGR mouse reports against `src/host/main.ts`, and writes ANSI and screen-text captures for each step. The plain host still creates no panels.

`panel-composer.test.ts` drives real keyboard editing, held preflight, started/queued/handled/refused input, native pointer/keyboard model and effort controls, busy/credential model refusals, notice-count compaction and draft retention. The bridge/host-session suites retain pending/idempotent delivery coverage. `qa-pi-panel-proof.py` also opens the native picker and cycles effort with real SGR pointer reports while retaining a draft.

`panel-host-proof.test.ts` and `qa-pi-panel-proof.py` are the layout-seam proof from T-687 (`docs/records/T-687-pi-panel-host-proof-report.md`). A reply is held after its first streamed delta while a real `read` result, inspection and 140→80→52→140 resizes are exercised. The bound session and generation are read back over the generation's own bridge, one composer submission is counted end to end, and a tool outside the loadout and a session replacement stay refused. Re-run both after any Pi SDK pin bump.

The owned production-main PTY fixture uses an offline provider/control plane, real isolated Git repository and real FIFO-held background process. It verifies main view switching, hunk-to-draft/no autosend, external edits, comparison/search, 140→80→52→140 resize, monitor arm/match, process exit 7, native confirmation and clean terminal restoration. No live owner/worker/PM panes or paid model calls are used. Screenshot replay is evidence only, not the product runtime.
