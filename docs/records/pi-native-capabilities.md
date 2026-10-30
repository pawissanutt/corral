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

## Assistant seam (T-544)

T-544 consumes the same resolver; it does not get a parallel one. To add the assistant
role it adds an `AgentRole` variant, which makes every catalog entry's exhaustive
`defaults` match a compile error until it states the assistant's ceiling and presets, and a
`[agent.capabilities.assistant]` table. Its own operation table in `call.rs` remains the
authority for Corral operations; capabilities only select optional reasoning features and
never grant Corral authority. Start from the worker's standard column and narrow it to the
assistant's scratch and co-work scope.
