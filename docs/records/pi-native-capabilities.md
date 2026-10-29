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

An enabled key's effective settings are recorded in its feature state (`settings`) and covered
by the digest. `trusted_projects` (default `["*"]`) is the one project-text trust decision;
`resolve_for(role, project, supported)` records it as the snapshot's `project_text`, which
doctor's project-free `resolve` leaves absent. A transition change sets `settings: true` when
a key's effective settings differ, whatever its enabled state. A legacy generation takes each
key's strict value (compaction and lessons on) except its recorded todo flag. The plan
roles' guidance settings are resolved today; their native hosts arrive with plan-roles.

`CAPABILITY_CATALOG_VERSION` (1) changes only when an entry's meaning or the snapshot's
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
canonical JSON of those fields and the feature map. Credentials are never part of it.

## Records

`AgentGeneration.capabilities` stores the snapshot written when the generation opens; it is
never rewritten. `AgentGeneration.capability_transition` is present when the snapshot's
digest differs from the previous generation's: `{from_generation, from_digest, from_preset,
changes: [{key, from, to}]}`. `AgentGeneration.todo` is still written for older readers;
`AgentGeneration::capability(key)` reads the snapshot, or the recorded todo flag for a
legacy generation.

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
project_text}` beside the legacy `todo` boolean; `enabled` lists host keys only and
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

## Assistant seam (T-544)

T-544 consumes the same resolver; it does not get a parallel one. To add the assistant
role it adds an `AgentRole` variant, which makes every catalog entry's exhaustive
`defaults` match a compile error until it states the assistant's ceiling and presets, and a
`[agent.capabilities.assistant]` table. Its own operation table in `call.rs` remains the
authority for Corral operations; capabilities only select optional reasoning features and
never grant Corral authority. Start from the worker's standard column and narrow it to the
assistant's scratch and co-work scope.
