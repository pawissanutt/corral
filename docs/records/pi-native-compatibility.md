# T-136: Pi-native PM compatibility decision

## October 24, 2026: LSP extension dependency review (T-309, plan T-252@P5)

**No dependency change.** The worker's LSP extension
(`packages/corral-pi/src/extensions/lsp/`) adds no package. Its JSON-RPC
framing and client are first-party code over `node:child_process` and
`node:fs`. The language servers are external executables that are started by
argv from `PATH` (`rust-analyzer`, `typescript-language-server --stdio`). They
are not npm packages and are not pinned here. A server that is not installed is
the explicit tool error `lsp_server_unavailable`.

The review followed step 1 of the upgrade procedure in the
[protocol record](pi-native-protocol.md#package-credentials-upgrade-and-rollback),
checked against the T-298 pins:

| Pin | T-298 GO value | After T-309 |
|---|---|---|
| `package.json` dependencies | `@earendil-works/pi-coding-agent` `1.3.0` | unchanged |
| `package.json` devDependencies | `@modelcontextprotocol/sdk` 1.25.2, `@types/node` 24.10.0, `typescript` 6.0.2 | unchanged |
| `bun.lock` raw SHA-256 | `292af16cf0f192abd1b8038754b48551994dddd5a2eee83f91dfa3bce9c9fd48` | unchanged; `bun install --frozen-lockfile` passes |
| `compat.json` | derived from `tools/pi-probe/bun-go.json` | byte-identical |
| Bun range, Pi integrity, upstream commit, provider route | `1.4.2`, `sha512-V/Wo...nnw==`, `71023aec`, `openai-codex` | unchanged |

The diff is therefore empty, and steps 2 to 4 (fixture re-run, quiesce, switch)
do not apply. The `package.test.ts` pin checks still bind `package.json`,
`bun.lock` and `compat.json` to the GO manifest. The only observed server
version is the owner toolchain's rust-analyzer 1.94.1 (e408947 2026-03-25).
It is recorded as evidence, not as a pin.

The extension's tools join the worker loadout through a host seam the PM
approved for T-309. A role may declare `localToolNames` and `localTools()`.
The loader refuses a name that collides with an upstream tool, a `corral_` tool
or another local tool, and the host adds the declared tools to the loadout that
its `tool_call` guard enforces.

## October 23, 2026: Bun re-proof (T-298, plan T-252@P5)

**GO (offline) for hosting the pinned Pi on Bun 1.4.2.** The thin SDK host of
upstream `InteractiveMode`, its bridge and its fixture tools now run on Bun,
with `node`, `npm` and `npx` shimmed to exit 127. The pinned release is still
`@earendil-works/pi-coding-agent@1.3.0` (commit `71023aec`, integrity unchanged).
All six T-136 criteria report PASS in a private plugin-free
`scripts/test-herdr.sh` fixture with zero provider requests. The live column is
**NOT RUN**: no owner authorization exists for T-298. The machine record is
`tools/pi-probe/bun-go.json`.

Only the probe runtime, launch and lock checks were ported. The fake transport
and the synthetic credential are harness-port additions inside the approved
scope. No production code, default, profile or Corral record changed, and
upstream 1.4.0 is not adopted (the plan allows that only through the documented
upgrade procedure). The October 18 and October 17 sections below remain the T-136
Node record.

### Offline results

| Criterion | Steps (all PASS) | Offline mechanism | Difference from T-136 |
|---|---|---|---|
| Terminal control | `terminal_bound`, `trusted_terminal_answer` | Upstream `InteractiveMode` in a private herdr pane reports `mode=tui`, `hasUI=true`. The driver's own pane input answers the native extension dialog. | Same mechanism. |
| Busy input | `busy_steer_follow_up` | The fake transport holds the first response stream open. Steer and follow-up both return `queued`, then three fake responses complete with `stop` and deliveries are start, steer, follow-up. | T-136 held the live payload before dispatch. |
| Cancellation | `cancel_in_flight` | Abort while the fake holds an open SSE stream gives assistant final `aborted` and `agent_settled.aborted=true`. The fake observed the reader's cancellation, and the committed fixture increment stays 1. | T-136 cancelled before HTTP dispatch; this cancels a stream in flight. |
| Lifecycle receipts | `handled_without_run`, `lost_ack_replay`, `provider_failure_not_completion` | A handled command makes no request. A lost acknowledgement replays its stored receipt, and a conflicting payload is refused. HTTP 503 from the fake ends as `error`, then settled, with no retry and no completion. | T-136 injected the failure before HTTP. |
| Session ownership | `scope_and_generation_fencing`, `session_replacement_rebind` | Foreign capability, foreign project and stale generation are refused. Replacement rotates the generation and native session ID, the old identity is refused, and an explicit rebind succeeds. | Same. |
| Restricted tools | `restricted_initial`, `restriction_after_reload_and_model_switch`, `session_replacement_rebind` | Only `pm_readback` is active. Seven nested attempts are denied after an allowed baseline, and this holds after reload, model reselection and replacement. | Same. |

Counters from the recorded run: 5 fake-transport requests (3 busy, 1 cancelled,
1 failed), 1 observed abort, 1 failure, **0 provider requests** (the refusing
global `fetch` was never called), 0 harness-port failures and 0 upstream
incompatibilities. Every fake request carried a zstd body that Pi compressed
through Bun's `node:zlib`. The same request guard that checked T-136's live
requests validated each one: endpoint, subscription headers, model and the
1,024-token cap.

The credential is a synthetic Codex-shaped file with a fixed signature and a
synthetic account. It is read through the same read-only path, and the owner's
`~/.codex` is never opened offline. A live attempt without a new grant refused
before it read any credential (`live_check_not_authorized`, 0 requests). T-136's
consumed authorization is always refused, and each future grant gets its own
persistent ledger with T-136's limits: 5 requests, 1,024 output tokens each and
$0 paid API.

### Clean install and lock review

- `bun install --frozen-lockfile --ignore-scripts` into an empty fixture root,
  with a fresh private HOME and cache and Node shimmed, installed 202 packages.
  The lock bytes stayed unchanged, all 205 locked placements were installed at
  their versions, and each of the seven Pi packages (1.3.0) and `typebox` 1.3.27
  has exactly one copy.
- Peers: all 15 declared peers are satisfied or optional and absent. The optional
  peer `@modelcontextprotocol/sdk` of `@google/genai` resolves to the
  host-provided root dev dependency 1.25.2. The optional peers of `openai` (zod
  4.6.7, undici 8.10.2, ws 8.22.0) resolve to the single graph copies.
- Lock: `tools/pi-probe/bun.lock`, raw SHA-256
  `292af16cf0f192abd1b8038754b48551994dddd5a2eee83f91dfa3bce9c9fd48`,
  migrated by `bun install` from T-136's npm lock (raw SHA-256
  `d96d0a7303245992b782b25787e13e4bd0576ec3f1d00b3e22ceb39e1648472a`).
- Reviewed differences from T-136's lock (full list in
  `tools/pi-probe/lock-diff.json`):
  - `typescript@6.0.2` was added as a dev dependency for the probe's own
    typecheck; T-136 used a global binary.
  - The root declares Pi as exact `1.3.0` where npm wrote `^1.3.0`. The
    resolution is the same.
  - No resolution or integrity changed: all 201 T-136 name@version pairs are
    present with identical integrity.
  - There are five hoisting differences. npm nested `agent-base@7.1.4` under
    `gaxios`, while Bun nests it under `gaxios/https-proxy-agent`. npm hoisted
    `retry@0.13.1` and nested `0.12.0` under `proper-lockfile`, while Bun does
    the reverse under `p-retry`. All 408 dependency edges resolve to the same
    version in both graphs, so no package sees a different dependency.
  - Conclusion: no runtime difference. The tested Bun graph is T-136's tested
    Node graph plus the dev typecheck.
- Only Bun 1.4.2 was tested, so `bunRange` is exactly `1.4.2`. Widening it
  requires a new re-proof.

### Fixtures for later children

- `tools/pi-probe/fixtures/session-usage.jsonl` (for pi-usage) is a Pi 1.3.0
  session written on Bun with three responses of scripted usage (response IDs
  `resp_fake_1`..`3`). Expected per-category totals: input 1936, output 125,
  cacheRead 1536, cacheWrite 128, reasoning 24, totalTokens 3725
  (`session-usage.expected.json`, derived from the script alone).
- `tools/pi-probe/fixtures/session-dangling.jsonl` (for recovery) is a snapshot
  taken while one `corral_gate` call and one upstream `edit` call were in
  flight, so neither has a result (`session-dangling.expected.json`). The
  `corral_gate` payload is a placeholder until the worker role defines it.

### Failure classes and Bun observations

A step failure is classed `harness_port` (probe, fake transport, bridge or
fixture plumbing) or `upstream_incompatibility`. The second covers an assertion
on observed upstream behavior, a probe failure tagged upstream, or an exception
whose first stack frame is under `node_modules/`. The recorded runs had neither.
All issues found during development were on the harness side, and each was
fixed without suppression. Two were checked-JS typing errors in the fixture
generator, and one was that ShellCheck could not follow the sourced helper. The
fourth was that live mode read the credential path before it checked the
authorization; the check now comes first.

On Bun, the pinned adapter finds `zstdCompressSync` through
`process.getBuiltinModule` and recognizes `process.versions.bun`, so request
bodies are compressed as on Node. T-136's `node:test` suites run under
`bun test`; only the request-guard suite changed, to pass the new
authorization parameter.

### Not verified

- Live provider behavior on Bun: NOT RUN, because no owner authorization exists.
- Bun versions other than 1.4.2, and platforms other than Linux x64 (WSL2).
- Production packaging, durable receipts and real owner decisions. These T-136
  boundaries are unchanged and belong to later T-252 children.
- T-136's Node-only offline preflight (`run.sh`) was not re-run.

## October 18, 2026: authorized resumption and current decision

**GO for the bounded SDK/native-terminal feasibility result; NO-GO for
production enablement without a successor reviewed plan.** Existing OpenAI
subscription reuse completed successfully. All six requested native
capabilities were exercised in one upstream visible terminal session, with
the limits below. No defaults, production integration, native workers,
follow-on tasks or owner herdr resources were changed.

The durable owner comment at `2026-10-18T02:05:19.15090102Z` explicitly
authorizes existing compatible subscription credential reuse for this probe,
superseding the earlier no-reuse boundary. It does not waive the five-request,
1,024-output-token, $0 paid-API limit or production authority prerequisites.
The complete comment and pre-resumption review notes are retained as evidence;
the October 17 record below remains historical, not the current result.

The branch rebased cleanly onto inspected main
`70a691496ac48558a8d7fb154940354d79546baf`, replacing the old baseline
`911486b4347027f522e155c69812405f10cb3759`. The board now reports T-102 and
T-109 Done; T-102's accepted proposal is still not an implementation contract.
Current PM needs and the shipped ownership/policy boundary remain those
described below. Only T-136's original write scope was used.

### Measured compatibility matrix

Raw current logs are under `tools/pi-probe/output/` and attached to T-136:
`reuse-route-2026-10-18.log`, `native-live-2026-10-18.log`,
`native-fixture-deferred.log`, `offline-preserved-2026-10-18.log` and
`live-budget.json`. Ignored runtime output is evidence, not shipped assets.

| Requirement | Shipped Corral / upstream declaration | October 18 measured result | Remaining boundary |
|---|---|---|---|
| Native terminal control | Corral retains its existing PM launcher; Pi publicly exports SDK session/runtime APIs and `InteractiveMode`. | **PASS:** the thin host initializes upstream `InteractiveMode` in a new private herdr pane. Its exact session is controlled over an authenticated Unix bridge; `native_session_bound` reports `mode=tui`, `hasUI=true`, actual native session ID. Idle input produces real native agent events and subscription responses. | This proves the fixture host, not installed production packaging. No second RPC conversation or screen parsing. |
| Busy input | Pi exposes steer/follow-up separately from prompt completion. | **PASS:** the real session is busy while the provider payload is held before dispatch. Both methods return `queued`; queue updates and later `input_delivered` events distinguish steer from follow-up. Three successful assistant responses follow. | Queue acceptance is not completion. No arbitrary concurrent producer or throughput claim. |
| Cancellation | Pi exposes native abort and final settlement. | **PASS:** a second actual native run is held before HTTP dispatch, then aborted. Its assistant final is `aborted`, `agent_settled.aborted=true`; the earlier committed fixture increment remains 1. | Pre-dispatch cancellation, not proof of cancelling remote work already accepted by OpenAI or rolling back effects. |
| Lifecycle receipts | Corral completion still requires its own gate/review/acceptance. Pi exposes agent and settlement events. | **PASS:** started, queued, delivered, assistant final, low-level end and final settled are distinct. Handled command starts no model work; an injected pre-HTTP provider failure produces `error`, then settled. Lost-ack fixture mutation replays the same receipt once. Reload and session replacement rebind native callbacks. | Injected failure is not a real backend outage. Receipt cache is in-memory, not restart-durable. No task Done inferred from settled. |
| Session ownership | Corral's saved socket/pane authority remains unchanged; the disposable bridge adds launch-bound scope and generation. | **PASS:** wrong capability, foreign project and stale generation are rejected. Replacement changes actual native session ID and generation; old identity is rejected, then an explicitly rebound client succeeds. Clients and panes use only the helper's new plugin-free private server. | Trusted local fixture, not malicious same-user process containment or production writer-lock validation. |
| Restricted PM tools | Corral Rust policy remains authoritative. Pi supports explicit resource/tool selection and nested execution hooks. | **PASS:** only `pm_readback` is active. Shell/read/write/edit/codemode/tool-search nested attempts are denied; an explicitly registered deferred sentinel reaches `tool_call` and is blocked before execution. A successful allowed nested baseline prevents false denial passes. Checks repeat after reload, same pinned-model selection and session replacement; inherited skills/prompts/context files remain zero. | Typed private-fixture tools only; no Corral mutation API, arbitrary model switch, MCP server integration or OS sandbox claim. |

The terminal decision test uses the native extension input component. The
driver types `approve fixture-decision-1 rev1` into its own private pane.
The pending action is checked exactly; before approval the increment fails,
after approval it commits once, a disconnected acknowledgement replays the
same result, and conflicting request-payload reuse fails. This is a
**simulated fixture terminal actor**, not real owner acceptance. Canonical
pending decisions, durable receipts and trusted production exact-action
binding remain mandatory before the first production mutation.

Tool checks explicitly seed a synthetic assistant parent through public
agent/session-manager APIs: the nested executor requires such a parent.
That fixture setup is labelled `providerGenerated=false`, not counted as
provider output. The actual live native responses remain separately measured.

### Credential, budget and reproducibility receipts

- The selected source was the existing Codex ChatGPT credential, mode `0600`.
  Only its unexpired access credential is supplied in memory through public
  `CredentialStore`; refresh/write/logout callbacks fail before invocation.
  No owner login/config was modified, auth directory copied, or token/account
  identifier emitted. Owner bytes, inode and mode were checked unchanged;
  fixture auth files were absent and private fixture cleanup completed.
- **4 total live requests**, all HTTP 200 on the native Codex subscription
  endpoint: route prerequisite output 7 tokens; visible-terminal outputs
  7, 7 and 6 tokens. Total reported output is **27 tokens**. No fifth call,
  retry, refresh, compaction, cache-warming or paid API request was made.
- Every actual compressed wire request was checked for the matching selected
  access/account headers, pinned model and `max_output_tokens=1024`. Pinned
  Codex code omits this field for plain `maxTokens`; the direct public
  provider payload hook supplies it and guarded fetch checks it after
  compression. The backend accepted the cap. Redirects and alternative
  destinations are forbidden; SSE and zero retries are forced.
- Pi remains exactly `@earendil-works/pi-coding-agent@1.3.0`, source commit
  `71023aec1546d6d677a65669c5665c0927ce0dd0`, with the integrity below, Node
  `v24.13.1`, npm `11.8.0`, and `openai-codex/gpt-6.1-sol`. The complete
  current dependency lock is retained as evidence: raw SHA-256
  `d96d0a7303245992b782b25787e13e4bd0576ec3f1d00b3e22ceb39e1648472a`.
  Verification peers are `@types/node@24.10.0` and
  `@modelcontextprotocol/sdk@1.25.2`; these are not new production dependencies.
- All 11 deterministic local tests, TypeScript 6.0.2 checked-JS compilation
  without library-check suppression, and ShellCheck 0.10.0 passed. Fresh
  configured Corral gate and independent review must bind the final committed
  candidate; earlier receipts and verdicts are historical only.

Development failures were retained rather than counted as passes: generic
`ui_prompt_start` does not cover native extension input dialogs; the pinned
dialog installs synchronously and now has an explicit readiness receipt.
Nested execution initially lacked an assistant parent, and an explicit
one-tool selection removed deferred fixtures from the registry. The final
loadout registers deferred sentinels but declares only the readback tool,
requires a successful nested baseline and observes the real denial hook.
All these failed fixture attempts made **zero model requests**.

The original offline probe was rerun unchanged and again reported zero
credentials/provider requests and six `not_run` results. Its deliberate
offline NO-GO is not the live resumption's result.

### Current transport decision

Select the **thin SDK host reusing upstream `InteractiveMode`** for any
successor proposal. It demonstrated the same visible conversation, explicit
resource loadout, lifecycle control and a fail-closed direct provider
credential/request boundary without a new UI or fork.

An extension-only CLI bridge remains less host code but lacks a public
`MainOptions` credential-store injection point. Its provider event handlers
also catch extension exceptions, so they cannot alone enforce this live
budget. It was not separately executed. RPC remains rejected for this goal:
a hidden RPC subprocess would not be the same visible terminal conversation.
Current OmO/omp defaults remain supported and unchanged.

Production is still **not authorized**: the fixture is not a durable adapter,
owner decision service, OS sandbox or installed package. The protocol record
names remaining safety and packaging work and revised conditional estimates.
The successful spike creates no later children and waives no acceptance step.

## Historical October 17, 2026 record

Everything below is the preserved pre-reuse baseline. Statements about absent
authentication, unexecuted native proof and unresolved transport describe
October 17 and are superseded by the measured October 18 section above.

October 17, 2026. **NO-GO for production integration.** The permitted native
OpenAI subscription route is declared by upstream and its model is present
in the installed release, but no owner-authenticated Pi fixture was supplied.
The disposable prerequisite probe stopped without login or provider calls.
It did not prove native-terminal control or select a production transport.
This is a bounded negative spike result, not a claim that Pi or the owner's
subscription is incompatible.

## Authority and inspected baseline

This work executes the separately established T-136 spike, not the
proposal-only T-102 parent. Read all 847 lines of T-102 P1, snapshot SHA-256
`31844cdc68c8251a564fd3c54c090f93032400b871c9623c7b768c46a386b682`.
PR3 independently approved at 20:57:34 UTC; PR4 recorded owner acceptance at
20:58:31 UTC on October 17. Their complete canonical notes and registry
metadata are retained in
[`tools/pi-probe/provenance.json`](../../tools/pi-probe/provenance.json).
PR3's nonblocking requirement remains mandatory: durable request/payload
identity, mutation receipts, canonical pending decisions and trusted
exact-action binding must precede the first production mutation.

Initial HEAD and current local main were both
`911486b4347027f522e155c69812405f10cb3759`; branch `corral/T-136` was clean.
No remote-main equivalence is inferred. Discovery used
`corral search "pi compatibility" --in all --json` and the narrower
`corral search pi --in all --task T-102 --limit 6 --json` before investigation.
`corral list --all --json` confirmed:

| Board item | Observed state | Consequence |
|---|---|---|
| T-031, T-037, T-097, T-098, T-099, T-100 | Done | Reuse current services; historical comparisons are not upstream Pi measurements. |
| T-048 | Dropped | No team-of-PMs or lease implementation is implied. |
| T-102 | Running, P1 owner-approved | Approval of the proposal is not its implementation contract. |
| T-109, T-113 | Done | Product-direction and independent-review reports are available. |
| T-136 | Running, R1 | This is the only executable scope here. |

T-109 recommends work-centered views and one optional conversation, rather
than persistent project PMs. The actual PM needs are bounded readbacks,
intent capture/routing, real owner decisions, event wake/recovery and
reviewable results. Corral already owns admission, gates and acceptance.
Coding tools, workpools, broad harness features and native workers are not
requirements for this PM spike. No quality or cost advantage over OmO was
measured.

## Exact tested pins and access boundary

| Component | Exact candidate/tested value | Evidence and limit |
|---|---|---|
| Package | `@earendil-works/pi-coding-agent@1.3.0` | Fresh npm fixture installation; CLI `--version` returned `1.3.0`, exit 0. The ambient harness-owned `pi` was not used. |
| Release | `v1.3.0`, commit `71023aec1546d6d677a65669c5665c0927ce0dd0` | Upstream tag readback; changelog dates it October 15, 2026. This is a source reference, not an assertion that the tarball was rebuilt reproducibly. |
| npm integrity | `sha512-V/WoCjA2hQSe3QLWIUB24cTRdtEdeLiIzkDqU5RMXV6VWuD+KGZnzVBMhglwdnQTySkehZsjt8a3g6Jd2g4nnw==` | Registry metadata matched installed lock entry; npm performed package integrity verification. |
| Runtime | Node `v24.13.1`, npm `11.8.0`, Linux x64/WSL2 | Node satisfies upstream package engine `>=22.19.0`. No Bun runtime compatibility was tested. |
| Internal Pi packages | `pi-ai`, `pi-mcp`, `pi-tui`, `pi-codemode`, `pi-agent-core`: `1.3.0` | Exact dependencies declared in retained registry metadata. |
| Dependency resolution | Lock digest `7902985c7bdedafad9514426fb71210317daad0c77881707e0f1125285b7873f` | SHA-256 of parsed lock serialized with `JSON.stringify`, not raw lock bytes. Full disposable lock was removed; this is not a reproducible transitive production pin. |
| Provider/model | `openai-codex/gpt-6.1-sol`, API `openai-codex-responses` | Installed offline catalog lookup succeeded. Context 500,000; catalog output maximum 128,000 is **not** the allowed probe output cap. No model request was sent. |
| Authentication | Native upstream OpenAI ChatGPT OAuth; browser/device-code flows declared | Pinned implementation identifies subscription support. Account entitlement and completed sign-in are untested. No other harness credential store was inspected or copied. |
| Probe allowance | At most 5 requests, each at most 1,024 output tokens; $0 API spending | Actual spike provider requests: 0; generated tokens: 0. No API fallback, token refresh, compaction or retry request was invoked. Investigation/reviewer harness usage is outside this probe count. |

The generic upstream provider page alone does not establish ChatGPT support.
The pinned `auth/oauth/openai-codex.ts` declares the native subscription flow;
the installed provider reports `subscription: true`. This is upstream-native
support, not a separate assertion of OpenAI endorsement of this third-party
client or account entitlement.

## Dated compatibility and provenance matrix

**Shipped**, **documented**, **measured** and **unresolved** are separate
columns. A declaration is not a passed probe.

| Requirement | Shipped Corral contract at inspected main | Upstream documented capability | Measured on October 17 | Unresolved / blocking evidence |
|---|---|---|---|---|
| Native visible-terminal control | `pm.rs` launches configured profile; `PmState` stores only pane ID. Legacy delivery inspects the composer. | Extension message injection and existing terminal UI; SDK exports `InteractiveMode`. | CLI startup/version and SDK import only. | Same visible conversation must accept ownership-bound commands. No extension bridge was executed. |
| Idle and busy input | Existing delivery evidence is not native Pi acceptance. | Explicit steer/follow-up; started/queued/handled dispositions; starting runs count as busy. | Not run. | Correlate each disposition and actual execution without screen inference. |
| Cancellation | Existing run/ownership policies remain authoritative. | SDK abort and extension abort context; RPC abort command. | Not run. | Idle/busy/race receipts, late-event fencing and committed-effect preservation. |
| Lifecycle receipts | Corral completion requires candidate evidence/gate/review, not idle text. | `agent_end` is low-level; `agent_settled` is final notification; provider failures may follow acceptance. | Not run. | Handled-without-run, failure, reconnect, reload, session replacement and disconnected acknowledgement. |
| Session ownership | Saved socket + workspace/pane + launch reservation; recheck under writer lock (`docs/session-ownership.md`). | Session lifecycle contexts; replacement invalidates old session listeners. | Fresh private helper server started and removed. | This proves fixture isolation, **not** native callback/store/generation authorization. |
| Restricted PM tools | PM prompt forbids project edits; shared Rust ops own mutations. Prompt policy is not an OS sandbox. | Explicit resources/tools; nested `executeTool`, deferred/codemode exposure and MCP discovery exist. | No session/tool loadout executed. | Deny shell/edit/proxy, nested/deferred and resource escapes after reload/model switch at enforcement boundary. |
| Trusted owner decision | Existing plan/owner acceptance cannot be supplied by a model actor field. | Terminal UI dialogs/commands available. | Not run. | Exact-action trusted callback, canonical pending state and stale-answer rejection. |
| Durable recovery | `WaitRequest`/`DecisionRequest` require real run IDs; PM has no run container. Legacy push may skip absent/disabled intervals. | Session entries/context persistence and SDK replacement events. | Source contract review only. | New canonical PM-scoped records, event cursor and mutation deduplication design require successor review. |
| Subscription route and cost | OmO route is not transferable proof. Defaults are `omo`, push off (`config.rs:357`). | Pinned upstream native ChatGPT subscription OAuth. | Catalog/provider metadata present; credential count 0. | Native owner sign-in absent. No entitlement, live reliability or comparative usage evidence. |

## Probe results and stop condition

[`tools/pi-probe/preflight-final.log`](../../tools/pi-probe/preflight-final.log)
contains the successful prerequisite command output from the final typed driver.
`preflight.log` retains the preceding successful attempt. `run.sh` clears inherited
environment variables before starting `scripts/test-herdr.sh`; the helper
creates private HOME/XDG directories, plugin-free herdr and one baseline pane.
The package is installed only under that disposable root with install scripts
disabled. Model networking is disabled and no session is created.

The first attempt exposed a real public-export mismatch:
`ReadOnlyAuthStorage` is an internal class, not a root SDK export. Its exception
excerpt is preserved in `tools/pi-probe/failed-preflight.log`. The corrected
driver uses public `ModelRuntime.create()` and `listCredentials()`, without
resolving credentials. Both attempts used the same top-level pins and made
zero model calls. A final rerun after local static-check fixes produced the
same result; all three temporary roots were confirmed absent after cleanup.
The npm dependency emitted a `node-domexception` deprecation warning; it was
not suppressed or treated as an integration failure.

Final preflight exited 0 and reported:

- Installed package integrity and CLI version matched.
- Offline selected model and subscription provider were present.
- Owner fixture credential count was 0; its newly created empty auth file
  had mode `0600`; fixture root had mode `0700`.
- JS fetch attempts were 0; no login, credential resolution or model operation
  was called. This counter is not an OS-level network capture; npm registry
  downloads happened before the offline phase.
- Native control, busy input, cancellation, lifecycle, ownership enforcement
  and PM-tool restrictions were all explicitly `not_run`.

There was no already provisioned owner-controlled fixture in this run's
inputs. A fresh empty fixture is not evidence that native sign-in is broken.
Completing login would require the owner's interactive authentication, not
copying existing subscription credentials. The task's stop-on-unavailable-
authentication boundary was applied: no live driver, terminal session bridge,
fixture mutation or owner-decision experiment was started. No persistent
credential fixture was left awaiting input.

Thus acceptance scenario 2's live capability proof is **missing**, not passed.
Scenario 4's explicit negative-outcome path governs this result. The six native
capabilities remain unknown, not rejected as upstream impossibilities.

## Transport ADR: no selection for production

| Alternative | Fit | Decision |
|---|---|---|
| Terminal + first-party extension + private bridge | Best candidate for controlling the exact upstream visible conversation. Requires authenticated session-scoped endpoint and structured lifecycle proof. | Preferred candidate only; **not selected** because control, permissions and receipts were not measured. |
| Thin SDK host using upstream `InteractiveMode` | Public SDK exposes session APIs and terminal components. Could host one conversation without a new UI. | Unverified fallback. Owns more resource/session lifecycle; root-export failure demonstrates why exact declarations matter. No host was built. |
| RPC subprocess | JSONL responses correlate command IDs; session events generally do not. Good Rust boundary. | Not selected. Documented RPC mode does not by itself expose the same interactive terminal session. A hidden second conversation/new chat UI fails this goal. |
| Existing OmO/omp | Current supported default/worker paths. | Retained unchanged. |
| Screen scraping, fork, native workers, generic shell/MCP wrapper | Does not meet the approved native/authority boundary. | Excluded; not substituted to manufacture a pass. |

Authentication is not established and the transport decision is unresolved:
either is sufficient for **no-go**. No production integration, default switch,
new implementation children or additional provider route is authorized.
Candidate package location, protocol and conditional later estimates are in
the [protocol record](pi-native-protocol.md); a successor independently
reviewed and accepted plan is required before production work.

## Primary upstream references

Retrieved October 17, 2026. Moving pages describe intent; pinned source and
installed metadata identify this spike's candidate.

- Registry: `https://registry.npmjs.org/@earendil-works/pi-coding-agent/1.3.0`
- Tag: `https://api.github.com/repos/earendil-works/pi/git/ref/tags/v1.3.0`
- Pinned source root: `https://github.com/earendil-works/pi/tree/71023aec1546d6d677a65669c5665c0927ce0dd0`
- Native auth: `packages/ai/src/auth/oauth/openai-codex.ts` at that commit.
- Public exports: `packages/coding-agent/src/index.ts` at that commit.
- Runtime: `packages/coding-agent/src/core/model-runtime.ts` at that commit.
- `https://pi.dev/docs/latest/providers`
- `https://pi.dev/docs/latest/extensions`
- `https://pi.dev/docs/latest/sdk`
- `https://pi.dev/docs/latest/rpc`
- `https://pi.dev/docs/latest/packages`

The initial guessed legacy OAuth path and provider-index path returned 404;
the pinned directory listing located the actual auth implementation. The
guessed website changelog route did not exist; the release-tag changelog was
used instead. None of those failed lookups was treated as absence of support.
