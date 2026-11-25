# Chat as a workspace Code session

Status: **proposal** (Wave 3 handoff). No runtime changes in this slice.

## Decisions

Chat becomes Code at workspace scope with board tools. Quick Ask is a cheap
profile of that host, not a second runner. Any enrolled device may start full
Code with terminal Code's trust. Disclose that trust before full launch or
promotion; do not require a local terminal or desktop device.

Do not change project Code, ordinary Builds, or the PM binding. A workspace
chat is not a managed Build, task run, review lane, or approval grant. Keep
`BuildFacts.mode = Code` and add scope/profile facts rather than a separate
Chat mode for new rows (`crates/corral-protocol/src/build.rs`).

## 1. Workspace cwd and identity

**Today.** `Store::find_root` uses `CORRAL_ROOT` or an ancestor with
`.corral/corral.toml`; Git does not identify the board
(`crates/corral-core/src/store.rs`). Pi probes Git with `rev-parse`, keeps
workspace and repository roots separately, and can infer project by matching
the Git root to a configured project path (`packages/corral-pi/src/build/launch.ts`).
Daemon start chooses the configured project path or the daemon root as cwd;
managed Code launch still rejects a non-Git cwd. Resume seeds launch from
`record.repo_root` (`crates/corral-daemon/src/builds/operations.rs`). Terminal
Code can already launch outside Git, but that is not a daemon workspace target.

**Proposed.** Add an explicit workspace-chat launch target. Do not infer it
from a missing project, non-Git cwd, or the app filter. Resolve it on the host
to the canonical daemon store root, persist it before serving the row, and
allow it outside Git. Do not expose a device-supplied host path. Initial and
resumed cwd is the umbrella even if it happens to be under an unrelated Git
root. If unavailable, refuse; do not substitute another repo or directory.

- Identity is daemon store + durable Code id. Name, profile, focus, device,
  and checkout paths are not identity. Terminal and daemon resume use the
  same record and preserve scope. No synthetic repo, worktree, or fallback
  to a first project.
- Serve Workspace scope, no bound project, no fabricated branch/base, and no
  aggregate diff or attribution inferred from concurrent repo activity.
  Repo facts require an explicitly resolved repository.
- Resolve board actions by authoritative task/project ids, not cwd or UI
  scope. Paths are relative to the umbrella unless a tool explicitly resolves
  a configured checkout. The owner can discuss or work across independent
  repos without changing the board or session identity.
- `quick` is an alias to a concrete Code id in the store. Serialize first-send
  creation; after acceptance, bind command ids to concrete sessions so retry
  cannot land in another thread after a reset.

## 2. Board tools and confirmation

**Today.** Pi has assistant contract/tools and Code-specific role conditionals
(`packages/corral-pi/src/roles/assistant/index.ts`,
`packages/corral-pi/src/roles/build/index.ts`). The assistant's typed owner
operation validates cited owner input and checks confirmation/provenance
before committing (`crates/corral-ops/src/agent/ops/owner_action.rs`,
`crates/corral-ops/src/assistant/owner_action.rs`). Its current classifier
marks forced moves, moves to Dropped, discard, config and operating mode
changes Confirm (`crates/corral-ops/src/assistant/owner_action/kinds.rs`).
Attachment-origin input can also trigger confirmation for otherwise ordinary
actions. The older assistant plan is not the live action allowlist.

**Proposed loadout (full Code):**

| Intent | Route and authorization |
|---|---|
| List/find tasks; inspect runs, reviews, plans, artifacts and citations | Bounded board reads; no confirmation. |
| File an idea/task, comment, attach an artifact, hand off to PM | Typed operations on request; attribute to this session; no separate confirmation. |
| Dispatch, normal move, approve and other supported ordinary owner actions | Explicit owner instruction and typed receipt. Preserve existing engine, approval, landing and provenance checks. |
| Force move, move to Dropped, discard, config or operating mode change | Separate host-rendered confirmation of exact target and operation. Decline or stale target changes nothing. |
| Delete non-scratch worktree or branch | Not a chat board tool in v1; separate owner confirmation if added later. Never reinterpret Code Remove. |
| Ambiguous or unsupported mutation | Clarify, offer PM handoff, or link to the normal owner surface. No guessed action or autonomous PM operation. |

Adapt the receipt path to authenticate a bound Code identity; never forge an
`A-n` record. Journal real terminal or enrolled-device owner input with origin,
generation and command id. Bind approval to the exact proposal/origin and
return the recorded result on replay. Attachment/context text, memory, job
output and agent messages cannot authorize. Reuse current engine operations,
not new ways around gate/review/landing. Do not fall back to shell CLI as the
owner for a refused or absent board operation; preserve role attribution in
`crates/corral/src/entry/class.rs` and `crates/corral-ops/src/command/route.rs`.
No raw state edits, pane injection or managed Build tools in the board loadout.

## 3. Write scope and shell

**Today.** The assistant's file-tool check classifies lexical and canonical
destinations: in-scope writes pass, out-of-scope writes ask, protected
Git/Corral/worktree paths refuse, and workspace Markdown memory has a narrow
exception. Unresolvable paths fail closed
(`packages/corral-pi/src/roles/assistant/write-scope.ts`,
`packages/corral-pi/src/host/tool-controls.ts`). A configured assistant without
a project requires scope in host setup (`packages/corral-pi/src/host/session.ts`).
Its shell guard addresses accidental pane input and state edits, not hostile
processes (`packages/corral-pi/src/roles/assistant/guard.ts`). `bash_rules`
has deny/ask/allow and a default; deny takes precedence
(`crates/corral-core/src/agent/local_controls.rs`). The trusted Code branch
retains plan restrictions but bypasses fixed shell, write-scope and bash-rule
policy (`packages/corral-pi/src/roles/build/guard.ts`).

**Proposed.** Full workspace chat has Code's trust; do not merely widen the
assistant root. It may edit owner checkouts without assistant path prompts and
is not confined to enrolled repo paths by the OS. Keep Code plan restrictions.
Legacy assistants keep their current guards. Do not silently copy assistant
`bash_rules` onto full Code or claim that they still apply. An optional Code
policy, if wanted, should apply equally to terminal and daemon Code.

Board confirmations govern board tools, not arbitrary programs executing with
the owner's privileges. Quick exposes no shell or job execution in v1; it is
not a full shell with a deny list. Neither flavor claims an OS sandbox.

## 4. Memory and task focus

Keep workspace Markdown under `.corral-next/memory/`, not in device-local
prompt history. The current write exception is Markdown in that subtree, not
arbitrary `.corral-next` files. The structured store validates metadata and
never treats memory as authority (`crates/corral-core/src/memory.rs`); operator
CLI and bound-agent authority differ (`crates/corral/src/memory_cmd.rs`).
Use the memory service for model reads and saves, with bounded lookup,
author/source metadata and no bulk prompt injection. Full can save on request;
Quick can look up, not save. Do not copy old transcripts into memory or let a
note execute a pending operation.

Today daemon chat stores optional task focus and derives a project for launch;
`corral pi --task` opens a focused assistant (`crates/corral-daemon/src/assistant.rs`,
`crates/corral/src/pi_cmd.rs`, `crates/corral-ops/src/agent/pi.rs`). Store focus
separately from workspace Code's scope. Load a bounded current task brief:
id, project, status, spec and links to runs, reviews and artifacts. Focus
survives resume and refreshes facts without changing cwd or authority. Make
change/clear explicit; a missing task is not replaced by another by title.
Attach-current-view is per-send context, not a focus change, memory write,
project selection or owner confirmation.

## 5. Quick profile

Today `quick` is an alias; its chosen Pi profile is not a price guarantee
(`crates/corral-daemon/src/assistant.rs`). Name and configure an explicit cheap
qualified model, lowest supported effort, and bounded context, tool results
and output. Use the Code launch model/effort plumbing
(`crates/corral-engine/src/runner/pi/build.rs`, `packages/corral-pi/src/build/launch.ts`).
Apply the budget to compaction and fallback too. If unavailable, report failure
rather than silently switching to a more expensive model.

Quick answers, summarizes, drafts, and reads bounded workspace files, board
facts, memory and attached context. No edits, shell, jobs, delegation, memory
writes, board filing or owner mutation. Enforce the profile at tool
registration and execution, including extension and indirect entry points;
prompt instructions alone are not enforcement. Reuse Code's journal, lifecycle
and transcript rendering.

**Continue in Code** explicitly opens full workspace Code, carrying selected
context and a link to the Quick exchange, not automatically replaying an
imperative request. Quick retains its id/profile and remains the default for
the next Ask. A model fallback, attachment or tool suggestion never promotes.

## 6. Existing `A-n` chats and cutover

Daemon `assistant.rs` already backs named and quick chats with Pi; it resolves
`A-n` separately from `quick` and has start-on-send and relaunch paths. Do not
rewrite historical records or load an assistant journal as trusted Code.

1. Ship workspace Code before routing Ask to it. Preserve `A-n` transcript,
   links, pins/docks, artifacts and usage. Preserve live turns and resume on
   the old host and guards during compatibility. Keep separately bound
   assistants and `corral pi` unchanged.
2. Advertise the new route via daemon capability. Updated clients create new
   chats as workspace Code and use the new Quick binding; old clients retain
   the legacy route. Explicit `AssistantSend` to `A-n` stays legacy. Preserve
   ack/dedupe, and do not translate legacy management verbs to Build verbs.
3. Offer explicit continuation from old chats with selected context and a
   source link. Never carry queued actions, old authorizations, or pending
   confirmations into Code. Retarget a dock or pin only by choice. No double
   billing. Retire guarded legacy send/resume after owner signoff; rollback
   changes defaults, never downgrades or deletes Code records.

## 7. Ask surfaces

App paths below are under `apps/corral-app/src/`. Current Ask uses the
assistant key (`model/assistant/live_act.rs`); change the binding without
coupling it to the desktop dock's selected session.

| Entry | Retain | Change |
|---|---|---|
| Desktop pane / Home composer | Resizable beside, narrow overlay, arbitrary-session dock (`shell/pane.rs`, `ui/ask/drawer.rs`, `model/dock.rs`). | Ask defaults to Quick Code; Full Code can be docked too. Opening its page uses the same id and one conversation. Page navigation and attach-view do not retarget it. |
| Terminal app / CLI | Drawer sizing (`ui/term.rs`); stdin, files, ack and `--pm` (`crates/corral/src/ask_cmd.rs`). | Apps and updated CLI send default Ask to Quick. Full terminal workspace chat uses explicit Code scope and same resume id. No `--pm` or ordinary project Code changes. |
| Phone | Floating Ask and compact/share entries (`shell/phone/`, `ui/ask/share.rs`). | Same daemon Quick, same attachments and acks; full Code opens as a session page with trust disclosure, no dock needed. |
| Native global shortcut | Existing configurable hotkey and window recovery (`os_integration.rs`, `ui/ask/hotkey.rs`). | Open the same Quick binding; shortcut alone starts nothing. No new shortcut in browser, terminal or phone. |

Sessions shows Code + Workspace + profile, not a second Chat row. Legacy is
still Chat; preserve filtering/sorting and show only meaningful preview facts.
Keep Code's managed remove/relaunch/terminate/exit refusals; cold Code uses
Code resume. Show confirmations on sending surfaces or link to one that can
answer; compact views must not turn confirmations into free text.

## Release checks

Use an umbrella fixture with its own Corral store and two sibling Git
checkouts. Include the inverse case: the umbrella happens to sit inside a
third repository. In both cases launch and resume full and Quick with the
same board identity and cwd; neither case should show a third repository's
branch as workspace metadata. Verify that sending focus for a task in the
second repo does not change where a later relative path resolves.

Exercise the authority matrix with actual typed calls. A board read needs no
owner message; an ordinary owner action needs a real one; a destructive
proposal needs an answer tied to the proposal and originating session. Attempt
the same operation from a quoted artifact, from a prior generation, and after
a reset; none can satisfy the checks. Replay a valid accepted request and
verify that the engine commits once. Preserve existing refusal diagnostics.

For Quick, check the effective tool list and a direct attempted invocation
of a disabled tool, not just the visible model label. Assert no process or
mutation starts. Check that a missing model and exhausted budget produce a
visible failure instead of switching capability or cost tier. Launch full
from the same device and verify that it still has ordinary Code file/shell
capability, while its board tools still use typed confirmations.

Exercise both generations of clients during cutover. Send from two updated
surfaces into one Quick alias, then reopen by concrete id. Replay a command
after rotation and verify no duplicate turn in either journal. Open an old
`A-n` citation while an old live turn finishes on its original host. Disconnect
and reconnect without changing which session owns any pending question.

Test host restrictions at the lowest layer, protocol routing with daemon
fixtures, and surface behavior with app headless journeys; no live herdr
needed. Do not pin incidental layout or copy. Native hotkey behavior belongs
in a supported desktop smoke check, not a build on this handoff host.

## Open questions for the owner

1. Which qualified cheap model, effort and output limits should ship? Allow
   explicitly priced same-tier fallback, or fail without fallback?
2. Keep Quick read-only with explicit new-session continuation, or allow
   limited filing or a same-id promotion?
3. Add optional `bash_rules` for both terminal and daemon Code, or keep current
   terminal Code parity without a new policy?
4. How long should guarded legacy `A-n` send/resume and old quick clients work?
5. Use one shared Quick per workspace as proposed? If so, does **New Quick**
   rotate it for every enrolled device or only the initiating device?

## Proposed tasks

| Title | Outcome | Constraints | Dependencies | Acceptance checks | Size | Difficulty |
|---|---|---|---|---|---|---|
| Workspace Code launch and identity | Explicit scope, same id on daemon/terminal resume | No synthetic Git, worktree or device cwd | Existing managed Code | Two independent repos: umbrella cwd on start/resume, no fake branch; project Code unchanged; managed cleanup refuses | l | hard |
| Workspace board reads and filing | Board-wide lookups and attributed filing, artifacts and PM handoff | Explicit ids, bounded results, typed mutations | Workspace identity | Read across both repos; file with correct project/session; unsupported mutation refuses | m | standard |
| Code owner actions | Typed receipts and owner confirmations | No forged assistant id or landing bypass | Identity, board tools | Terminal/device requests succeed once; confirmed force/drop/config only; cancellation and replay cannot repeat mutation | l | hard |
| Memory and task focus | Bounded shared Markdown lookup/save and durable focus | Memory is not authority; cwd never changes | Identity, read tools | Focus survives resume; authorized memory shared; missing task is surfaced, not replaced | m | standard |
| Quick profile and binding | Low-cost tool-limited Code and explicit continuation | No shell, indirect mutation or automatic upgrade | Identity, read tools, owner profile choice | Concurrent first sends use one id; absent model is visible; forbidden tools cannot execute; continuation preserves link | l | hard |
| Legacy compatibility and cutover | Preserve `A-n`; negotiate new route | No journal rewrite, copied approvals or duplicate usage | Quick binding, owner compatibility choice | Old dock/link and live turn work; retry stays in original id after rotation; rollback preserves both histories | m | hard |
| Switch Ask surfaces and document trust | Route all four surfaces and update `docs/guide/corral-pi.md` | Keep dock, sizing and `--pm`; no managed controls | Cutover, Quick, owner-action answers | Same id in pane/page; phone can launch full; shortcut dismiss starts nothing; CLI acks and `--pm` unchanged | m | standard |
