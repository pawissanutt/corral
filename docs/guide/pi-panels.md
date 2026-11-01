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

## Practical change review

The default is all uncommitted changes in the Git worktree containing the launch directory. The actual Git root is resolved once per scope, so repository-relative paths also work when launched in a subdirectory. A bound worker's launch worktree remains its initial review scope; browsing another repository does not rebind the host or change the model's cwd.

While review has keyboard focus:

| Key | Action |
|---|---|
| `/` | Filter changed paths by space-separated search terms |
| `o` | Choose a read-only repository scope |
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

`patches/@earendil-works%2Fpi-coding-agent@1.3.0.patch` adds an optional typed `InteractiveModeOptions.mainView(transcript)` hook to the pinned SDK's fullscreen layout factory. The original transcript instance is supplied to the host's public-component composition; the original fixed input dock remains outside it. Defaults are unchanged when the hook is absent. The patch is tracked through Bun patchedDependencies/frozen lock and compat lock digest. The additional pi-tui@1.3.0 patch supplies removable image activation registration; unregistered images retain stock behavior. No runtime private-field casts, root discovery/reparenting, replacement editor loop or second agent runtime.

The Terminal adapter supplies default cell colors only in the alternate screen, without OSC palette/default-color setters. Explicit widget/selection colors and opaque clipboard/image protocols are retained. Foreground prompts and active selection suppress background interactions.

## Verification

```sh
cd packages/corral-pi
bun run typecheck
bun test src/ui/image-viewer.test.ts src/ui/workbench.test.ts src/ui/panels.test.ts src/ui/panel-canvas.test.ts test/native-panels.test.ts test/jobs.test.ts test/jobs-host.test.ts test/package.test.ts
cd ../..
python3 scripts/qa-pi-workbench.py /tmp/corral-native-workbench-evidence
python3 scripts/qa-pi-image-viewer.py /tmp/corral-native-image-evidence
```

The owned production-main PTY fixture uses an offline provider/control plane, real isolated Git repository and real FIFO-held background process. It verifies main view switching, hunk-to-draft/no autosend, external edits, comparison/search, 140→80→52→140 resize, monitor arm/match, process exit 7, native confirmation and clean terminal restoration. No live owner/worker/PM panes or paid model calls are used. Screenshot replay is evidence only, not the product runtime.
