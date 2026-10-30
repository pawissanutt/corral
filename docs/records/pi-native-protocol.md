# T-136: candidate native PM protocol and successor boundary

Updated October 18, 2026. **Production contract remains unimplemented.**
The bounded probe selected a thin SDK host using upstream `InteractiveMode`
and exercised the six native criteria. This is a feasibility result, not
a shipped Corral protocol or permission for production integration.
See [compatibility and raw evidence](pi-native-compatibility.md), including
the preserved historical October 17 authentication-blocked result.

## Measured disposable protocol, not production v1

`tools/pi-probe/native-host.mjs` creates one upstream terminal session in a
fresh private plugin-free herdr fixture. Public `ModelRuntime` accepts the
read-only in-memory subscription credential store; upstream `InteractiveMode`
owns the UI and session rebinding. There is no second RPC conversation.

The launcher creates a mode-0600 private bootstrap containing a random
capability and non-secret scope. The local Unix socket is mode 0600 inside
the private root. Each request binds ID, capability, store/root/role/project,
saved fixture socket/workspace/pane, native session ID and generation.
Wrong scope/capability and stale generation fail before dispatch. Lifecycle
subscribers must first authenticate; replacement fences old subscribers and
requires explicit client rebinding. This is trusted same-user fixture
isolation, not peer-credential verification or process containment.

Actual fixture commands are fixed `state`, `start`, `steer`, `follow_up`,
`release`, `cancel`, `handled`, `ask`, `permissions`, `mutate`, `reload`,
`switch_model`, `replace` and `fail`; no arbitrary code/text/tool/CLI command
can be submitted. The hold/release and injected-failure commands are
test-only synchronization, not production transport requirements.

Frames are JSONL bounded to 64 KiB; at most 64 in-memory request receipts
are retained. Identical IDs/payload digests share an in-flight result or
return the earlier receipt; conflicting payloads fail before execution.
Capacity is checked before effects. Events have a monotonic sequence and
current generation. Event waits are subscribed before actions with a
45-second deadline; the parent watches the atomic result file before launch
with a 150-second overall deadline. These are tested fixture bounds, not
negotiated production concurrency or performance guarantees.

The native UI dialog authorizes exactly one synthetic private fixture
increment. It is answered by the test driver through its private terminal,
never by a model actor field. Lost-ack replay, pre-approval rejection,
reload, same pinned-model selection and native session replacement were
exercised. Its pending action and receipts are **not durable across process
death**. Native cancellation is tested while a provider payload is held
before HTTP; injected provider failure also occurs before HTTP.

Only the readback tool is declared. Deferred fixture mutation and denial
sentinel are explicitly registered so nested-hook enforcement is actually
exercised. A labelled synthetic assistant parent and successful nested
readback baseline prevent missing-context errors from masquerading as
permission denial. Shell/edit/proxy tools and inherited resource discovery
are absent/denied, including after reload/replacement. This does not make
trusted extensions an OS sandbox.

Four requests total used the existing OpenAI subscription, 27 reported
output tokens total, each wire-capped at 1,024; $0 paid API, no retry,
refresh or fifth request. The non-secret persistent budget ledger is shared
by route and terminal probes and must never be reset for more calls.
All private servers/fixtures were removed. The original offline probe stays
unchanged and credential-free.

## Ownership and roles

Rust `corral-ops` and canonical Corral records remain the only task-policy
authority. Pi owns reasoning, context and tool execution. A Pi transcript,
extension entry, tool result or settled event cannot approve a plan, satisfy
a gate, accept review or mark a task Done.

The launcher, never the model, binds this handshake identity:

```text
protocol_version, negotiated_capabilities, store_identity, workspace_root,
authorized_project_scope, role, native_session_id, session_generation,
task_id?, run_id?, herdr_socket, herdr_workspace_id, herdr_pane_id
```

A PM has null task/run fields and one explicit authorized project scope.
Do not fabricate a worker run to fit today's run-only obligation records.
Worker/reviewer identities and native workers remain outside this scope.
The canonical native PM obligation container and exact record paths are
**unresolved**; no schema is being added to `state/pm.json`.

An eventual Linux bridge candidate is a Unix socket in an owner-only
directory, with safe creation, peer identity and a per-launch capability
delivered by private inherited channel. Neither UID nor socket possession
alone establishes the store or current generation. Do not expose bootstrap
capabilities in model context, tool arguments, CLI arguments, logs or config.
Provider credentials and bridge capabilities are different secrets.

Before accepting commands/events and again under the board writer lock before
mutation, check saved store/project/role/session generation and actual saved
herdr socket/workspace/pane. Reject stale, foreign, cancelled or differently
scoped callbacks. Changing current config, supplying a task ID or colliding
pane IDs cannot transfer authority. Restart/replacement fences the old
generation. Reload/reconnect must rebind listeners and prove current identity;
lost events remain uncertain until canonical reconciliation.

Extensions have the user's OS privileges. This is a trusted local extension
design, not process containment of malicious same-user code.

## Candidate v1 envelope and limits

These concrete bounds are **proposed for production successor review**, not
the implemented disposable wire format above or performance-tested values.
A successor must accept or revise them against
the actual selected API. Unknown major version/capability fails closed.

```json
{
  "version": 1,
  "request_id": "opaque-stable-id",
  "payload_sha256": "digest-of-canonical-action-payload",
  "session_id": "launcher-bound",
  "generation": 1,
  "kind": "submit_input",
  "payload": {"text": "bounded", "busy_policy": "follow_up"}
}
```

| Boundary | Proposed limit and behavior |
|---|---|
| Frame | UTF-8 JSON, LF terminated; 64 KiB maximum encoded frame, reject before unbounded allocation. U+2028/U+2029 are not frame delimiters. |
| Input/readback | 8 KiB input text; 32 KiB result; mark truncation with authorized continuation reference. No credential content. |
| Concurrency | One input-starting command at a time; at most 4 control requests in flight and 8 queued deliveries; return backpressure rather than silently drop. |
| Deadlines | 5 seconds handshake; 10 seconds acceptance/state/cancel receipt; 60 seconds fixture operation deadline, followed by explicit unknown/lost and cleanup. No completion invented from timeout. |
| Reconnect | One bounded reconciliation attempt per diagnosed disconnect; no automatic replay of uncertain mutations. |
| Events | Monotonic sequence per generation plus applicable request/operation/question IDs. Gap => reconciliation requirement, not guessed correlation. |
| Live allowance | Max 5 actual provider requests, max 1,024 output tokens/request, subscription only, $0 API spend. Retries/continuations/compaction count too. |

Possible commands are `hello`, `get_state`, `submit_input` with explicit busy
policy, `cancel`, `register_question`, trusted `answer_question` and `shutdown`.
Registering a pending question must return promptly; the runtime/controller
owns asynchronous subscriptions outside model tool execution.

Every input produces a truthful acceptance disposition: started, queued,
handled or rejected. An extension-handled command need not produce any model
run. SDK prompt completion, RPC prompt acknowledgement and Pi final settlement
have different contracts. They cannot share a single success flag.

## Lifecycle map and receipt obligations

This maps production receipt requirements. The disposable subset measured
above does not provide their canonical persistence or causal reconciliation:

| Candidate fact | Possible upstream/controller observation | Required meaning |
|---|---|---|
| `accepted` | Explicit injection/SDK/RPC disposition | Delivery was accepted in its stated way; not execution or completion. |
| `running` | Structured agent/turn/tool events | Session activity; attribution requires a known causal operation, not latest request. |
| `awaiting_owner` | Canonical question registered + trusted terminal dialog | Named decision remains pending across turn end or restart. |
| `waiting_external` | Corral obligation + actual runtime subscription | Cursor/scope/cleanup duty survive; tool returns the obligation identity. |
| `failed` | Provider/tool error or controller process failure | Retain earlier acceptance; distinguish command rejection, provider failure and lost process. |
| `cancelled` | Abort completion plus controller fence | Identified work cancelled; committed mutations, pending questions and cleanup survive. |
| `turn_ended` | `agent_settled` | No automatic reasoning continuation; not task Done. `agent_end` alone is insufficient. |
| `session_replaced` / `lost` | Structured lifecycle or supervised process exit | Fence previous generation; rebind or report uncertainty without terminal inference. |

Each receipt binds request, payload digest, store/session/generation, event
sequence and operation identity. Pi events generally lack command IDs; an
adapter needs an explicit causal mapping. Concurrent ambiguous activity
must remain session-scoped, not be attributed to the last submitted input.
Record accepted delivery separately from model start, settled reasoning,
durable obligations and validated task completion.

Required future tests include handled input without a run, starting/busy
steering and follow-up, cancellation races, provider error after acceptance,
reload/replacement/process death, missing acknowledgement and event gaps.
Subscribe before triggering and await exact signals with bounded deadlines;
no fixed sleeps or screen/transcript parsing.

## PM tool boundary and first-mutation prerequisite

| Operation | Boundary |
|---|---|
| Brief/next, task/list/search/route, artifact/lesson readback | Enumerated typed Rust operations, bounded output, authorized project scope; re-read before any subsequent action. |
| Capture/comment/reply/planning/dispatch | Future policy-authorized operations only, after durable request receipts and exact-action enforcement exist. No arbitrary command-string wrapper. |
| Owner decision | Separate trusted terminal/board callback, not a model-callable `actor=owner`, inferred natural-language approval or `owner_ok` field. |
| Gate/review/completion | Present current evidence or request permitted workflow; cannot manufacture receipts, impersonate reviewer, waive gate or land directly. |
| Shell/edit/proxy/config/accounts | Denied. Disable built-ins, inherited extensions, MCP, skills and project/resource discovery unless explicitly reviewed and allowlisted. |

An active declaration allowlist is insufficient: nested `executeTool`,
codemode, deferred tools, search/activation, resource discovery and resumed
loadouts must not restore forbidden capabilities. Enforcement must cover the
actual execution path and the Rust operation boundary after reload/model
switch. The spike demonstrated the bounded disposable configuration above,
not Rust-operation authorization, arbitrary models or hostile extensions.

Before the first production mutation, the successor must implement:

1. Stable durable request ID and canonical payload digest, bound to store,
   role, generation, task/spec/run and exact intended action.
2. Under-lock current authority/policy/identity validation. Reusing an ID with
   different content fails; identical replay returns the stored result.
3. Atomic mutation + canonical receipt where possible. External effects use
   reservation/journal reconciliation, never blind replay after lost ack.
4. Canonical pending decision/action revision and trusted owner callback.
   Restart cannot clear it; approval of a changed action is rejected.
5. Lost-ack-after-commit, conflicting-payload reuse, restart-with-pending-
   approval and stale/foreign callback tests before enabling capture,
   comments, dispatch or any other production mutation.

Slice 2 from P1 must remain read-only/fixture-only until that subset lands.
Do not postpone these controls until the later unattended-recovery slice.
The successor must name the actual durable paths in its write scope;
this spike deliberately does not invent or implement an authoritative store.

## Package, credentials, upgrade and rollback

Candidate source location: `packages/corral-pi/`, owning a first-party
extension, concise PM context and explicit resource manifest. This directory
does not exist as a result of the spike. Production packaging selection is
conditional on the transport decision and successor approval.

Follow upstream host-provided peer dependencies for `pi-ai`, `pi-agent-core`,
`pi-coding-agent`, `pi-tui` and `typebox`; do not bundle a second agent runtime.
Because upstream requests `"*"` peer ranges, enforce tested versions through
Corral's compatibility manifest/startup check and a full resolved installation
lock, not floating peer resolution. Ship installed assets, not paths into a
developer checkout. Use explicit invocation-local resource paths, not edits
to personal/global Pi configuration.

Node is an explicit prerequisite candidate; this spike tested v24.13.1 only.
No runtime installer, automatic download, new platform or Bun support promise
is included. The disposable installation's top-level integrity and lock digest
are evidence, not a complete production dependency strategy.

The October 18 owner authorization permits existing compatible subscription
reuse **for this spike**, superseding the earlier no-reuse boundary. The
Codex file is read-only, bounded and never copied; only the selected
unexpired subscription access credential is passed to public
`CredentialStore` in memory. Refresh/write/logout callbacks are denied before
invocation, owner bytes/inode/mode are checked unchanged, and no fixture
auth file is created. No environment API key, paid fallback, owner
login/config write, whole auth-directory copy or secret artifact is allowed.
This is not blanket permission for a future production credential adapter;
its owner-controlled provisioning and refresh policy require successor
review. Never log OAuth URLs/codes, account IDs, headers or token responses.

For a future upgrade:

1. Pin candidate package/integrity/source commit, runtime, full dependency
   lock, model/provider and protocol compatibility manifest.
2. In a private fixture, rerun actual visible-terminal lifecycle, permission,
   owner-action, mutation-replay, recovery and packaging checks within a
   separately authorized live budget. Documentary changes do not pass them.
3. Quiesce/fence the old session; preserve pending questions, waits/cursors,
   receipts and external-effect reconciliation duties in canonical records.
4. Switch only after reviewed acceptance; negotiate capabilities and reject
   unsupported startup. No implicit upgrade or schema conversion.

The production host now negotiates optional features through `compat.json`'s
`capabilities` list (T-563): an unavailable default is reported and omitted, and only an
explicit request the host lacks refuses startup. See
[capabilities](pi-native-capabilities.md).

Rollback disables the opt-in integration and fences/stops only its owned Pi
resources, restores the previously configured OmO PM and supplies a canonical
brief/next/questions/waits handoff. Keep tasks, artifacts, mutation receipts
and pending decisions. Do not replay ambiguous actions or promise transcript
portability. Preserve unrelated workers, reviewers, gates and the owner herdr
server. Incompatible durable migrations require their own reviewed recovery
plan.

Current-spike rollback was fixture cleanup only; no integration was enabled,
no production state migrated and no old-harness runtime rollback was tested.

## Conditional successor estimates, not children

The production no-go blocks scheduling implementation. These are engineering
allowances, not measured elapsed time or approvals. A successor reviewed plan
must review the measured SDK transport and resolve production packaging,
credential refresh and authority boundaries. Authentication and the bounded
six-criterion native proof are no longer missing.

| Later slice | Revised allowance | Required ordering / uncertainty |
|---|---|---|
| Review spike and production contract gaps | 1-2 engineer-days | Bounded proof passed. Scope production credential policy, installed assets and canonical records; do not spend more live calls under this authorization. |
| Bundled read-only PM + first mutation safety | 1-2 engineer-weeks | Package/control bridge, canonical PM container, durable request/payload receipts and exact-action owner binding before any mutable PM tool. Higher than a tool-registration-only slice. |
| Lifecycle and recovery | 1-2 engineer-weeks | Durable subscriptions/cursors, replacement/reconnect, cancellation races, ambiguous effects and legacy coexistence. Exact persistent paths require review. |
| Opt-in packaging and evaluation | 4-6 engineer-days | Installed assets, clean prerequisites, matched replay/live comparison and actual rollback. Separate owner budget/thresholds before live evaluation. |

Total later PM work: roughly 3-5 engineer-weeks after successor approval.
Remote cancellation, real owner decisions, restart recovery and installation
could still change feasibility; none was proved by this bounded fixture.
No native-worker estimate is
converted into an executable scope. No default switch, later child, PM lease,
fork or production integration follows from these reports.
