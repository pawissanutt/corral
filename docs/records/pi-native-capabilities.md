# Pi-native capabilities (T-563)

The capability framework is the shared seam for every optional Pi-native feature in plan
T-460@P3. Rust resolves one catalog into one per-role snapshot per generation; the host,
`corral agent doctor` and `corral agent state` all read that snapshot. No other component
keeps its own defaults.

## Catalog

`crates/corral-core/src/agent/capabilities.rs` owns `CATALOG`. Each entry has a key, a
one-line summary, `defaults(role) -> Option<[strict, standard, full]>`, `host` and an
optional `settings` function. `None` is the role's ceiling: no preset or override can enable
the key for that role, and configuration that tries is a parse error. `host` says whether the
installed Pi host must implement the key; `lessons` is Rust-side (the dispatch prompt), so an
older host never makes it unavailable and the launch message never names it. `settings`
validates a compound key's configured table and returns its effective settings.

| Key | Worker | PM | Reviewer, planner, plan reviewer | Settings |
|---|---|---|---|---|
| `todo` (T-562) | off / on / on | off / on / on | not available | none |
| `web.enabled` (T-568) | off / on / on | not available | off / on / on | own `web` table and `snapshot.web` |
| `context_files` (T-564) | off / on / on | off / on / on | off / on / on | none |
| `skills` (T-564) | off / on / on | off / on / on | off / on / on | `user_dirs`, default `["~/.agents/skills"]` |
| `prompt_templates` (T-564) | off / on / on | off / on / on | off / on / on | `user_dirs`, default `["~/.agents/prompts"]` |
| `compaction` (T-564) | on / on / on | on / on / on | on / on / on | `reserve_tokens`, `keep_recent_tokens`, `branch_summary`; only configured values |
| `lessons` (T-564, Rust-side) | on / on / on | not available | on / on / on | none |
| `session_export` (T-564) | off / on / on | not available | off / on / on | none |
| `mcp.enabled` (T-567) | off / on / on | off / on / on | off / on / on | own `mcp` table and `snapshot.mcp` |
| `tool_search` (T-567) | off / on / on | off / on / on | off / on / on | none |

An enabled key's effective settings are recorded in its feature state (`settings`) and covered
by the digest. `trusted_projects` (default `["*"]`) is the one project-text trust decision;
`resolve_for(role, project, supported)` records it as the snapshot's `project_text`, which
doctor's project-free `resolve` leaves absent. A transition change sets `settings: true` when
a key's effective settings differ, whatever its enabled state. A legacy generation takes each
key's strict value (compaction and lessons on) except its recorded todo flag. The plan
roles' guidance settings are resolved today; their native hosts arrive with plan-roles.

`CAPABILITY_CATALOG_VERSION` (3) changes only when an entry's meaning or the snapshot's
canonical form changes. A later member registers its own implemented key and defaults here
and adds it to the host's `HOST_CAPABILITIES` and `compat.json` in the same change.

## Configuration and resolution

`[agent.capabilities]` takes `preset` (`strict`, `standard`, `full`; absent means
`standard`) and per-role tables `worker`, `reviewer`, `pm`, `planner` and `plan_reviewer`
whose keys are catalog keys with boolean values. Parsing rejects an unknown key and a key
outside the role's ceiling. `AgentCapabilities::resolve(role, supported)` returns the
`CapabilitySnapshot`; `supported` is the installed host's `compat.json` `capabilities` list.

For each catalog key the role may use, the snapshot records:

- `enabled`: requested and implemented by the installed host;
- `source`: `preset`, `role_override` or `legacy`;
- `unavailable`: the reason, when the key was requested but the host does not implement it.

The snapshot also records `catalog`, `role` and `preset`, and `digest`, the SHA-256 of the
canonical JSON of those fields, the feature map and enabled compound settings (`web`
and `mcp`). Credentials are never part of it.

## Records

`AgentGeneration.capabilities` stores the snapshot written when the generation opens; it is
never rewritten. `AgentGeneration.capability_transition` is present when the snapshot's
digest differs from the previous generation's: `{from_generation, from_digest, from_preset,
changes: [{key, from, to}]}`. `AgentGeneration.todo` is still written for older readers;
`AgentGeneration::capability(key)` reads the snapshot, or the recorded todo flag for a
legacy generation.

Compound settings have concrete `web_change` and `mcp_change` records containing the
old and new settings. MCP settings retain exact project bindings, transports, approved
tool identities/effect ceilings and resource URI scopes. Changing any of them changes
the next snapshot digest; schema refreshes for an approved tool do not.

A generation recorded before the catalog has no snapshot. `effective_capabilities` labels
it `legacy_strict` with its recorded todo flag and every other optional key off. Recovery
uses that label as is; it never reconstructs historical settings or refuses because of them.

## Explicit resume

An active generation keeps its snapshot; a reload rebuilds the loadout from it. An explicit
resume, dispatch or review launch opens a new generation, which resolves the current
configuration. The new generation records the transition from its predecessor, and the old
snapshot stays available to classify that generation's interrupted calls. A harmless
configuration change therefore adapts on resume instead of forcing a fresh run.

## Older installed hosts

`compat.json` lists the keys a host implements in `capabilities`; a manifest without the
list implements none. Corral resolves against it at launch:

- an unavailable preset default is reported in the snapshot, in `corral agent state` and in
  doctor's `pi_native_capabilities` line, and is left off; the launch continues;
- an explicit `true` override for a key the host lacks refuses the launch with
  `capability_unavailable`, naming the setting and `corral agent install`, before any
  generation opens. Doctor fails the same line with the same fix.

Protocol, credential, identity and role errors keep their existing handling.

## Launch message and host

The launch message carries `capabilities: {catalog, digest, preset, enabled, settings,
project_text, web?, mcp?}` beside the legacy `todo` boolean; `enabled` lists host keys only and
`settings` maps each enabled compound key to its effective settings. When `capabilities` is present the host takes todo from `enabled`;
an older corral that sends only `todo` still works. The host refuses a launch message that
enables a key outside its own `HOST_CAPABILITIES`, since corral only does so when the
installation is mismatched. `RoleContext.capabilities` exposes the snapshot to role modules,
and the bridge `get_state` result reports it.

## Execution guard

The host's one `tool_call` check runs for model-issued calls and for calls a tool makes
through `ctx.executeTool` (`parentToolCallId` set). It blocks a tool outside the role's
loadout, and it blocks every `corral_*` tool when nested: a Corral operation's request ID
is bound to a top-level transcript tool call, so nested execution would lose that identity.
Upstream's fixed tool allowlist already hides non-loadout tools; the check is the second,
role-aware layer. Bound-session replacement refusal and provider protections are unchanged.
`packages/corral-pi/test/nested-guard.test.ts` checks the allowed nested read, the refused
nested `corral_todo`, the unreachable foreign tool and the single top-level receipt, at
startup, after a reload and after every refused replacement.

Bash, background jobs, stdio MCP, LSP, hooks and browser subprocesses run outside the
in-process network guard (plan decision E1); this framework adds no containment claim.

## MCP and cached discovery (T-567)

`capabilities/mcp.ts` uses the pinned Pi MCP client directly, not its ambient built-in
extension. Only the generation's workspace configuration is loaded. The adapter filters
servers by the bound project and applies one role/tool predicate to discovery,
registration, activation and calls. Annotations are not consulted. Resources share the
same server binding plus exact URI or trailing-wildcard scopes. General shell/write
proxies are denied to read roles even if labeled read; Corral proxy identities are denied
for all roles. Configured servers remain trusted code, not certified pure services.

The host's execution guard also applies to nested calls. Cached `tool_search` filters
only currently permitted metadata and has no activation or connection side effect.
`mcp_activate` refreshes a configured server before activating approved names; it never
enables codemode. With search disabled, approved MCP tools are ordinary direct tools.
`mcp_connect` exposes an empty/unavailable configuration as useful state and refreshes
approved schemas without digest approval. Unapproved new identities stay denied.

HTTP MCP has its own configured-origin transport and network-ledger entries. It cannot
borrow provider authorization or the public-web exception, and redirects are refused.
Stdio transport is outside that guard. Both use the existing pinned client's bounded
requests and process-group cleanup. Reload reconstructs the adapter from the same
snapshot; explicit resume may resolve new settings. Only exact cached `tool_search`
is no-effect during interruption recovery; all MCP operations stay conservative.

### Static cost delta

Estimator `utf8-json-bytes/4-v1` serializes `{name, description, parameters}` for the
new active tool declarations and rounds UTF-8 bytes / 4 upward. It is a reproducible
heuristic, not a provider tokenizer or inference measurement. With standard/full and
no project servers, the three MCP controls, activation and cached search add 1,397
bytes (350 estimated tool tokens) for each of worker, reviewer and PM. Optional role
prompt text changes by 0 bytes. Strict, with no explicit override, adds 0.

The planning-role resolver enables the same keys and the same role-independent tool
declarations would have that static delta, but native PL/PR host execution is not
installed in this task's base and is not claimed as a runnable measurement. Configured
server metadata is additional, server-dependent context; deferred tools contribute
their schemas when activated. `test/mcp-host.test.ts` prints the raw per-role
`MCP_STATIC_COST` records. No dynamic token saving, billing result or harness comparison
is inferred from these estimates.

## Early usage checkpoint (T-570)

This is a **partial early checkpoint**, not the completed standard comparison. The frozen
cohort deliberately includes guidance/session, todo, background jobs and codemode only.
Even when other capabilities exist on the checkout, web, MCP, configurable LSP, explore,
oracle and native plan-role behavior are excluded from completion claims. Existing worker
LSP declarations remain in both arms. PM costs are separate from worker/review costs.

`packages/corral-pi/test/usage-baseline.test.ts` builds actual host sessions without inference,
using a private fixture home, one fixed skill, fixed project guidance and empty MCP resources.
It emits `EARLY_STATIC` JSON records with complete prompts/tool schemas, selected capability
keys, SHA-256 hashes, byte counts and estimator `utf8-json-bytes/4-v1`. Disposable fixture and
asset prefixes are normalized before estimating. These are schema/prompt estimates, not
provider token counts; a skill body is loaded on demand, not all charged at startup.

| Role | Strict prompt / tools estimate | Early standard prompt / tools estimate |
|---|---:|---:|
| Worker | 1,995 / 3,125 | 2,140 / 4,746 |
| Reviewer | 1,484 / 1,757 | 1,725 / 2,290 |
| PM | 1,064 / 1,272 | 1,119 / 1,707 |
| Planner | not runnable | not runnable |
| Plan reviewer | not runnable | not runnable |

This historical capture used source commit `90ab4219` on base `d0ea9d24`, before native
PL/PR modules landed. Absent planning costs are null, not zero-cost successes. The candidate
was subsequently rebased onto newer main, preserving its planning/question-hold changes;
that does not retroactively expand the measured cohort or authorize new inference. Actual owner home skills and configured resources would change these
numbers. The role matrix resolves settings in Rust; this fixed early subset is a test control,
not a second production default resolver.

Collector fixtures exercise copied/resumed transcripts, assistant and summary requests,
auxiliary references, all five durable container roles, repeated collection, subscription
API-equivalent pricing, Bedrock pricing and unknown-model gaps. Implementation refresh is
wired through `agent/hooks/usage.rs`; PM and planning readback uses
`agent::usage::collect_container`, without changing implementation totals. Auxiliary references
are reusable accounting infrastructure; child launch and budget enforcement remain later work.

### Authorized subscription smoke

Owner Q1 revision 2 authorized at most four subscription requests (W/R × strict/standard),
1,024 output tokens each, no paid API and no automatic retries. The smoke used
`openai-codex/gpt-6.1-sol`, SSE, reasoning effort `none`, the above frozen prompts/schemas,
and one identical sorted-unique/sum JSON question. No tool was executed. Wire payloads retained
`max_output_tokens=1024` explicitly because the pinned Codex adapter omits `maxTokens` when
building its body. This checkpoint does not exercise the host workflow or compare task quality.

| Arm | Input / output | Cache read / write | API-equivalent USD | Wall ms | Outcome |
|---|---:|---:|---:|---:|---|
| W strict | 4,173 / 36 | 0 / 0 | 0.008706 | 2,755 | valid smoke JSON |
| W early standard | 5,539 / 17 | 0 / 0 | 0.011248 | 6,303 | valid smoke JSON |
| R strict | unknown | unknown | unknown | 1,666 | backend overloaded |
| R early standard | not run | not run | not measured | — | stopped after backend error |

Three requests were attempted, no retries; the fourth allowance was unused. The raw error's
SDK-zero usage is **unknown**, not free work. Known-response API equivalents use the exact
`gpt-6.1-sol` starter price in `crates/corral-core/src/pricing.rs`: input $2, output $10,
cache-read $0.10, cache-write $2.50 per million. Pi-reported costs remain separately retained.
PM was static-only; planning roles were not runnable. Raw controls, full request payloads,
response IDs, usage, outcomes and times are attached to T-570 as the subscription-smoke evidence.
The runner is `packages/corral-pi/src/host/usage-smoke.ts`; execution needs a fresh output directory
and its explicit run-authorization flag. Tests exercise its controls without inference.

The worker pair passes only this small arithmetic smoke; the reviewer pair is incomplete.
There is no cost-saving, completed-standard, workflow-quality or default-harness-switch claim.
T-313 remains a qualified historical non-pass, not a causal savings baseline.

## Completed standard profile capture (T-579)

### Question, boundary and controls

What context overhead does the completed, shipped standard loadout add relative to strict,
and what does one matched request cost? This is a **final capability-profile capture plus
bounded capability-cost smoke**, not a completed-task workflow experiment or proof of quality.
It supersedes the early subset only for feature coverage; the historical early results above
remain unchanged. T-579/R1/Q1 revision 2 authorized 16 subscription-only requests, 1,024 output
tokens each, no paid API, no retries and stop on provider error. All 16 completed; no allowance
remains. No tool or auxiliary request was executed by live inference.

Frozen source: `b4908eb7e8858ba226633ab105ab38dcce494fcd`, catalog 6, Pi 1.3.0,
Bun 1.4.2, `openai-codex/gpt-6.1-sol`, SSE, effort `none`. Both arms use the exact same
sorted-unique/sum JSON question from T-570. Arm order was saved before sending: inert then
configured; within each, W/R/P strict then standard, followed by PL/PR standard. This fixed
order is not randomized, cache state is uncontrolled, and one observation per arm supports
neither statistical nor causal performance claims. PL/PR strict is disabled by the resolver:
not runnable, not a zero-cost success. PM and planning results are separate from W/R.

`tools/pi-compare/resolver/` links the production Rust catalog. The capture resolves both
presets for every role against the checkout's actual `compat.json`; it does not recreate
preset defaults in TypeScript. The host sessions use actual role modules with private fixture
auth/delivery. No owner home, credential text, live herdr or board state enters static captures.
The source hashes, supported keys, config requests, effective snapshots/digests, complete
prompts/schemas and prompt/schema hashes are retained. Temporary project/home/arm/assets roots
and the ephemeral MCP origin are normalized before estimating. A static audit verifies that
inert/configured strict prompts and tools have identical hashes.

Resources are controlled, not representative of every owner's installation:

- **Inert:** empty private home skill/template directories, fixed trusted AGENTS text, empty MCP,
  default public `web_fetch`, no SearXNG, no custom LSP languages.
- **Configured:** same AGENTS text, one fixed home skill and template, one read-only private HTTP
  MCP peer (`docs.lookup`), configured SearXNG origin, and lazy LSP language mapping. Startup and
  post-activation MCP schemas are retained separately. The peer is real JSON-RPC but supplies
  synthetic fixture content. No search/fetch or LSP process is invoked. The configured LSP
  command is deliberately `false`: it measures settings/schema overhead, not server fitness.
- **Live configured arm:** the post-activation schemas, with no MCP transcript/tool execution.
  Static activation used fake provider responses, not part of the 16-request allowance.
- Optional skill bodies, template expansions, arbitrary project lessons and real dispatch task
  prompts can add later input. These startup profiles do not estimate those unbounded texts,
  owner's actual skill inventory or the entire dispatched run's prompt.

### Static results

Estimator `utf8-json-bytes/4-v1`: prompt UTF-8 bytes and serialized
`{name,description,parameters}` tool bytes, each divided by four and rounded up. This is a
reproducible heuristic, **not provider tokenization**. Tools in this table are active schemas;
MCP metadata/deferred schemas are additional when activated. All values below are
**prompt / tools** estimated tokens.

| Role | Strict, either resource case | Standard inert | Standard configured startup | Standard configured activated |
|---|---:|---:|---:|---:|
| W | 1,994 / 3,125 | 2,094 / 5,867 | 2,136 / 5,867 | 2,136 / 5,929 |
| R | 1,484 / 1,757 | 1,583 / 2,961 | 1,722 / 2,961 | 1,722 / 3,023 |
| P | 1,062 / 1,272 | 1,115 / 2,056 | 1,115 / 2,056 | 1,115 / 2,099 |
| PL | not runnable | 776 / 3,091 | 915 / 3,091 | 915 / 3,152 |
| PR | not runnable | 767 / 3,174 | 906 / 3,174 | 906 / 3,236 |

Standard inert adds 100/2,742 estimated prompt/tool tokens for W, 99/1,204 for R and
53/784 for P. Configured home text adds 42 prompt tokens for W, 139 for R/PL/PR and
0 for P in this fixture; this is startup metadata, not a claim that reading skills is free.
Activating the one approved MCP schema adds 61–62 estimated tool tokens (43 for PM's
rendered schema). Do not sum rounded estimates as if they were exact tokenizer counts.

The capture includes every shipped standard-affecting member, not just the early subset:

| Member / surface | Final capture coverage and cost boundary |
|---|---|
| Todo / framework (T-562/563) | Current effective snapshots; W/P todo schema; role ceilings and strict planning disable retained |
| Guidance/session (T-564), skill pack (T-583) | Real host contracts/packaged assets, fixed trust/text, automatic home directory settings, skill/template metadata; expansion/read/export/compaction invocation costs are not exercised |
| Jobs (T-565) | W job and monitor schemas; no shell work or monitor wakes in live smoke |
| Codemode (T-566) | Actual W/R/PL/PR declaration and prompt additions; PM standard remains off; no nested execution in live smoke |
| MCP/search (T-567) | Empty/configured snapshots, cached controls and activated read schema; no external server fees measured |
| Web (T-568) | Public-default fetch and configured/unconfigured SearXNG settings/schema; no search service or downloaded text costs |
| LSP (T-569) | Worker built-ins in both presets, PL/PR standard read tools, lazy custom-language settings; R/P standard LSP off |
| Explore/oracle (T-571) | W/PL/PR explore schema plus separate actual auxiliary-adapter static captures below; oracle standard off |
| Native plan roles (T-573) | Actual PL/PR read-only host/schema, standard runnable; typed completion operations not executed |
| Browser (T-574) | Actual W standard navigation/screenshot schemas are included. This differs from P3's full-only browser column; it records shipped `browser_defaults`, not a silent exclusion or a new policy change |
| Hooks/bash rules (T-572), rewind (T-575), local model (T-578) | Present catalog entries but off in standard; no invocation cost, no full/override behavior claim |
| Budget/headless (T-576/577) | No configured limits or headless opt-in in these controls; no budget enforcement or headless completion claim. Neither is a new standard tool-schema cost in this source |

The installed agent running T-579 is not assumed to have identical assets to the frozen
checkout; the measurement constructs sessions from that checkout. Optional feature/runtime
failures outside these captures (including codemode execution) are not proven away by a
schema measurement.

### Dynamic results and raw outcome validity

Raw controls, admissions, complete wire payloads (`max_output_tokens=1024`), full replies,
response identities, reported usage and wall times are retained in the attached T-579 evidence
archive. `capture/audit.ts` independently rechecks all 16 JSON answers, caps, distinct response
identities, profile hashes and predeclared order. **16/16 arithmetic answers are valid.** This
means only correct smoke output, not successful worker completion, review, PM action, planning
submission, gate or landing. Strict PL/PR is absent by design, not counted as failed or passed.

Each usage column is uncached input / output / cache read; **cache write is 0 in every row**.
Output includes provider-reported reasoning tokens, even with effort `none`. USD is
API-equivalent, not subscription billing: frozen starter prices are $2/$10/$0.10/$2.50 per
million input/output/cache-read/cache-write tokens (`pricing.rs`, `gpt-6.1-sol`). Pi's reported
cost object is separately preserved. No pricing gap occurred in these known-model responses;
future unknown usage/prices remain null, never $0.

| Resource | Role / preset | Input / output / cache read | API-equivalent USD | Wall ms |
|---|---|---:|---:|---:|
| Inert | W strict | 2,128 / 40 / 2,048 | 0.0048608 | 5,297 |
| Inert | W standard | 6,360 / 30 / 0 | 0.0130200 | 2,441 |
| Inert | R strict | 2,653 / 31 / 0 | 0.0056160 | 2,547 |
| Inert | R standard | 3,733 / 78 / 0 | 0.0082460 | 5,248 |
| Inert | P strict | 1,700 / 17 / 0 | 0.0035700 | 1,923 |
| Inert | P standard | 2,131 / 17 / 0 | 0.0044320 | 2,450 |
| Inert | PL standard | 3,136 / 30 / 0 | 0.0065720 | 2,295 |
| Inert | PR standard | 3,173 / 42 / 0 | 0.0067660 | 3,302 |
| Configured | W strict | 4,176 / 30 / 0 | 0.0086520 | 2,052 |
| Configured | W standard | 6,464 / 39 / 0 | 0.0133180 | 5,331 |
| Configured | R strict | 2,653 / 88 / 0 | 0.0061860 | 5,289 |
| Configured | R standard | 3,916 / 32 / 0 | 0.0081520 | 3,060 |
| Configured | P strict | 1,700 / 17 / 0 | 0.0035700 | 1,582 |
| Configured | P standard | 2,162 / 17 / 0 | 0.0044940 | 2,396 |
| Configured | PL standard | 3,319 / 28 / 0 | 0.0069180 | 1,907 |
| Configured | PR standard | 3,356 / 40 / 0 | 0.0071120 | 2,459 |

Total: 52,760 uncached input, 576 output, 2,048 cache read, 0 cache write; **55,384 tokens,
USD 0.1114848 API-equivalent**, 49,579 ms summed request wall time. Paid API spend was $0.
The first W strict reply had 2,048 cached input tokens, so its apparent lower uncached input
is not a prompt reduction: input + cache read equals configured W strict's 4,176. Wall time
excludes static capture/setup; caching and shared provider conditions prevent causal speed
or savings conclusions. Separate roles/cohorts must not be extrapolated to task-level spend.

An initial local analysis bug examined the SDK's nested `usage.cost` object as though every
usage value were numeric, leaving all raw `api_equivalent_usd` fields null. Those captures
are immutable. The corrected helper validates only the four priced traffic counters;
`report-data.json` recomputes the table offline from preserved usage and retains the original
null field alongside it. No additional inference was used. A regression covers the actual
SDK-shaped nested cost, nonfinite usage and uncertain error-path zeros.

### Auxiliary and non-token costs

`capture/auxiliary.ts` drives the **production** `runAuxiliarySession` adapter with fake
credentials/transport and the captured parent schema intersection. All six W/PL/PR ×
inert/configured explore contexts have 49 estimated prompt tokens and 1,384 tool tokens
(read/grep/find/ls, allowed read LSP/web). There is no recursive spawn, MCP, shell, hook or
Corral tool. This includes no owner guidance and no parent transcript: actual child questions,
file results and responses add variable traffic on every invocation. The tool-free oracle
reference is 35 prompt / 1 serialized-empty-list heuristic tokens; oracle remains off in
standard. This one-token wrapper estimate is not a real empty-tool provider charge.

**Live auxiliary/summary request count is 0**, hence no auxiliary/summary usage in the main
smoke totals. Their *future* dynamic cost is unmeasured/null, not zero. Each explore invocation
uses its own model context and can make multiple tool-round requests; oracle and compaction
also require separately attributed usage. The seven offline adapter responses carry synthetic
usage, excluded from all dollar/token totals. Durable accounting/collector coverage was
verified by T-570; this task does not rerun or replace its recovery tests.

Other auxiliary costs are not billed LLM tokens: local MCP initialization/refresh and server
runtime, LSP startup/diagnostics, shell/job CPU and output, browser page work/screenshots,
exports, hook execution, and remote search/MCP service charges. Only private MCP fixture
connection/activation occurred here (raw RPC list retained); process/service costs are not
priced. No host load generation or live browser/herdr check was performed. Future tool output,
images, search results, activated schemas, hooks and summaries can increase the next model
request. A startup profile is not a bound on that cumulative spending.

### Historical comparison and recommendation

T-313 observed 65,317,494 native worker/review tokens (63,507,200 cache read) and
USD 11.4245400 API-equivalent across five paired approval windows. It remained a **qualified
historical non-pass**: fewer than six complete pairs, missing counterpart landings,
`complete:false`, `valid:false`, `outcome:not_evaluated`; PM stayed OmO and was not evaluated.
These 16 tiny strict/standard requests are neither matched to those tasks nor an OmO arm.
There is no valid ratio between their totals, no T-313 pass promotion, and no causal savings,
assistant-completion or default-harness-switch claim. T-544 retains assistant scope.

Recommendation: keep this as the reproducible completed-standard context-cost reference,
use the separate T-313 workflow protocol for any later outcome comparison, and request a new
finite allowance before further inference. Standard exposes substantially more context than
strict; configured resources and auxiliary use must be budgeted separately. Do not infer
cost neutrality or task-quality parity from 16 correct arithmetic responses.

### Reproduction and evidence

Offline adapter instructions are in `tools/pi-compare/README.md`. This run used the shared
target directory for `cargo build --manifest-path tools/pi-compare/resolver/Cargo.toml`, then
`capture/static.ts` (final capture `final-static-v3`), `capture/smoke.ts` once under Q1,
`capture/auxiliary.ts` (final capture `final-auxiliary-v2`) and `capture/audit.ts`. Earlier
static captures were discarded from final analysis after normalizing arm-specific temporary
path spellings; they consumed no inference. The raw smoke was never repeated.

The attached evidence archive contains final static profiles/config snapshots/source hashes,
static and auxiliary logs, full smoke controls/admissions/payloads/replies, the authorization
readback and `report-data.json` with input digests. Reports and raw records remain distinct.
Validation: 46 offline comparison/metric/profile tests pass; standalone and capture-adapter
TypeScript checks pass; the Rust resolver builds and formats. The run's receipted quick gate
is recorded separately; no full gate was started by this task. No workflow-quality or real
optional-service execution acceptance is inferred from it.

## Assistant seam (T-544)

T-544 consumes the same resolver; it does not get a parallel one. To add the assistant
role it adds an `AgentRole` variant, which makes every catalog entry's exhaustive
`defaults` match a compile error until it states the assistant's ceiling and presets, and a
`[agent.capabilities.assistant]` table. Its own operation table in `call.rs` remains the
authority for Corral operations; capabilities only select optional reasoning features and
never grant Corral authority. Start from the worker's standard column and narrow it to the
assistant's scratch and co-work scope.
