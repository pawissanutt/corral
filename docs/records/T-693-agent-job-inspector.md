# T-693 — scoped agent, transcript and job inspection

## Delivered scope

The production Pi main view now has a read-only scoped agent inspector. Use
`/panels agents`, or F8 then `a`; `j`/`k` and literal row clicks select an agent,
`t` previews an eligible existing transcript, `b` returns to jobs, and F9 returns
to the original conversation. `/panels jobs` opens host jobs directly.

The scope is explicitly **this bound host and the last 100 native auxiliary
observations**, not a workspace-wide or cross-harness roster. Task/run/assistant,
generation, current session, native-child request and parent-session identities
are shown. Parent links are provenance, not dependency edges. Activity comes
from the SDK or auxiliary runtime; child token usage and health remain unavailable.
The bridge state includes the immutable host identity and bounded native-child
observations without giving it new commands or lifecycle authority.

Auxiliary inspection reuses existing prepare/start/finish attribution, including
queued, completed, uncertain and not-started outcomes. It does not launch work.
Web-search references are not eligible SDK transcripts. Missing/disconnected
agents remain missing/stale; selection is by identity, not a shifting row index.

Transcript reads resolve only exact host-owned references, recheck the binding
and selected identity, verify the existing JSONL session header, and read at most
24 KiB / 200 source lines. Ordinary-file and no-follow/nonblocking-open guards
reject missing, symlink and special-file sources. They do **not** call
SessionManager.open, switchSession, resume, create, migrate or rewrite a session.
The preview is a first-page raw JSONL snapshot, not an editable/live session.

Jobs reuse JobRuntime.inspection's existing bounded in-memory tail, timestamps,
exit code/signal and retained line-monitor observations. Job details precede
potentially lengthy receipt history. Selection retains job identity; missing,
lost or disconnected sources cannot expose output as current. A line match is
not service health, and exit is not gate/task acceptance. Native runtime state
currently supplies running/exited, not a canonical lost-job feed; the inspector
says **lost status unavailable**, rather than inventing zero lost jobs. A supplied
lost observation is rendered without output. No new output bridge, kill, stop,
restart, resume or role-switch action was added. Advanced telemetry/job-control
investigations remain separate.

## Acceptance evidence

- Identity/activity/usage/health and parent provenance: `src/ui/agent-view.test.ts`,
  `test/auxiliary.test.ts`, production-host PTY captures and bridge readbacks.
- Transcript eligibility and non-revival: `test/transcript-preview.test.ts` checks
  byte/line limits, header mismatch, missing/symlink/special files, and unchanged
  bytes/mtime of a legacy-version transcript; inspector input tests refuse stale
  and disappeared selections without reading a different agent.
- Host jobs: `test/jobs.test.ts` exercises real disposable process groups and
  retained inspection of completed output and readiness-line observations;
  `src/ui/agent-view.test.ts` covers missing/lost/disconnected adapters.
- Narrow behavior/authority: native main-view input tests at 40 columns plus
  real production-host owned-PTY F8/a/t/q/F9 input at 140 and 52 columns. Bridge
  readbacks show unchanged bound/current session, generation and turn during
  transcript inspection. Original draft/conversation and role refusal checks pass.

## Checks run

From `packages/corral-pi`:

- `bun run typecheck`: PASS.
- `bun test src/ui/ test/transcript-preview.test.ts test/auxiliary.test.ts test/auxiliary-host.test.ts test/jobs.test.ts test/jobs-host.test.ts test/native-panels.test.ts test/panel-host-proof.test.ts`:
  **125 pass, 0 fail** (21 files).

From the worktree:

- `python3 scripts/qa-pi-panel-proof.py .evidence/T-693/pty`: PASS. Uses the
  production host/main.ts and InteractiveMode in a new owned PTY with isolated
  HOME/XDG, offline provider/control-plane fixtures, real read tool and actual
  process-group jobs. Captures running/armed monitor, observed line match, bounded
  output, and exit 7. No owner/PM/worker pane was driven and no paid calls made.
- `git diff --check`: PASS.

The older `qa-pi-workbench.py` probe timed out at its Changes/diff stage, before
job inspection. That unrelated scenario was not repaired; the production-host
panel proof above now independently captures the actual job scenario. The first
iteration of the new transcript probe waited for an unchanged title to be
repainted after modal close; it was corrected to await a complete synchronized
frame (Pi legitimately uses diff rendering), then passed.

`cxm context` could not build the task-start packet because its Corral list exceeded
the configured output limit; targeted source reads and a task-scoped Corral search
were used instead.

## Limitations

No cross-host/historical agent roster or canonical lost-job reconciliation is
claimed. Cross-harness health, per-child token accounting and service readiness
remain unavailable. There is no transcript continuation/paging beyond the bounded
first page, nor any new job/agent lifecycle control. Live provider calls and load
testing were unnecessary and were not run. The assigned receipted quick gate is
recorded separately by Corral after commit/rebase.
