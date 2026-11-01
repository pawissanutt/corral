# T-690: native composer, delivery status and model controls

## Outcome

The rich panels retain Pi's real CustomEditor and its submit/steer/follow-up path. No second submit loop, agent runtime, model catalog or role selector was introduced. A non-submitting observer reports upstream preflight for the last typed input: pending, started, queued, handled or rejected. Started is explicitly not completion; queued is explicitly not execution. Status lives in the existing editor border, so it does not grow the dock or disturb reading anchors.

Model-label clicks invoke the same native picker action as Ctrl+L. Effort clicks invoke the same supported-level cycle as Shift+Tab, with no action for a non-thinking model. T-641's model/credential preflight is unchanged. Native busy/credential refusal messages retain the current model, session file and binding.

The pinned SDK patch adds an opt-in readOnlyQueue mode for rich panels: actual pending counts replace raw queued text, dequeue cannot edit/cancel/reorder it, and Esc aborts the current operation without importing queued notices into the owner's draft. Bare/plain/headless defaults remain separate. Role labels and bound-session replacement guards are unchanged.

## Acceptance evidence

| Criterion | Proof |
|---|---|
| Typed input reaches the existing path once; truthful dispositions/counts | `panel-composer.test.ts`: real keyboard edit/backspace/Unicode submission, held preflight, held stream, handled/refused input; one user message; exact steer/follow-up queues. Existing host/bridge tests cover idle/busy rejection, pending settlement and idempotent/conflicting replay. |
| Drafts survive views/resize; notices do not flood or become owner input | Real editor survives F7/F8/F9 and 140/80/52 relocation. Twelve incoming wake fixtures appear as counts, not raw composer text. Dequeue and current-operation abort retain the draft and all thirteen follow-ups. Existing native and shell tests retain selection, modal isolation and reading anchors. |
| Only available models/efforts; actionable refusals | Native pointer picker selects a second available reasoning model. Supported effort cycling is tested by pointer and keyboard. Busy and removed-credential attempts cannot commit model/session changes; credential refusal retains the corral login command. Existing assistant/PM subscription tests cover GPT/Claude/GPT and removed credentials. |
| Role is a label; no unsupported queue/session controls | Read-only queue presentation/dequeue fence; unchanged role loadout and session replacement guards. Production-main PTY proof rejects a forbidden tool and native new-session request, with one hello, one bound session and generation 1 throughout. |
| Deterministic and real input evidence | Three new integration tests use the real HostSession/InteractiveMode/editor, deterministic held input/stream barriers and SGR mouse reports. The owned-PTY script drives production main, native picker/effort, resize and refusal paths. No owner/PM/worker pane is driven. |

## Checks

From `packages/corral-pi`:

```sh
bun install --frozen-lockfile
bun run typecheck
bun test test/panel-composer.test.ts test/host-session.test.ts test/bridge.test.ts test/subscriptions-host.test.ts test/native-panels.test.ts test/panel-host-proof.test.ts test/panel-shell.test.ts src/ui/panels.test.ts test/package.test.ts
```

- Frozen install and typecheck: PASS.
- Targeted regression set: **55 PASS, 0 FAIL**, 1,225 assertions across nine files.
- New composer tests were first retained red: all three failed against the initial tree (missing pending/queued status and pointer controls).
- A development check caught dock-height/reading-anchor interference from a footer status row. Keeping status inside the existing editor border restored the native/shell regressions without weakening their assertions.

From the worktree root:

```sh
python3 scripts/qa-pi-panel-proof.py .evidence/T-690/pty
```

PASS: real owned PTY, isolated HOME/XDG, offline provider/control plane; native picker and effort activated with SGR pointer reports; draft retained; streamed tool/reply, binding/bridge checks, 140/80/52 resize, forbidden-tool and session-replacement refusals, clean terminal exit. The separate assigned quick-gate receipt/log is retained by corral.

## Retained captures

Attached artifacts include red and green test logs, typecheck, PTY result/log and bridge-state readbacks, native ANSI traces, and screenshots replaying those traces through pinned xterm.js 5.5.0. The picker/minimum-width screenshots replay production-main PTY input; the notice-count screenshot replays the real in-process terminal integration test. Replay is evidence only, not product code or mock UI state.

## Limits

No paid/live provider request, upload, new provider, owner-pane interaction or load test was performed. Provider/control-plane I/O is deterministic fixture I/O in these checks. Pending counts and local preflight are observations/admission, not canonical inbox execution or task-completion receipts. Queue management remains out of scope.
