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
- **F8 / Activity:** live, read-only background jobs and monitor observations.
- **F9 / Conversation:** returns to the same transcript/scroll-view instance, independently of review/composer focus.
- Click the main view labels. Enter while reviewing returns keyboard input to the composer; clicking it also releases review focus. Ordinary typing returns to the actual editor. Esc returns to Conversation when the review body has focus; it retains stock behavior while composing or inside a prompt.
- **F6:** sidebar/inline inspector focus; arrows navigate. **Ctrl+B:** wide rail/inline layout or explicit compact-detail expansion.
- 105+ terminal columns reserve a 34-column rail; narrower layouts use one navigation row at rest. Draft and binding survive relocation.

## Input delivery and model controls

The composer is Pi's real editor. **Enter** sends while idle and steers while busy. The follow-up shortcut shown on its top border queues after the current work (normally Alt+Enter; Ctrl+Q on Windows/WSL). Editing, paste, history and configured shortcuts stay native. View changes, inspection and resize keep the same draft; they do not submit it.

**Last typed input** reports upstream preflight: pending means admission is unknown, queued is not execution, started is admission rather than completion, handled means a command/extension consumed it, and rejected retains the refusal reason. The pending-input row shows known steering/follow-up counts only. Inbox observation/delivery journals are not proof of execution or completion. Structured notices are never restored into the owner editor by dequeue or Esc; queued input has no edit, cancel or reorder controls. Esc retains native current-operation abort behavior without replacing the draft.

Click the **model ▾** label to open Pi's existing model picker (keyboard: Ctrl+L). It offers the host's available catalog and commits through the existing provider/credential preflight. Busy or missing-credential refusals keep the current model/session and show the recovery action. Click **effort ↻** to cycle supported levels (keyboard: Shift+Tab); models without thinking have no effort action. `/thinking` remains the native level picker. These controls do not replace the native session. The bound worker/assistant/PM/reviewer role is a label, never a Build/Plan/Review switch.

## Stable reading, focus and compact paging

- **Follow pauses while you read.** Scrolling the conversation back (PgUp, wheel, Ctrl+Home) stops it following streamed output. New output then arrives below without moving your reading position, and Pi shows **↓ Jump to latest message · Ctrl+End**. Only Ctrl+End or a click on that control returns to live output.
- **Jumping never steals focus.** Pi's viewport keys (PgUp/PgDn, Ctrl+Home/Ctrl+End, prompt jumps) scroll the conversation even while the inspector has focus, and the inspector keeps it. Clicking the jump control does not move focus either. Ordinary typing still returns to the composer.
- **Relocation keeps your place.** While follow is paused, the 105-column relocation, Ctrl+B and terminal resizes keep the same transcript passage at the top of the view, even though Pi re-wraps it at the new width. The draft, the inspector tab, its current item and the inspector focus are kept too.
- **Short terminals page metadata.** When the inspector or rail has more items than rows, it shows a pager such as `‹ 2/7 ›`. ↑/↓ and clicks on `‹`/`›` move the same item, and the wheel scrolls the same list. A foreground dialog owns input: pager clicks, F6 and Ctrl+B do nothing underneath it.
- **The minimum width stays legible.** Below 70 columns the view bar reads `Chat F9  Changes F7  Activity F8`, and its click targets follow the labels. A launch directory outside Git reads `Not a Git repository` rather than a raw Git command error.

## Practical change review

Bound task/run sessions retain their canonical committed-candidate comparison. Other repository-bound sessions use working changes. Workspace assistant/PM sessions without a project directory show **Choose project**, never workspace-wide Git changes or a misleading clean-workspace result.

Use **o** in Changes or a focused inspector, click the Files project row, or run **/panels project** to choose a configured project or one of this assistant's scratch worktrees. The chosen scope is visible in the main view bar and rail title. A selected directory that is not a repository root is refused rather than widened to its parent workspace. Project selection clears the previous file/filter/diff metadata; an explicit Files scope offers only working/staged/unstaged comparisons, not a different task's canonical candidate. Browsing never rebinds the session, changes its tools, model cwd, inbox scope or canonical plan.

Files emphasizes the filename first, then status, known line deltas and directory on separate rows. Agents separates the bound session, activity and helpers; a workspace session explicitly says it has no bound project. Plan identifies its session, reports done/open/dropped counts, separates phases and wraps long steps, criteria and waits. Waiting steps use warning color; completed steps recede. The plan remains the session ledger, not a project-wide progress claim.

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

## Click-to-expand images

Click an inline image to fit it into the main work area. Click again, press Esc or F9 to close it. The same transcript/reading position returns and the real draft is preserved. The expanded image is bounded by the actual remaining viewport, including a tall composer. Uses existing bytes only: no new download, external viewer, file write or model call. Native image rendering requires a supported graphics terminal (Kitty in the current fullscreen SDK); this does not add graphics support to text-only terminals.

## Background activity and monitoring

Activity refreshes local host snapshots once per second while visible—no model/control-plane polling loop. It shows locally observed job state, real elapsed time, command, exit code/signal, process-group cleanup state, output byte count and bounded recent output. Click or j/k selects a job.

Existing line-monitor observations show their pattern, recorded deadline, local arm/match and report-queued states. Monitor attempts are **not** active-wait counts. Deadline elapsed is labeled outcome unknown; queued reports and local exit are **not canonical settlement, task completion or gate PASS**. Exit-only waits do not currently expose retained canonical outcomes here. Auxiliary research/oracle figures are occupied/queued semaphore slots, not invented worker execution history. Missing runtimes/telemetry are unavailable, not zero.

Workspace board task/run monitoring is separate. This view does not start, cancel or kill jobs, create monitors, dispatch workers or change authority.

## Native integration seam

`patches/@earendil-works%2Fpi-coding-agent@1.3.0.patch` adds an optional typed `InteractiveModeOptions.mainView(transcript)` hook to the pinned SDK's fullscreen layout factory. The original transcript instance is supplied to the host's public-component composition; the original fixed input dock remains outside it. An optional `readOnlyQueue` mode renders pending counts and prevents raw queued input being restored into the owner editor; the rich host enables it. Defaults are unchanged when these options are absent. The patch is tracked through Bun patchedDependencies/frozen lock and compat lock digest. The additional pi-tui@1.3.0 patch supplies removable image activation registration; unregistered images retain stock behavior. No runtime private-field casts, root discovery/reparenting, replacement editor loop or second agent runtime.

The Terminal adapter supplies default cell colors only in the alternate screen, without OSC palette/default-color setters. Explicit widget/selection colors and opaque clipboard/image protocols are retained. Foreground prompts and active selection suppress background interactions.

## Verification

```sh
cd packages/corral-pi
bun run typecheck
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
