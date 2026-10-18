# T-136: Pi-native PM compatibility decision

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
