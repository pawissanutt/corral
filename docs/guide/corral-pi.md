# corral-pi agents (opt-in, in progress)

corral is gaining an opt-in agent host that runs the Pi coding agent under Bun in a run pane
and talks to corral through authenticated, idempotent calls instead of screen or idle
detection. OmO stays the default worker, reviewer and PM. Nothing changes unless you configure
a profile whose harness is `corral-pi`.

The host lands in stages. This page states what works today; the stages that are not yet
installed refuse explicitly rather than half-working.

## Compatibility during rollout

The former harness identifier remains a read-only configuration/history alias; new
records use `corral-pi`. Profile names such as `pi-sol` stay unchanged. New installs use
`$XDG_DATA_HOME/corral/corral-pi` (or `~/.local/share/corral/corral-pi`) and the bundled
skills directory `~/.corral/skills/corral-pi`. When the new directory is absent, existing
legacy installations are read in place, without moving or merging directories, so old
and new binaries can coexist. No reinstall is required.

## Configuring a profile

`corral-pi` is a built-in harness: you do not declare a `[harnesses.corral-pi]` table. A
profile selects it like any other harness:

```toml
[profiles.pi]
harness = "corral-pi"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
# A development or fixture asset directory; leave unset to use the managed installation.
assets_dir = "/path/to/pi-assets"
```

Model and effort travel in the host's one-shot bootstrap, not on the command line, so a
corral-pi profile takes no `args`. The `openai`, `anthropic`, `amazon-bedrock` and `llama.cpp` vendors are allowed (for
example `model = "claude-opus-5-5"` with `vendor = "anthropic"`); any other vendor is refused
when you dispatch, before a worktree exists.

Other `[agent]` settings apply only to corral-pi runs: `question_unattended_secs` (default
600), `gate_start_uncertain_secs` (default 300) and `drain_secs` (default 60).

### Retained memory

Bound corral-pi PMs, assistants and workers have `corral_memory_list`,
`corral_memory_read`, `corral_memory_put` and `corral_memory_remove`. All four use
authenticated operations and receipts; selectors cannot choose a workspace root or author.
List pages contain at most 20 entries; body reads contain at most 4096 UTF-8 bytes.
Writes require a freshly read content hash (explicit null for creation), and the service
stamps author/time. Missing-receipt mutations have unknown file effects: explicitly reread
before any new intentional write, never automatically retry.

Canonical Markdown lives in the bound workspace's `.corral/memory`, not the coding
worktree. Changes are immediately visible without a commit; memory operations never
change HEAD or the index. Use normal explicit workspace commits to preserve history.
Memory is not a secret store.

Launch and explicit resume capture working-tree summaries in a private generation-local
`memory-context.md`. The complete projection is at most 12,288 UTF-8 bytes: up to 6 KiB
workspace, 2 KiB own shared role, and (workers only) 4 KiB trusted bound-project memory.
PMs and assistants do not preload project memory. Whole entries are omitted with a count
and retrieval hint; long notes and people context stay available through paginated tools.
Provenance and sources are labelled; memory is fallible retained context below current
instructions and policy, never a grant, owner message or task-state change.

Reload and compaction retain the same generation block through shared Pi/Claude prompt
assembly without appending conversation messages. Active sessions explicitly reread newer
entries with the tools; there is no live watcher or OmO fallback. Installed assets must
advertise `memory_projection: 1` in `compat.json`; reinstall older hosts rather than
silently dropping memory. Bare Pi, explore children, reviewers and planning roles receive
no memory tools or projection.

Workers read workspace and read/write their own role and bound project; they propose
workspace knowledge through the PM inbox. Worker-role guidance is shared across projects,
not a private persona store. PM project access follows effective approved scope; assistants
explicitly select registered projects. Project reads respect project-text trust.
Reusable evidence-backed project rules remain lessons, and unfinished work remains in
inbox/tasks; link those records rather than copying mutable operational history.

### Owner choice questions

Only assistant and PM loadouts with `ask_user` enabled expose owner choices. Their contracts
and tool guidance require it for real user-facing questions, decisions and clarifications,
including implement-now/queue and ship/later/drop decisions. Offer 2–4 meaningful options,
label a recommendation when useful, and optional free text/Other. Research discoverable facts
instead of asking, batch independent decisions, and act on clear instructions without asking
again. Generic choices never replace owner-action confirmations or file-permission dialogs,
and do not change worker/reviewer/planner board-question routing or PM authority.

Existing calls with `questions` alone keep their synchronous batched dialog, including headers,
label/description options, multi-select and partial answers. Deferred questions use plain-string,
single-selection options with optional free text; richer batches must use the synchronous dialog.
The deferred schema rejects unsupported rich fields rather than silently dropping them. To keep working:

```json
{"questions":[{"question":"Which scope?","options":["Small fix (recommended)","Broader change"],"freeText":true}],"nonBlocking":true,"key":"scope"}
```

The result contains `id`, `status: "pending"` and an expiry, without opening a modal, waiting
for an answer, or retaining a tool/model/composer lock. A persistent widget and footer count
remain beside the main composer in both rich and plain TUI. Use `/questions`, Ctrl+Shift+Q,
or click the widget in fullscreen to reopen choices; `/questions <id>` opens a specific one.
The owner can inspect output and keep typing while questions wait. Opening a dialog is an
explicit owner action; it never replaces an already-open confirmation/permission dialog.

Continue unrelated reads, research or implementation. Stop only steps dependent on the
answer, especially permission/scope decisions. With Todo enabled, use its existing `wait`
operation on those steps, naming the pending ID; when everything remaining waits, end the
turn. The answer's normal follow-up resumes the conversation; do not busy-poll. Without
interactive TUI support (including headless/RPC), the deferred call returns `no answer` and
an unavailable-UI reason, not a fabricated pending question or approval.

Lifecycle and recovery:

- A repeated `key` (or the same tool-call ID when omitted) returns the original ID/status;
  different questions with that key refuse. Up to 8 pending questions and 128 distinct
  questions per Pi session are supported. Resume does not reset these bounds.
- Escape or a five-minute answer-dialog timeout discards partial input and leaves the
  question pending for reopening. `/questions cancel <id>` explicitly cancels; the model
  can withdraw with `ask_user` input `{"withdraw":"<id>"}`. A new deferred call may name
  `supersedes: "<id>"` to withdraw an obsolete pending question. Pending choices expire
  after 24 hours, including time offline. Cancellation, expiry and withdrawal record no
  owner consent; late or duplicate answers cannot reopen them.
- The existing Pi session journal stores question state, actual selected answers and
  delivery identity. A host exit suspends unresolved choices; reopening the same session
  restores them. Session replacement closes them; a closed/stale host cannot deliver a
  late reply. A canonically ended Corral session cannot rebind or restart from a reply.
- Only an actual complete UI answer enters the authenticated host owner-input event path
  (assistant `dialog` provenance). It includes the question ID, option and optional text,
  then arrives via the existing delivery module and a normal follow-up user message. PM
  answers remain PM-session input, not assistant owner-action authority. Status notifications
  and pending IDs are never citable owner input. Current-generation owner-input receipts
  are replayed with the same saved event ID after uncertain acknowledgement.
- An answer saved before a crash but not consumed is delivered on resume. A successor
  generation journals that genuine saved selection in its own generation before delivery.
  The persisted user message is the consumption receipt: a consumed answer is never resent,
  even after compaction/reload/resume. Delivery failures leave an answer-recorded indicator;
  `/questions` or resume reconciles it without asking for another answer. Terminal records
  remain in the session journal, not a second board-question store.

Unavailable/disabled UI, missing answers, cancellation and timeout are never approval.
Report the limitation and unresolved decision truthfully; do not silently proceed or
repeatedly re-ask in prose. These changes require rebuilt/reinstalled managed assets and a
new/resumed generation: editing source does not update an already-running compiled host.
See [installation](#installing-the-host).

### Language servers

Workers retain built-in Rust and TypeScript servers in every preset. Reviewer LSP is enabled
in full; planner and plan-reviewer LSP in standard/full; PM has no LSP. Per-role `lsp.enabled`
can override these defaults. Configure extra servers in the workspace configuration:

```toml
[agent.capabilities.worker.lsp]
enabled = true
[agent.capabilities.worker.lsp.languages.example]
command = "example-language-server"
args = ["--stdio"]
extensions = { ".ex" = "example" }
root_markers = ["example.toml", ".git"]
```

Configured extensions take precedence over built-ins. The nearest marker-containing ancestor
inside the worktree is the server root; absent a marker the worktree is used. Servers are
same-user subprocesses, outside the in-process network guard; only diagnostics, definition and
references are exposed, not executeCommand or workspace edits. Config is snapshotted per
generation and changes take effect on explicit resume. Interrupted LSP calls are conservatively
unknown because server startup can have effects, including for built-in servers.

### Capabilities

Optional corral-pi features are selected per role by one shared catalog:

```toml
[agent.capabilities]
preset = "standard" # default; "full" adds more where a feature defines it, "strict" turns optional features off

[agent.capabilities.worker]
todo = false       # optional per-role override

[agent.capabilities.pm]
todo = true        # optional per-role override, including under strict
```

Role tables are `worker`, `reviewer`, `pm`, `planner` and `plan_reviewer`. A key that is
not in the catalog, or that the role may not use (todo for a reviewer), is a configuration
error. The catalog today, as strict / standard / full:

| Key | Worker, PM | Reviewer, planner, plan reviewer |
|---|---|---|
| `todo` | off / on / on | not available |
| `write_permissions` | not available (assistant only: off / on / on) | not available |
| `llama_cpp.enabled` | off / off / off | off / off / off |
| `web.enabled` | worker off / on / on; not available to PM | off / on / on |
| `context_files` | off / on / on | off / on / on |
| `skills` | off / on / on | off / on / on |
| `prompt_templates` | off / on / on | off / on / on |
| `compaction` | on / on / on (upstream defaults) | on / on / on |
| `lessons` | worker on / on / on; not available to PM | on / on / on |
| `session_export` | worker off / on / on; not available to PM | off / on / on |
| `mcp.enabled` | off / on / on | off / on / on |
| `tool_search` | off / on / on | off / on / on |
| `codemode` | worker off / on / on; PM off / off / on | off / on / on |
| `hooks` | worker off / off / on; not available to PM | not available |
| `bash_rules` | worker off / off / on; not available to PM | reviewer off / off / on; not available to planning roles |
| `background_shell` | off / on / on (also assistant) | not available |
| `role_enabled` | not available | planner and plan reviewer off / on / on; not available to reviewer |
| `browser` | worker off / on / on; not available to PM | not available |
| `checkpoints` | worker off / off / on; not available to PM | not available |
| `headless` | worker off / off / off; not available to PM | reviewer off / off / off; not available to planning roles |

`headless` is never a preset default; set it per role (see [Headless runs](#headless-runs)).

Assistant-only `write_permissions` allows scoped owner dialogs for external edit/write calls.
For example, `[agent.capabilities.assistant.write_permissions]` with
`allow = ['~/.agents/skills']` and `deny = ['~/.agents/skills/corral']` skips prompting in the
first directory except the denied subtree. Settings apply next generation; fixed protected
paths always refuse. See [assistant write permissions](assistants.md#write-permissions) for
scope limits, once/hour/session choices, inspection and revocation.

A compound key also takes a table with an optional `enabled` and its settings:

```toml
[agent.capabilities]
trusted_projects = ["*"]        # default: every registered project; [] trusts none

[agent.capabilities.worker.skills]
user_dirs = ["~/.agents/skills", "~/.corral/skills/corral-pi", "/opt/team-skills"] # [] loads no home skills

[agent.capabilities.reviewer.prompt_templates]
user_dirs = []                   # default ["~/.agents/prompts"]

[agent.capabilities.worker.compaction]
reserve_tokens = 16384           # omitted values keep Pi's defaults
keep_recent_tokens = 20000
branch_summary = { reserve_tokens = 16384, skip_prompt = false }
```

`[agent.capabilities.worker.background_shell]` takes `max_jobs` (default 4, at most 16),
`max_output_bytes` (retained per job, default 8 MiB), `max_runtime_secs` (default 3600),
and `wait_ms` (foreground observation budget, default 10000, range 0–600000). PM and assistant
use the same settings under their existing role capability; this does not enable the capability.

`user_dirs` entries are absolute or `~/` directories; a missing directory is skipped. Web takes
its own `[agent.capabilities.<role>.web]` table (below).

`role_enabled` switches a native planning role itself. With it off (preset `strict`, or
`role_enabled = false` under `planner` or `plan_reviewer`), launching that corral-pi host refuses
with `role_disabled` before any binding is recorded; corral never substitutes another harness.
An installed host older than the planning roles is refused with `role_unavailable` until
`corral agent install` updates it. See [the planner and plan reviewer roles](#the-planner-and-plan-reviewer-roles).

### Guidance and session export

Under standard and full every role loads, as text only:

- Agent Skills from its `skills.user_dirs` (by default `~/.agents/skills` and
  `~/.corral/skills/corral-pi`), plus the
  project's `.agents/skills` directories from the working directory up to the repository root;
- prompt templates from its `prompt_templates.user_dirs` (by default `~/.agents/prompts`), plus
  the project's `.agents/prompts`, as `/name` commands; skills also expand with `/skill:name`;
- the `AGENTS.md`/`CLAUDE.md` context files from the working directory up to the corral
  workspace root, in that order, where a linked worktree's file replaces its main checkout's.

Workers and reviewers read project text from their worktree. The PM loads guidance from the
workspace root only, including its AGENTS.md chain, not from any project's checkout. Project
instructions remain reachable by reading files as needed; they are not preloaded.

Project files (context files, `.agents/skills`, `.agents/prompts`) load only when the task's
project is in `trusted_projects`; the decision is recorded in the generation's snapshot as
`project_text`. The workspace PM records trust for the workspace root's own guidance instead;
this does not trust or preload its project checkouts. Home directories do not depend on it. Pi's own discovery stays off: nothing is
read from `~/.pi` or a project's `.pi` (settings, extensions, MCP, skills, prompts), and no
ambient extension, MCP server or theme loads. The worker keeps its packaged corral-worker skill in every
preset; a home or repository skill with the same name is skipped (the pane shows the conflict). `corral agent state` lists the selected paths under `guidance`.

`lessons` is the dispatch prompt's existing project-lessons section; turning it off for the
worker drops that section, and its exposure record, from corral-pi worker prompts only. Lesson
storage is unchanged, and the PM keeps its `read_lessons` tool.

`compaction` passes its settings to Pi's automatic compaction and `/tree` branch summaries.
A summary records its own request usage on its session entry, and corral's usage collector
counts it toward the run with the assistant responses.

With `session_export`, the owner can type `/corral-export` in a worker or reviewer pane. The
host writes the bound session's current branch as one offline HTML file into the private
`exports` directory beside the session file and attaches it to the run (or review child) as a
`log` artifact. Nothing is uploaded. A snapshot is the session's last transcript entry; corral's own
bookkeeping entries do not change it. Exporting the same snapshot again reuses the file and the
request ID, so corral replays the first receipt instead of attaching a duplicate; after a new turn
it is a new snapshot and a new artifact. A failed export is reported in the pane and as a
`session_export_failed` host event. Pi's own `/export` and `/share` are unchanged.

Each generation records its resolved settings and their digest once. `corral agent state`
shows them with where each value came from, and, after an explicit resume with changed
settings, what changed since the previous generation. Older generations without a record
show as `legacy_strict` and still resume. `corral agent doctor` lists every role's
settings against the installed host: a default the host does not implement is shown as
unavailable and left off, while an explicit `true` for it refuses the launch until you run
`corral agent install` or remove the override. Capabilities never grant Corral authority;
Corral tools also refuse to run from inside another tool. See
[the capability record](../records/corral-pi-capabilities.md).

### Curated corral-pi skill pack

`corral agent install` also installs six lean, Pi-compatible Agent Skills. To install or refresh
just the pack, run `corral skill install --harness corral-pi`. Worker dispatch also ensures the
pack is current. A fresh standard/full session lists these skills and can load them by reading
its listed `SKILL.md` or with `/skill:name`:

| Skill | Guidance |
|---|---|
| `frontend` | Web UI build, style, responsive layout, accessibility and polish |
| `visual-qa` | Fresh screenshot comparisons and interaction checks, with explicit blocked checks |
| `programming` | Typed boundaries/errors, exhaustive variants and tests that can fail |
| `debugging` | Competing hypotheses, discriminating probes and a reproduced fix |
| `git-master` | Atomic commits, safe rebases and evidence-led history search |
| `simplify-and-cleanup` | Bounded code, comment and docs cleanup with behavior/meaning preservation and proportional verification |

Ask for cleanup naturally or invoke `/skill:simplify-and-cleanup` with explicit files or a diff/base
and an audit/apply request. Review requests remain read-only; cleanup requests follow the role's
existing edit permissions. The skill prefers existing checks, adds tests only for concrete uncovered
behavioral risks, and skips changes that need substantial new testing to establish safety. It does
not waive required gates or review, and making no edits is a valid outcome.

The pack lives only in `~/.corral/skills/corral-pi/<name>/SKILL.md`, outside OmO's bundled,
`.omo` and shared `.agents` discovery roots, so it does not duplicate OmO's own skills. Do not
copy it into shared roots if you also use OmO. Each managed file carries corral's version and
content digest; reinstall refreshes managed files. Local edits inside managed `SKILL.md` files
are overwritten. Unmanaged skill files and extra owner files beside them are left alone (unless
you explicitly request `--force` for the skill file).

To disable only the pack for a role, set its `skills.user_dirs = ["~/.agents/skills"]`, omitting
the Pi-only root. To extend it, add a personal/team directory to the list while keeping both
default roots; entries replace, rather than append to, the defaults. Put custom skills in that
separate directory and use distinct names. `user_dirs = []` disables home skills but not trusted
project skills; `skills = false` disables optional skill loading entirely (the worker's packaged
corral-worker remains). Strict has optional skills off by default. Changes apply to the next
generation. Repeat per role as needed.

The pack grants no new tools. Visual QA uses an available Pi browser integration or a repository
capture harness; without either it reports blocked visual checks, never a fabricated visual PASS.
The pack's tests check tool references against the standard worker's actual tool registry.

### Codemode

`codemode` runs JavaScript cells on Corral's own persistent Bun kernel (T-682): one kernel per
host session, started on the first cell, with no QuickJS or other engine behind it. Use it
to batch independent tool calls, chain dependent ones and filter large results before anything
reaches the model. Standard enables it for worker, reviewer and planning roles; PM requires full
or an explicit `codemode = true` in its role table. Strict and `codemode = false` leave the tool
absent, and every direct tool keeps working. MCP and tool search are independent: neither
enables codemode, and codemode needs neither.

Arguments: `code` (raw JavaScript, not a JSON string or fence), `wait_ms` (foreground window,
default 60000, at most 600000), `cancel` (`"cell-N"`, stops that live cell) and `reset` (discard
every variable, then run `code` if given).

**Globals.** `tools.<name>(args)` returns a Promise of that tool's result. `print(...)` and
`console.log/info/warn/error/debug` emit text, `display(value)` emits text or an image
(`{type: "image", data: <base64 or bytes>, mimeType}`), and `setTimeout`/`clearTimeout` schedule
work inside the cell. There is no `Bun`, `process`, `require`, `fetch` or `import`: every
capability is a tool call. Only printed or displayed output and an explicit `return` value reach
the model, so keep large intermediate results in variables.

**Persistence and reset.** Top-level `const`/`let`/`function`/`class` declarations persist from
cell to cell. They are discarded, and the result says so, by `reset`, by cancelling a running
cell, at its deadline, on a reload, and when the host restarts. Nothing is snapshotted or
re-run. Cells of one session run one at a time in submission order; calls inside a cell overlap.

**Authority.** A tool usable directly is usable from a cell and nothing else is: the callable
list in the tool's description and every nested execution derive from the same loadout check,
and Pi's validation, the host's `tool_call` controls, shell rules, hooks and owner confirmations
apply exactly as to a direct call. Corral mutations are allowed; another tool that nests calls
through `ctx.executeTool` may still nest only the receipt-free board reads, because only a cell
journals its children. Each nested call has its own
identity, `<codemode call id>/<cell>.<n>`: receipted tools derive their request ID from it and
send it as `identity.parent`, which corral checks against the request ID and stores on the
receipt, so a replayed child returns its one receipt and a new child never reuses one. Before a
child starts and when it settles the host appends a `corral_code_call` entry to the session; if
the host is lost, resume classifies every child without a settled entry from its own receipt,
including children of a detached cell. A confirm-class owner action shows the owner its command
and waits for the actual answer; other calls in the cell keep running meanwhile, and code never
supplies an answer or authority.

**Result shapes.** Corral tools resolve to their canonical response object: a receipt
(`request_id`, `op`, `state`, `result`, `identity`, `replayed`), refusals included, or a host
failure `{request_id, op, error: {code, category, reason, effect, lookup?}}`. Other tools resolve
to their text. A call refused or failed without a structured result rejects with an `Error`
whose `name`, `message` and `code` (`denied`, `not_run`, `tool_error`, `stale_binding`) survive
`JSON.stringify`. The model-visible result keeps the cell's own status (`ok`, `error`,
`cancelled`, `timeout`, `reset`, `closed`) apart from its nested calls: counts of ok, failed and
still-running children and committed receipts, then a bounded list of every failure with tool,
child identity, request ID, error code and message, and recorded effect (a receipt state, `read`,
`not_started` or `unknown`). The list appears even when the script caught the failure.

**Detached cells.** A cell still running after `wait_ms` detaches: the call returns its handle at
once and the cell settles in the background. Its settlement is reported exactly once: by the
`cancel` call that stopped it, or by one notice that enters the session through the host's bridge
delivery, journaled as `corral_delivery` with key `code:g<generation>:<cell>` and its own
bridge-origin turn. The notice is bounded at 32 KiB, but its status, nested-call counts, failure list
(failures past the eighth still named by tool) and return value always stay whole; only printed
output yields, with a note saying how many bytes it shows. Keep a large result in a top-level
variable to read it in a later cell. Do not poll. Continue independent work; when a plan step needs the result,
record that step as waiting on event `code cell <cell>` with `corral_todo` so request
continuation (T-679) does not wake you meanwhile: with only waiting steps open, corral decides
`waiting_event` and continues nothing, while an actionable independent step is still continued. A
reload settles running cells `closed` and
reports them after the reload; a host shutdown reports nothing, and resume's recovery notice
classifies their children instead.

**Failures.** A failed or interrupted cell is never re-run, and committed nested effects stand.
Reconcile an uncertain mutation with `corral_receipt_lookup` (its failure names the lookup)
instead of re-running the cell. Tool subprocesses keep their ordinary OS and network access; the
kernel is not an OS sandbox.

```javascript
// Independent reads in parallel; only the filtered lines reach the model.
const files = await Promise.all(["README.md", "AGENTS.md"].map(path => tools.read({ path })));
print(files.map(source => source.split("\n").filter(line => line.startsWith("#"))));
```

```javascript
// A dependent read -> mutation -> read chain (PM): each step uses the previous result.
const shown = await tools.corral_show({ task: "T-9", section: "overview" });
if (shown.state !== "committed") throw new Error(shown.result.code);
const made = await tools.corral_comment({ task: "T-9", text: "Checked the overview." });
if (made.state !== "committed") throw new Error(made.result.code);
const back = await tools.corral_receipt_lookup({ request_id: made.request_id, task: "T-9" });
return { comment: back.state, request: made.request_id };
```

```javascript
// An uncertain mutation is reconciled, never retried blindly.
const asked = await tools.corral_ask({ kind: "clarification", text: "Which input?" });
if (asked.error?.lookup) {
  const found = await tools[asked.error.lookup.tool](asked.error.lookup.arguments);
  return found.lookup.found ? found.state : "not committed yet";
}
```

A fixture pipeline that reads a 819,232-byte tool result and returns one selected line emitted
112 bytes to model context; calling the same tool directly emitted 819,227 bytes
(`test/codemode-compose.test.ts`, `CODEMODE_EMITTED_BYTES`).

### Local inference (llama.cpp)

Use an already running OpenAI-compatible chat endpoint. Corral does not start llama.cpp,
install a runtime, download weights, or enable Pi's model-management extension:

```toml
[profiles.local]
harness = "corral-pi"
vendor = "llama.cpp"
model = "my-model.gguf"  # endpoint-owned ID; names containing "/" are supported
effort = "off"

[agent.capabilities.worker.llama_cpp]
enabled = true
endpoint = "http://127.0.0.1:8080"
discover = true
context_window = 32768
max_tokens = 8192
timeout_ms = 15000
```

Select this profile through the normal routing configuration. Configure the corresponding
`reviewer`, `pm`, `planner` or `plan_reviewer` capability table for another role; role
availability and Corral authority are unchanged. Local inference is explicitly off in
strict, standard and full. A `llama.cpp` profile requires the role's local capability.
Enabling it also permits the same role's cloud profiles: the selected profile still
determines its catalog, credentials and transport, without contacting the local endpoint.
No cloud login is needed for a local profile.
`corral pi --bare` has no bound capability configuration and does not offer this route.

`endpoint` is one HTTP(S) origin, optionally with a trailing slash, without a path,
credentials, query or fragment. No ambient `LLAMA_BASE_URL`, API key or foreign credential
store supplies it. Discovery reads only `GET /v1/models` (up to 1 MiB and 4,096 entries)
and selects the profile's exact model ID; an optional `llama.cpp/` prefix is stripped.
New IDs need only server/config changes, never a source patch. A missing ID refuses
startup with `local_model_unavailable`. For a server without model listing, set
`discover = false` to use the configured ID directly without a catalog request.

The in-process guard admits only that origin's catalog GET and
`POST /v1/chat/completions`; redirects, router load/download operations, cloud endpoints,
cookies and foreign authorization are refused. Inference uses a fixed non-secret
`Bearer local` marker, so this route is for endpoints that do not require a secret key.
The request timeout and token limit come from the generation snapshot; `max_tokens`
must not exceed `context_window`. Explicit model refresh can rediscover IDs at the same
origin. Reload retains the generation settings; explicit Corral resume resolves current
settings and records their difference without rewriting the old snapshot.

Local responses retain input, output and cache token counts with Pi-reported USD zero.
Zero cost never means zero tokens. Corral's existing collector still reads these
assistant records; API-equivalent pricing gaps remain explicit for IDs without configured
prices. No metered inference allowance is consumed by the deterministic HTTP fixtures.

### Web access

Enabled worker and reviewer generations expose `web_fetch`, `web_read` and `web_search`; the same
capability is available to native planning roles when installed. PM cannot enable it.
Strict disables web unless the role explicitly opts in:

```toml
[agent.capabilities.worker.web]
enabled = true
hosts = ["*"]             # default: all public HTTP(S) hosts
max_bytes = 1048576        # default: response body byte limit
timeout_ms = 15000        # default: whole request, including DNS, redirects and body
max_redirects = 5         # default; supported range 0..10
# searxng_url = "http://127.0.0.1:8888/" # optional base URL or /search endpoint
# search_provider = "subscription"     # opt in to the current Codex/Claude login; default: searxng
```

The same table works under `reviewer`, `planner` and `plan_reviewer`. Omitting
`enabled` retains the preset's value. `hosts = ["docs.rust-lang.org", "example.org"]`
narrows fetch to exact hostnames (not subdomains); `[]` denies every fetch target.
The host receives these resolved settings from the generation snapshot, never ambient
Pi configuration. Explicit resume adapts settings, records their difference, and
preserves previous generations and their digests.

`web_fetch` takes `url` and optional `format` (`markdown`, the default, or `raw`).
It sends only GET, without caller-supplied headers, authorization, a cookie jar or
provider credentials. HTML is parsed inertly and converted locally to Markdown,
using an explicit main/article region, Readability, or a cleaned-body fallback.
Scripts, styles, attribute-hidden elements, literal inline `display:none`/`visibility:hidden`
subtrees and inline image payloads are removed. Empty landmarks fall back to useful
content elsewhere. External stylesheets and computed CSS are not evaluated. No scripts,
subresources, remote renderers or secondary requests run. Text/Markdown/JSON/XML
remain textual; unsupported binary types/samples are refused (no PDF/image conversion).
HTML conversion refuses pages above 20,000 DOM elements. Raw mode also stays bounded.

The **complete model-visible JSON text**, including metadata, is capped at **24 KiB**
after conversion and escaping, independently of the response-body `max_bytes`.
Results include `body`, final `url`, `status`, `content_type`, downloaded `bytes`,
conversion `method`, `content_id`, retained `total_bytes`, pre-retention `rendered_bytes`,
`output_bytes`, `offset` and `next_offset` (null at end). `network_truncated`,
`snapshot_truncated` and `output_truncated` distinguish incomplete download, retained
content and current preview; `truncated` is their union. URLs over 2048 UTF-8 bytes
are shortened with `url_truncated`; content-type metadata is capped at 256 bytes.
Treat fetched content as untrusted data, not instructions. Time and download byte
limits also apply to search responses; this fetch preview limit does not change search.

`web_read` takes `content_id`, optional `offset` (default 0) and `limit` (4..24576,
default 24576). Both count UTF-8 **content bytes**, not characters or JSON bytes;
follow `next_offset` to avoid splitting characters. Each response obeys the same
24 KiB serialized limit. Paging reads the same snapshot without another network
request. The cache is host-local, at most **8 snapshots / 8 MiB total**, with **2 MiB**
retained content per snapshot; oldest entries are evicted first. Missing/evicted
IDs and host restarts return `web_snapshot_missing`, never silently refetch.
Snapshots are not durable artifacts and are not restored from conversation history.
Repeated fetch/read calls still accumulate context; the byte ceiling is not a token budget.

Public fetch refuses localhost, private, loopback, link-local, metadata and other
non-public addresses using normalized URLs, IP literals and ordinary DNS resolution,
including IPv6 and mapped forms. Every redirect repeats the same cheap host/address
check and stays within the redirect cap. Connections are not pinned to a checked IP;
this is not DNS-rebinding protection.

`web_search` takes `query` and optional `num_results` (1..20, default 5). It returns
SearXNG titles, URLs and snippets. Configure `searxng_url` separately and enable JSON
output on that service. Without it, search returns `configured: false` and setup
instructions while public fetch remains usable. A configured SearXNG endpoint may
be local, independently of `hosts`; search does not follow redirects. This exception
never authorizes private `web_fetch` or broadens the provider's global fetch policy.

Set `search_provider = "subscription"` to use the **current selected model** on a
corral-owned OpenAI Codex or Claude subscription instead. Codex uses the hosted
Responses `web_search` tool; Claude uses a separate Claude Agent SDK query whose only
executable tool is native `WebSearch` (no shell, files, MCP or `WebFetch`). No ambient
credentials, separately billed API route, SearXNG fallback, or alternate model is
selected on failure. Availability depends on the subscription, model and region;
unsupported providers, quota/auth failures and answers without completed search
evidence fail explicitly. This does not guarantee the account has extra usage disabled.

Subscription results include `provider`, `model`, `request`, a synthesized `answer`,
and source `results` (titles/URLs, capped by `num_results`). The valid JSON envelope
is bounded by the smaller of `max_bytes` and 32768 bytes, with `truncated` set when
sources or answer are shortened. `timeout_ms` covers queueing and execution. Fetch
host restrictions still apply to `web_fetch`, not provider-side search sources.

Each search is serialized per host and registered as auxiliary `web_search` usage
before execution, with its own transcript and model usage. Interrupted/failed work
retains uncertain accounting; the host does not replay the invocation. Hosted service work
has no reliable finite priced reservation, so a **hard token or USD budget refuses
subscription search before model execution**; unlimited budgets still record usage. Provider
model-token pricing is API-equivalent accounting, not a subscription invoice or a
complete search-service fee estimate. Existing SearXNG behavior is unchanged.

Both tools share the execution check, including nested calls and reloads. Public
fetch and SearXNG use the host network ledger; Codex search uses its guarded provider
transport. Claude subprocess networking remains outside the in-process guard.
Interrupted fetch is `interrupted_no_effect`; interrupted search is conservatively
`outcome_unknown` because subscription quota may have been consumed.

### Workspace MCP and tool search

MCP connects only servers in Corral's workspace configuration. Each role has its own
tool/resource policy; each server must explicitly name its enabled projects:

```toml
[agent.capabilities.worker]
tool_search = true

[agent.capabilities.worker.mcp]
enabled = true

[agent.capabilities.worker.mcp.servers.docs]
projects = ["corral"]
url = "http://127.0.0.1:8080/mcp"
tools = { lookup = "read" }
resources = ["docs://public/*"]

# A stdio server uses command/args instead of url:
# command = "/usr/local/bin/docs-mcp"
# args = ["--stdio"]
```

The same tables work for reviewer, PM and planning roles, whose tool entries must be
`"read"`. Worker may also configure `"write"` operations outside Corral. Known shell
and write proxies cannot be labeled as reads for read roles. Corral proxy tool names
are refused for every role; Corral authority still uses top-level `corral_*` bridge
operations. Server annotations never authorize anything. Resources use exact URIs or
a trailing `*` prefix scope; an empty resource list denies access.

New standard/full generations enable MCP and cached `tool_search` independently.
With no project servers, `mcp_connect` returns `configured: false`, not a startup
failure. An unavailable configured server appears in host state and can be retried
explicitly with `mcp_connect`. Strict disables both unless individually overridden.
MCP never enables codemode or a disabled search capability.

With search off, approved discovered MCP tools are directly active. With search on,
`tool_search` returns cached names, descriptions and schemas without connecting or
changing the active loadout. `mcp_activate` takes `server` and the returned `tools`
names, refreshes the server and activates only permitted identities. Both paths work
without codemode. Direct, nested and deferred execution use the same policy; denied
calls do not reach the server. `mcp_connect` refreshes schemas for approved identities
without schema hashes or reapproval. New unapproved names remain unavailable.
`mcp_resources` and `mcp_read_resource` enforce the same project/server/URI policy.

The host ignores repo/home `.pi/mcp.json`, ambient Pi settings and extensions, and
foreign credential stores. Inline MCP credentials and OAuth discovery are not
configured by this adapter. HTTP MCP uses only its configured HTTP(S) origin through
a separate transport, refuses redirects and does not borrow provider authorization.
Stdio servers run as trusted subprocesses outside the in-process network guard.
Each request has a 15-second timeout and a 1 MiB transport-message cap. Reload and
normal host shutdown close owned MCP connections and stdio process groups.

The generation snapshot and digest include the resolved MCP configuration. Explicit
resume adapts it while preserving old snapshots. An interrupted local `tool_search`
is `interrupted_no_effect`; MCP connections, activation, tool and resource calls
remain `outcome_unknown`, regardless of read annotations.

### Hooks and bash rules

Two worker-side controls run inside the host, in full or by role override. Both come only
from corral configuration and are resolved into the generation snapshot:

```toml
[agent.capabilities.worker.hooks]
enabled = true
timeout_ms = 60000                 # default; per hook, 1..600000

[[agent.capabilities.worker.hooks.pre_tool]]
name = "lint"                      # [A-Za-z0-9._-], unique per phase
argv = ["scripts/lint.sh", "--quick"]
tools = ["edit", "write"]          # omitted: every local tool

[[agent.capabilities.worker.hooks.post_tool]]
name = "fmt"
argv = ["cargo", "fmt"]
tools = ["edit"]

[[agent.capabilities.worker.hooks.turn_end]]
name = "notify"
argv = ["notify-send", "turn done"]

[agent.capabilities.worker.bash_rules]   # also under reviewer
enabled = true
deny = ["git push*", "rm -rf *"]
ask = ["git reset --hard*"]
allow = []
default = "allow"                  # allow, ask or deny for a command no rule matches
```

Hooks are programs run with argv (no shell) in the worktree, with the call as JSON on stdin
and `CORRAL_HOOK_PHASE`, `CORRAL_HOOK_NAME`, `CORRAL_HOOK_INVOCATION` and
`CORRAL_HOOK_TOOL` in the environment. A pre-tool hook that exits non-zero, cannot start or
times out refuses the call with its output tail; a failing post-tool hook appends a note to
the result; a turn-end hook runs once as each turn settles. A timed-out hook's process group
is killed. Hooks wrap only top-level local calls that passed the role check and bash rules:
never a corral operation, a denied call, or a call another tool issued. Hook subprocesses
are trusted same-user programs outside the in-process network guard, like bash.

Before launching a hook the host appends a `corral_hook` entry (`prepared`) to the bound Pi
session, then `started` and `completed`. After host loss, recovery uses these records and
the lost generation's own hook settings: a call a hook was recorded around, or that a
configured pre/post hook wraps, is `outcome_unknown` even when it is a read, and a hook
that never completed (a turn-end hook included) is listed as `hook:<phase>:<name>` with
its invocation ID. Hook scripts are never replayed.

Bash rules decide every bash call, nested ones too, before it runs. A command is split on
`;`, `&&`, `||`, `|`, `&` and newlines, and the bodies of `$(...)`, backticks and the
command string of an `sh`, `bash`, `dash`, `zsh` or `ksh` invocation whose options include
`c` (`-c`, `-lc`, `-ec`, `-o pipefail -c`) are checked as well; quotes are not parsed, so
splitting errs toward more pieces. Patterns are globs (`*` any text, `?` one character) matching a whole
piece. Deny wins over ask, ask over allow; under `default = "ask"` or `"deny"` every piece
must match `allow`. A denied command fails with the rule and nothing runs.

An `ask` command also fails at first, naming a `bash-permission:<sha256>` reference. The
worker asks with `corral_ask` kind `permission`, options `allow`/`deny`, that reference in
`context_refs` and the exact command in its text. When corral delivers the owner's answer
to that question choosing `allow`, the identical command runs once; it is decided again
from the start, so a deny rule still wins. The PM cannot answer permission questions, and
a correction to another option revokes an unused grant. Grants live in the host process:
after recovery, ask again.

### Browser

Workers get `browser_navigate` and `browser_screenshot` under standard and full. They load
only a top-level target the generation allows: an exact configured dev origin, or an `.html`/
`.htm` file inside the worktree (a relative or absolute path or a `file:` URL; symlinks may not
leave the worktree). Data, `javascript:`, other schemes and unconfigured origins are refused
before any browser starts:

```toml
[agent.capabilities.worker.browser]
origins = ["http://127.0.0.1:5173"]   # default []: worktree HTML only; scheme://host[:port], no path
timeout_ms = 30000                    # default; bounds start-up, then the page, 1..600000
# executable = "/usr/bin/chromium"    # default: google-chrome, chromium or chrome on PATH
```

Each call starts one headless Chrome or Chromium with a fresh temporary profile and HOME, so
no owner cookies, logins or extensions are visible and nothing carries over between calls.
Start-up and the page are bounded separately by `timeout_ms`: the browser must answer its first
DevTools command within it, and the page's budget starts only then, so a slow launch on a
loaded host does not use up the page's time. It waits for the page's load event, then checks
the URL it landed on: a redirect or scripted
navigation to a target outside the allowed set fails the call without returning the page.
`browser_navigate` returns the final URL, title and visible text (64 KiB, with `truncated`).
`browser_screenshot` returns a PNG inline (optional `width`, `height`, `full_page`); with
`save_as` (a worktree-relative `.png` path) it also writes the file, ready for
`corral artifact add`. Nothing is uploaded.

Only the top-level document is checked. Page scripts run, and their subrequests are neither
intercepted nor routed through the in-process network guard; like bash, the browser
subprocess's network is outside it. The browser speaks DevTools over a private pipe: a
finished, failed, timed-out or cancelled call kills its process group, waits for the exit
and removes the profile, and if the host dies the pipe closes and the browser exits. An
interrupted browser call is `outcome_unknown`; it is never replayed.

### Todo progress

New worker and PM generations expose `corral_todo` by default (the `todo` capability).
Reviewers and planning roles cannot mutate todo ledgers. Reinstall the host to get its
todo tool.

`corral_todo` accepts `init`, `append`, `start`, `done`, and `drop`. Initialize with
`{"op":"init","phases":[{"id":"build","label":"Build","steps":[{"id":"wire","label":"Wire the tool"}]}]}`.
Append uses `{"op":"append","phase":"build","steps":[{"id":"check","label":"Check the result"}]}`.
Start or finish a step with `{"op":"start","step":"wire"}` or `{"op":"done","step":"wire"}`.
Drop uses `{"op":"drop","step":"check","reason":"Covered by another check"}`.
IDs stay stable. Only one step may be in progress; finish or drop it before starting
another. Done/dropped steps are terminal and init cannot replace an existing ledger.
Worker steps may include registered acceptance `criteria` IDs; PM steps cannot.

The server selects the worker's current `Run.progress` or the PM's own ledger from its
authenticated binding. The model cannot select a task, run, role, or revision. Each
request commits progress and its receipt together; replay returns the original receipt
without another update. The receipt includes `revision` and full `progress`, bounded
by the existing 8 KiB receipt-result limit. Worker progress appears in board cards and
`corral show`; the PM ledger appears in `corral agent state --pm` and its JSON output.
The compact host transcript shows the operation, step and resulting revision.

PM and assistant sessions keep one request plan at a time (T-679). Their contracts ask for a
phased plan on every multi-step request and an update as each step changes; with `todo` off
they say so instead. Each plan has an identity (`P1`, `P2`, ...) that the Todo widget header
shows beside its revision. `init` is refused while the current plan has open steps; once
every step is done or dropped, the next `init` keeps the old plan in history and opens the
next one. `append` with a new `phase` and a `label` opens a phase for a mid-request
correction. `{"op":"wait","step":"land","on":{"kind":"event","event":"T-12 lands"}}` or
`"on":{"kind":"owner","question":"..."}` marks a step as waiting; starting or finishing it
clears that. Updates may name `"plan":"P2"`, and one that names an older plan is refused.

The host restores the ledger at startup and resume and gives every model request a bounded
summary of the current plan (identity, revision, outstanding steps, waits and the next
action). It is built from the latest accepted ledger right before each request, so a todo
update earlier in the same turn is already reflected, and it is never written into Pi's
session file. When a turn ends with an actionable step, corral may grant one continuation
per plan revision. The host starts it only while the pane is idle, the budget holds and no
newer turn has started since the granting one, and only after re-reading the ledger and
finding the same plan revision; a turn that starts during that read, even one that is
cancelled or fails, voids the grant. A cancelled, aborted or failed turn is never
continued, and neither is a turn waiting on an event or the owner. A continued turn that
changes nothing stops continuation, and so does a run of 8; the stop is shown in the pane.
Board activity alone never starts a continuation. `corral agent state --pm` shows the
request record as `todo_requests`.

### Managed commands

With the existing `background_shell` capability, worker, PM and assistant Bash calls use one
owned execution from launch. Other roles, or generations without that capability, retain
blocking Bash. No configuration or role authority is silently expanded.

- `bash({command, wait_ms?, timeout?})` waits up to `wait_ms` (default 10000 milliseconds),
  then returns a stable `j<generation>-<n>` handle and current output **without restarting or
  killing the command**. `wait_ms: 0` backgrounds immediately. `timeout` remains a separate
  hard deadline in **seconds**, capped by `max_runtime_secs`, fixed from process launch.
- `job_output({job, tail_lines?, cursor?, max_bytes?})` is a **nonblocking peek**, even while
  running. Default: latest 50 lines within 16 KiB; maximum 64 KiB. Readers have independent
  UTF-8-aligned cursors. `cursor` cannot be combined with `tail_lines` or legacy `offset`;
  `offset`/`next_offset` remain supported. Reads never acknowledge completion. A retention
  `gap` is explicit: the capped file prefix is never misrepresented as the current tail.
- `job_wait({job?, wait_ms?})` awaits one owned job, or the next unconsumed completion among
  current eligible jobs. It returns immediately when none exist. The default wait budget is
  60000 milliseconds; expiry or wait cancellation releases observation, not the process.
- `job_kill({job})` stops the verified owned process group, retaining final output. It uses
  SIGTERM then SIGKILL after two seconds. Terminal calls are idempotent. `termination`
  distinguishes normal exit, cancellation, hard deadline and owning-host shutdown;
  `group_clear` is a local cleanup observation, not gate PASS or canonical task settlement.
- `job_start({command})` remains a compatibility alias for immediate-background Bash.
  New workflows should use Bash plus wait/peek rather than a separate launch engine.

Default nested `await tools.bash(...)` waits for actual completion before the next JavaScript
statement. Code Mode owns yielding the enclosing cell and its filtered final notification;
explicit `wait_ms: 0` requests an independent background handle. Cancelling an awaited Bash
stops its owned group; cancelling `job_wait` does not. Ordinary turn endings do not stop work.

Before executable work is released, a worker records prepare and process identity with
corral; PM/assistant record a session fact. The launcher waits on stdin until that handshake
succeeds, stays the process-group leader, reports status on a separate channel, and kills its
group if the host disappears. Commands run in the worktree with the usual Pi session identity.
Bash rules, fixed refusals, permissions, hooks and Code Mode child attribution still apply.
Tool-return hooks observe a yielded tool result, not command exit; `command_settled` is a
separate host event. Notifications never replay commands or mutating hooks.

Completion is journaled before automatic delivery. A foreground result, explicit wait/stop
or exit monitor claims the completion instead of also posting an automatic duplicate. Notices
use the existing follow-up delivery path, queue during work and wake idle sessions. Failed
submissions retry with the same identity; bind/turn end also retries pending delivery.
Already-ready completions coalesce in groups of at most four without waiting for other work;
notice excerpts are capped at 2 KiB each while full retained pages remain available through peek.
Recovery deduplicates against observed or still-queued messages, rather than assuming an old
queue acknowledgement means the model saw the result. A turn with unfinished commands holds
request continuation (`waiting_command`) without spending a no-progress nudge.

`corral_job_monitor({job,on,pattern?,timeout_secs?})` retains explicit exit and future-line
(JavaScript regex) watches, default timeout 3600 seconds. Worker monitors keep their canonical
run wait/inbox receipt and deadline semantics, including calls from journaled Code Mode;
arbitrary unjournaled nesting remains refused. PM/assistant watches are session-owned and
journaled, not fake task waits. Exit, line match, deadline or owning-host interruption wakes
once per watch. A line match is not service health; a deadline does not kill work.

The host retains at most 64 command results/process records, including running commands;
only acknowledged terminal entries can be evicted to admit another command. A full unread
completion cache refuses new work instead of silently dropping it. Per-job file storage is
capped by `max_output_bytes`, with a rolling 64 KiB in-memory tail even after that cap. Evicted
IDs report expired, never fabricated empty success. Host loss does not create daemons: the
next bind reports lost executions/monitors without replay, and retries committed pending
completion notices. Worker sync still reaps fenced/lost-generation/run-ended jobs and sweeps
unverified groups. Signalling requires the recorded leader and kernel start time; leaderless,
foreign or reused groups are never claimed.

Activity → Now has distinct **p Peek**, **b Background** and **x Stop** controls. Peek opens
current logs immediately. Background releases the current foreground observer without
changing its deadline. Stop confirms first, then uses the same owned-stop path and configured
hooks. The adapter rechecks session, generation, capability/loadout and selected job after
any dialog/hook; stale/foreign/denied selections never signal. An uncertain acknowledgement
is reported without automatic retry. There is no arbitrary PID or restart control.

Bash and jobs remain outside the in-process network guard. Interrupted Bash/start/kill calls
remain `outcome_unknown`; output/wait are `interrupted_no_effect` unless hooks may have acted.
Worker canonical records remain in `agent_host.jobs`; bridge state additionally exposes
command observation/delivery state and `job_started`, `job_exited`, `command_settled` events.
Claude receives the managed Bash schema with seconds-based timeout; legacy Claude built-in
Bash still translates its milliseconds to Pi seconds.

### Checkpoints and rewind

Worker generations with `checkpoints` snapshot the worktree twice per turn: when the turn
starts, before its prompt is recorded or any request is sent, and when it settles. A snapshot
commits every non-ignored file (staged, unstaged and untracked) over HEAD through a private
index, so the worktree's own index is untouched, and points a per-worktree ref at it
(`refs/worktree/corral/checkpoints/<run>/g<generation>-t<turn>-<start|end>`; other worktrees and
the main checkout never see it). The host then appends one `corral_checkpoint` entry to its
bound Pi session; a start entry also names the last session entry before the turn. Ignored
files (build output, `node_modules`) are not in snapshots and are never restored. A snapshot
that fails is recorded with its error and the turn goes on; it cannot be a rewind target.

`corral agent rewind <task> [--run <id>] --list` lists the run's checkpoints on its current
timeline and its rewinds. `corral agent rewind <task> --to g<n>-t<m> [--replace]` puts the
worktree and the Pi session back to the start of that turn:

1. It refuses, changing nothing, a run that is not a worker in a linked worktree
   (`not_worker`, `not_isolated`), an unknown checkpoint or one whose ref moved
   (`checkpoint_unknown`, `checkpoint_missing`), and a conflict (`rewind_conflict`). Only paths
   the agent changed in its turns since the checkpoint are restored. Changes between two turns,
   or after the last turn ended, are not the agent's: they stay, and a path changed both ways,
   or a HEAD moved outside the agent's turns, refuses the rewind with the paths named. A turn
   with no end checkpoint (its host died inside it) counts as the agent's up to the rewind.
2. It records the rewind `prepared`, then runs `corral agent resume`'s path: the old host must
   be dead or `--replace` stops it (a refusal here, such as `host_alive`, closes the record
   `aborted`), the generation is fenced with reason `rewind`, executors drain and the lost
   generation's calls are classified for the `recovery_notice` exactly as on a resume.
3. `fenced`: the fenced generation's background jobs are reaped. `saved`: the worktree as the
   fence left it is committed under `refs/worktree/corral/rewind/<run>/<rewind>`.
   `restoring`: removals run first, deepest path first, then writes, so a file and a directory
   can replace each other. Each planned path is checked before it is changed: one already
   restored is skipped, and one that changed since the save is left as it is and listed as
   `kept`, as is a directory that still holds something else. One still as saved is replaced
   without a window for an owner edit: its replacement is materialized first, then the path is
   atomically moved aside and what was moved is compared with the save. Only then is the
   replacement linked in, and never over a file that appeared meanwhile (a removal also removes
   the parent directories it leaves empty). A path edited at that moment is put back and listed
   `kept`, and every displaced byte is kept under the worktree's git directory
   (`corral-rewind/`), so nothing an owner saved is lost. The moved-aside copy uses a fixed
   hidden sibling name (`.<name>.corral-rewind-displaced`): if a restore stops between moving a
   path aside and replacing it, the next attempt (continuing or aborting) first puts that copy
   back when the path is empty, or keeps it when something is there, and then judges the path
   afresh, so the restore finishes instead of reporting its own displacement as an edit. HEAD moves back by compare-and-swap of the branch only from the saved HEAD, and the
   restored paths' index entries are reset to it. `restored`: the next generation's launch
   continues the same Pi session file from the checkpoint's entry (Pi branches there; the
   abandoned turns stay in the file); the generation it opens is recorded on the rewind before its
   host can run. `launched`: the rewind is closed with that generation. The cursor stays owed until
   the session file shows a host of that generation or a later one bound on a branch from the
   checkpoint's entry (`applied`): a relaunch whose host died first, including a plain
   `corral agent resume`, is given the same cursor again, and a rewind continued after its
   controller stopped before `launched` honors the same evidence instead of replaying the cursor.
   Each stage commits before the next starts.

An interrupted rewind leaves its stage recorded, and `corral agent resume` refuses
`rewind_pending` until it is settled: `corral agent rewind <task>` (no `--to`, or the same one)
continues from the recorded stage, and `--abort` abandons it, putting each restored file back
from the saved snapshot first. Nothing a rewind does rolls back records: receipts, turns,
questions, delivered messages, usage (read from the whole session file, abandoned turns
included) and approvals stay, and the saved snapshot keeps the pre-rewind worktree. Bash
children the dead host started that are not background jobs are not stopped, as on a resume.

Capability settings are frozen for an active generation and resolved again on explicit resume.
Ledgers and drop reasons survive host loss and resume; disabling todo does not delete
them or invalidate old receipts. Legacy generations and launch messages without a todo
setting keep the tool disabled. Todo is progress bookkeeping, never gate or acceptance
evidence and never permission to finish a Corral task.

### Headless runs

```toml
[agent.capabilities.worker]
headless = true

[agent.capabilities.reviewer]
headless = true
```

A headless worker or reviewer host does not start Pi's interactive terminal and never reads
what is typed into its pane. Everything reaches it the way an interactive host's automatic
input already does: its kickoff turn starts when it binds, and answers, gate wakes, nudges and
recovery notices arrive from its durable inbox through the authenticated bridge when `corral
sync` delivers them. Its `corral_*` calls are receipted exactly as before, and its questions go
to the board, `corral question`, the PM wake and Needs me as usual. The pane shows only a plain
log: the headless notice, each turn's start and end, denied tools and refused network calls.

The host still runs in the run's herdr pane, which remains the process container: pane loss,
the launch-exit record, `corral agent resume` into the same pane, and closing the run work
unchanged. `corral agent state` shows `mode: headless` (JSON `mode`), and the bridge `get_state`
result reports `mode: "headless"` with `interactive: false`. Because each generation resolves
its settings anew, an explicit resume keeps the run's container, pane and bound session and
stays headless while the setting is on; turning it off makes the next generation interactive
and records the change. The setting is read at dispatch and resume, so it applies to hosts
launched after the change.

`[agent] ui` sets how the run pane looks (default `"rich"`). Rich mode now starts the real
Pi interactive host in fullscreen with a graphite session sidebar and filled composer.
At fewer than 105 columns the sidebar relocates into a native inspector above the composer;
the real transcript, editor, bindings, model controls and permission dialogs remain Pi-owned.
`F6` focuses the inspector, arrows navigate, Enter inspects, Escape returns to composing, and
`Ctrl+B` switches wide sidebar/inline modes or compacts narrow details. `/panels context`,
`/panels files`, `/panels agents`, `/panels plan` and `/panels refresh` expose the same real
state and bounded read-only Git inspection. See [native panels](pi-panels.md) for scope and checks.
There is no browser or local web server in the production UI. The pane retains its
two-line header (role, task and run, generation, model, effort and worktree), names the
terminal tab after the task and role (`T-503 worker`), and adds footer status for the corral
binding (a filled dot with the generation, such as `● g2`, or the footer text “○ corral unbound”
when unbound) and, once one happens, a count of refused network calls and denied tools.
Each `corral_*` call is one line: its short name and the argument that says what it does
(for an ask, its kind and question, displayed as “corral ask clarification: ...”),
followed by `✓ key=Q-3` or `✗ rejected  <code>: <reason>`. Expanding tool output
shows the full receipt. `ui = "plain"` keeps stock Pi. The setting is read at dispatch, so it
applies to hosts launched after the change.

## Installing the host

A release install (`install.sh`, v0.4.0 and later) puts `corral-pi-host` beside `corral` and
runs `corral agent install`, which then installs that compiled host. It is one executable
built by `scripts/build-pi-host.sh` that embeds Bun, every dependency from the frozen lock and
the role, delivery and auth modules, and never autoloads `.env` or `bunfig.toml`.
`--from <path>` also accepts a `corral-pi-host` executable. The installation is
`assets/<digest>/` holding that executable plus the `compat.json` and role files it writes
(`corral-pi-host --write-install`); the digest covers all of them, so a replaced executable
is refused like any other changed asset. A compiled installation needs no Bun: dispatch,
login (`corral-pi-host login`), model listing (`corral-pi-host models`, which feeds
`corral harness list` and the settings model picker) and recovery run the executable directly, and `corral agent
doctor` reports its Bun as embedded. Anthropic profiles in a compiled host use the `claude`
CLI on PATH, because the per-platform Claude Code binary the SDK ships is not bundled.

When its build checkout is present, `corral agent install` rebuilds the compiled host there,
checks its HEAD against the binary's build commit, and replaces the sibling host. Bun is required.
`--no-build` uses the sibling first; `--from <dir>` explicitly installs source from a package.
Source installation refuses a fixture
installation (`fixture_assets_not_allowed`) and a Bun outside the package's `compat.json`
range (`bun_unavailable`, `bun_version_unsupported`), then copies only what a host runs:
`package.json`, `bun.lock`, `compat.json`, `src/` (without `*.test.ts`), `assets/` and dependency
`patches/`, never `test/` or `fixture.json`. It runs `bun install --frozen-lockfile` in that copy
and stores it as `$XDG_DATA_HOME/corral/corral-pi/assets/<digest>/` (`~/.local/share/corral/corral-pi/...`
without an absolute `XDG_DATA_HOME`), recording the digest in `installed.json` beside it.
Reinstalling an unchanged package reuses the same directory.

Dispatch uses that installation whenever `[agent] assets_dir` is unset, so runs never reference
the checkout it came from. If the installed files change afterwards, dispatch refuses with
`installed_assets_changed` until you reinstall. The installation carries the worker, reviewer
and PM roles. Reinstall to include a newly added role in an older managed installation.

## Credentials

`corral agent login` runs interactively, in your own pane, once per provider you use. On a
terminal it first shows a picker:

```
Log in a model provider for corral-pi
> OpenAI Codex (ChatGPT Plus/Pro)   [logged in]
  Anthropic Claude (Pro/Max)        [not logged in]
↑/↓ move · Enter select · q cancel
```

The arrow keys or j/k move, Enter selects, and q or Esc cancels with exit status 130 and writes
nothing. Each row's state comes from the credential file's metadata; its contents are never read.
`corral agent login --provider openai-codex` or `--provider anthropic` skips the picker. Without
a terminal on stdin, `corral agent login` without `--provider` exits with a usage error instead
of prompting.

The selected provider's Pi subscription OAuth flow then runs from the installed
`src/auth/login.ts`: for OpenAI Codex (ChatGPT Plus/Pro) a browser or device-code login, for
Anthropic Claude (Pro/Max) a browser login or a copy-code login for a headless pane. Each
provider's credential is its own file in `$XDG_DATA_HOME/corral/corral-pi/auth/`:
`openai-codex.json` or `anthropic.json`. The directory is 0700 and every file 0600. Logging in
again replaces that provider's file atomically. A Codex login made before per-provider files keeps
working without a new login: it stays in the shared `auth.json` until its first refresh moves it
to `openai-codex.json`. During login the network is limited to POST requests to that provider's
OAuth endpoints: OpenAI's token and device-code endpoints, or Anthropic's
`https://platform.claude.com/v1/oauth/token` (the Anthropic browser callback is a loopback
listener). A fixture installation is refused, because its credential is synthetic. A failed
login prints only the failing step and an HTTP status, never the provider's response, so no
token reaches the terminal or a log.

A running host loads the credential of its profile's vendor (`openai` uses `openai-codex`,
`anthropic` uses `anthropic`) through the installed `src/auth/index.ts` and lets Pi refresh the
token in place. Every write holds an exclusive kernel lock (`flock`) on `auth.json.lock`, and the
kernel releases it if a host dies, so concurrent hosts never double-refresh and no stale lock
needs recovery. Without a login for that provider, dispatch from the managed installation
refuses `credentials_unavailable` naming the provider, and the host stops with
`credentials_unavailable`; both name the command to run, for example
`corral agent login --provider anthropic`. For these subscription providers only OAuth credentials are stored. There is no
API-key or other paid fallback for them: the host drops Pi's Anthropic API-key handler and clears every
`ANTHROPIC_*` variable (API key, auth and OAuth tokens, workload federation) before Pi loads, so
after a `/logout` or a removed credential the provider has no auth rather than a paid key, and
any Anthropic request without the subscription OAuth token is refused before it is sent. corral
never reads or writes `~/.codex`, `~/.pi`, `~/.claude` or another harness's store.

Assistant and PM sessions launched on a subscription provider also discover the other usable
subscription login at startup. `/model` lists both OpenAI Codex and Anthropic models when both
are logged in; selecting a model infers its provider without replacing the session, history or
corral identity. Model switches refresh credentials and check compatibility before committing;
a missing login or incompatible model leaves the current selection unchanged. Each route keeps
its own guarded transport (Claude model traffic still belongs only to its subprocess lane).
Workers, reviewers, Bedrock and local-inference sessions keep their launch provider.
Corral records the active provider and model on each turn, not just on the initial launch.

For assistant/PM launches, `corral pi --provider anthropic` retains an already-compatible profile
model or defaults to `claude-opus-5-5`; `corral pi --provider openai-codex` similarly uses
`gpt-6.1-sol`. An explicitly incompatible `--provider`/`--model` pair is refused before launch,
with a corrected command. A model-only override infers the provider for known model families.

Anthropic profiles run on your Claude Pro/Max subscription through the official Claude Agent SDK
(`@anthropic-ai/claude-agent-sdk`, pinned in `package.json` and `compat.json`). Pi keeps the
conversation and runs every tool; each model call starts one SDK `query()`, which spawns the real
Claude Code engine from the SDK's platform package (for example
`@anthropic-ai/claude-agent-sdk-linux-x64`). Claude Code keeps its own `claude_code` system prompt,
with only corral's role contract and working directory appended (Pi's harness sections are left
out, since Anthropic bills a request that presents a third-party harness as extra usage). It
receives the conversation as one user message and the corral login's access token as `CLAUDE_CODE_OAUTH_TOKEN` in its environment, and
nothing else: every `ANTHROPIC_*` and other `CLAUDE_CODE_*` variable is removed, and its config
directory is a private `claude-code/` directory under the host's runtime directory, so it never
reads `~/.claude`. Claude Code only plans: its tool calls stream back to Pi, while a
`PreToolUse` hook and the permission callback deny every execution inside Claude Code. Pi's
`read`, `write`, `edit`, `bash`, `grep` and `find` appear to Claude Code as its own Read, Write,
Edit, Bash, Grep and Glob; the role's other tools (`corral_*`, the LSP tools) appear through an
in-process MCP server whose handler refuses. Cancelling a turn interrupts and closes the query. A
missing platform package is refused `claude_code_unavailable`, naming the package to install;
nothing is downloaded. Each model call is independent: there is no resident Claude Code session,
session reuse or Claude Code compaction. The policy record is
`docs/records/pi-native-claude-route.md`.

## Amazon Bedrock (metered opt-in)

Bedrock traffic is metered per token to your AWS account. Opt in explicitly; there is no
per-run or per-task spending cap. Automatic availability fallback still has its existing
spending bound and a Bedrock candidate must declare metered cost.

```toml
[profiles.bedrock]
harness = "corral-pi"
vendor = "amazon-bedrock"
model = "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
```

Choose a bare Pi catalog ID, not a model or application-inference-profile ARN. The model
listing and settings picker include regional and cross-region IDs such as `us.`, `eu.`,
`apac.` and `global.`. Bind a named AWS profile and region before launching:

```sh
corral agent login --provider amazon-bedrock --aws-profile dev --region us-west-2
corral agent doctor
```

On a terminal login can prompt for the profile and region, defaulting the region to the
profile's configured region. Without a terminal supply both flags. The binding contains only
the profile name and region, not secrets. A changed binding applies to newly launched hosts.
Each worker, planner, reviewer and PM launch emits one `metered` notice before its host can
make a paid call; repeated readiness checks do not emit notices.

Credentials come only from the named profile in `~/.aws/config` and `~/.aws/credentials`:

- static `aws_access_key_id` and `aws_secret_access_key`, optionally `aws_session_token`;
- `role_arn` with `source_profile`, up to four role steps without cycles, rooted in static keys
  or SSO, optionally using `external_id`, `role_session_name` and `duration_seconds`;
- IAM Identity Center (SSO), with `sso_session` and its `[sso-session]` section, or legacy
  `sso_start_url` and `sso_region`, plus `sso_account_id` and `sso_role_name`.

`credential_process`, `web_identity_token_file`, `credential_source`, `mfa_serial` and
`login_session` are refused. Ambient `AWS_*` variables, Bedrock API keys, ECS and EC2
credentials are not used; Bedrock hosts and their subprocesses have `AWS_*` removed. corral
never writes `~/.aws`, including the SSO cache, and never refreshes or logs in to SSO. Run
`aws sso login --profile dev` yourself for a missing or expired token. Already obtained role
credentials remain usable until their own expiry if renewal fails; fresh credentials then
require a valid cached token.

The guard permits only these destinations for the bound profile:

- POST `https://bedrock-runtime.<region>.amazonaws.com/model/<URL-encoded model>/converse-stream`,
  with AWS SigV4 authorization;
- POST `https://sts.amazonaws.com/` or `https://sts.<region>.amazonaws.com/` only for a
  profile chain with a role;
- GET `https://portal.sso.<sso_region>.amazonaws.com/federation/credentials` only for SSO.

OIDC token endpoints, other regions, models and methods, FIPS, dual-stack, VPC and custom
endpoints, GovCloud and China endpoints are not allowed. Raw TCP Node connections are refused;
Unix sockets remain available. See [the Bedrock route record](../records/corral-pi-bedrock-route.md).
For a live check with AWS access, run `scripts/bedrock-smoke.sh --aws-profile dev --region
us-west-2 --model us.anthropic.claude-sonnet-4-5-20250929-v1:0`; it refuses to send without
the --yes-metered opt-in and prints the intended endpoint, model and token cap first.

The routing vendor selects the provider, credentials, guard and pricing; reviewer independence
uses the model family. Bedrock Claude and Anthropic Claude both belong to `anthropic`, so they
are not independent under `require_cross_family`. Bedrock Llama belongs to `meta`; for other
vendors the family is the vendor. An unknown Bedrock family fails closed when cross-family
review is required. `routing.reviewer_by_vendor` retains its name but its keys select families.

## Readback

`corral agent doctor [--json]` reports, and `corral doctor` adds when a corral-pi profile is
configured:

| Check | Reports |
|---|---|
| `corral_pi_bun` | The Bun on PATH, its version and whether it is inside the GO range. |
| `corral_pi` | The asset directory and its source (installed or `[agent] assets_dir`), its digest, the GO verdict, the auth module (`corral-pi`, `fixture` or `missing`), the installed roles and each role's readiness with the refusal it would get. |
| `corral_pi_credentials` | Provider login states and file permissions; Bedrock reports its bound profile, region, mechanism and metered status without secrets. |
| `corral_pi_bedrock` | Offline readiness of the binding and named profile, including SSO token expiry or a fix hint; shown for a binding or Bedrock profile. |
| `corral_pi_question_wake` | Where a new question's wake goes: the bound corral-pi PM's inbox, the OmO PM push (when `pm.push_kinds` includes `question_opened`), or nowhere, in which case a waiting question reaches Blocked after `question_unattended_secs`. |

`corral agent state <task> [--run <id>] [--review <id>] [--json]` (or `--pm`) shows one
container's binding: harness, role, mode (`interactive` or `headless`), host health, current
generation, generation, receipt and turn counts, open question keys, and the same installation
summary.

## herdr status

Every bound host, whatever its role, reports itself to the herdr server of its pane as the agent
`corral-pi`, so herdr's sidebar, `herdr agent list` and `herdr agent wait` show it without reading
the screen. One adapter in the host does this for all six roles. It reports only from inside herdr
and only for the pane corral bound it to (`HERDR_ENV=1` and `HERDR_PANE_ID` equal to the bound
pane), by running `$HERDR_BIN_PATH` with arguments, never a shell. A headless host and plain
`corral pi --bare` report nothing.

| herdr field | What the host reports |
|---|---|
| `agent` | `corral-pi`, from the reporting source `custom:corral-pi` |
| `display_agent` | `corral-pi <role> <identity> g<generation>`, for example `corral-pi worker T-12/R1 g2`, `corral-pi reviewer T-12/R1 review1 g1`, `corral-pi pm app g1` or `corral-pi assistant A-3 g1` |
| `tokens` | `corral_role`, `corral_generation`, `corral_session` (the bound Pi session ID) and whichever of `corral_project`, `corral_task`, `corral_run`, `corral_review` and `corral_assistant` the binding has |
| `agent_status` | `working`, `idle` or `blocked`, below |

Each state report also gives herdr the bound session's ID and file as the agent's session
reference, never a session guessed from the provider. herdr 0.9.5 keeps but does not
display a custom source's session reference, so the `corral_session` token is the readable one.
The host reports no resume command: after a herdr restart, recovery stays with
`corral agent resume`.

| State | When |
|---|---|
| `working` | A turn is running. |
| `idle` | The turn settled and no question holds the host. herdr shows `done` until the pane is looked at. |
| `blocked` | The host has a pending question that is blocking or goes to the owner (`owner_only`, `permission`, or escalated). The report's message names it. |

Only corral decides when a question opens or settles: a committed `corral_ask` receipt opens it,
and the inbox delivering its answer or correction, or a committed `corral_withdraw` receipt,
settles it. A turn that ends, fails at the provider or is cancelled, and a nested tool call, never
clear `blocked`, and a settled turn never stays `working`. A resumed host starts from the pending
questions its hello receipt names. When the PM or owner escalates, cancels or answers a question,
corral sends the asking host, over its authenticated bridge (`questions`), the state of every
pending question of the container and of the one that changed, however old, with no message to
the model and no new turn: an escalation blocks, a cancellation releases only that question. Each question keeps its newest revision, so a late or repeated
update cannot undo a newer fact. Should that push fail, sync's next probe of the host sends the
pending questions and every question the host still holds, until it shows the blocking questions
corral holds.

One report is in flight at a time, and changes that arrive meanwhile are sent once, as the latest
state; an unchanged state sends nothing. Sequence numbers fence generations: at hello corral
allocates the binding an epoch that is at least its hello time in milliseconds and above every
epoch it allocated before for this user (the high-water mark is kept in the runtime directory all
of the user's stores share), records it with the generation, and names it in the hello receipt
(`herdr_seq_epoch_ms`). The host numbers its reports from that epoch shifted left 20 bits, and
herdr ignores a report that is not above the last one it accepted from `custom:corral-pi` for the
pane, so a later binding in the same pane reports above an earlier one even after the wall clock
stepped back or within one millisecond. If no epoch can be allocated, the host reports nothing.

- **Graceful exit.** When the owner quits or corral shuts the host down over the bridge, the host
  releases the pane (`pane release-agent`) and clears its display name and tokens, so the shell
  that returns is not shown as `corral-pi`.
- **Host loss.** Sync probes the bound host of every role (worker, reviewer, planner, plan
  reviewer, PM and assistant). When it records `agent_lost` on that container, corral releases
  the generation's report and clears its metadata with the last number of the generation's range,
  so no stale `working` or `blocked` state stays. herdr itself also drops an agent whose pane is
  back at its shell prompt.
- **Resume and replacement.** The next generation names itself and its state as soon as hello
  binds it, with numbers above every number of the generation before. A late report or cleanup
  from the older generation is ignored and cannot overwrite or clear it.

Neither path closes the pane or types into it. herdr status is display only: corral never takes it
as evidence that a gate passed, a question was answered or a run completed. A report herdr refuses
is recorded as a `herdr_report_failed` host event and in the `herdr` section of the host's bridge
`get_state` result, and changes nothing else; `herdr_reporting_off` names why a host does not
report at all.

## What is checked before a run starts

Dispatch, `corral review-run` and opening the PM check a corral-pi profile before any worktree,
pane or prompt exists, and refuse with a stable code naming what is missing:

| Code | Meaning |
|---|---|
| `vendor_refused` | The profile's vendor is not on the corral-pi allowlist (`openai`, `anthropic`, `amazon-bedrock`, `llama.cpp`). |
| `metered_vendor_not_explicit` | A metered vendor was inferred rather than explicitly set on the profile. Set `vendor = "amazon-bedrock"`. |
| `provider_unsupported_by_host` | The vendor is on the allowlist but the installed host's `compat.json` `providers` list does not include it. A `compat.json` without `providers` declares `openai` only. |
| `assets_unavailable` | No asset installation: `[agent] assets_dir` is unset and `corral agent install` has not run, or the directory does not exist. |
| `fixture_assets_not_allowed` | A test fixture installation (it carries `fixture.json`) was reached other than through `[agent] assets_dir`. |
| `compat_unavailable` | The assets have no `compat.json` recording a `GO` verdict. |
| `role_unavailable <role>` | The assets lack `src/roles/<role>/index.ts`; the reason names the stage that adds it. |
| `delivery_unavailable` | The assets lack `src/delivery/index.ts`. |
| `credentials_unavailable` | Auth is missing or the provider is not ready. For Bedrock this includes a missing binding or AWS profile, unsupported mechanism, or missing or expired SSO token; the reason names a fix. |

Until every stage has landed, a corral-pi dispatch therefore stops at one of these refusals.
`corral doctor` reports the same readback for each role when a corral-pi profile is configured,
and says nothing about Pi otherwise.

## How a worker host starts

When a corral-pi worker run reaches its pane, dispatch opens the run's next host generation and
writes a one-shot bootstrap file, mode 0600, into that generation's runtime directory, mode 0700:
`$XDG_RUNTIME_DIR/corral/<store>/<container>/g<n>/`, or `/tmp/corral-<uid>/...` without
`XDG_RUNTIME_DIR`. The pane then runs `corral agent launch --bootstrap <path>`, plus
`--assets <dir>` when `[agent] assets_dir` is set. A reviewer host launches the same way from
`corral review-run` (see The reviewer role); the PM host launches from `corral pm`.

`corral agent launch` reads and deletes the bootstrap, then checks, in order, and refuses before
any secret exists or anything is started:

| Code | Meaning |
|---|---|
| `bootstrap_unavailable`, `bootstrap_unsafe`, `bootstrap_invalid` | The bootstrap is missing (it is one-shot), is not a 0600 file of yours inside its own generation directory, or is unreadable. |
| `assets_unavailable` | The asset directory is gone. |
| `fixture_assets_not_allowed` | The assets carry `fixture.json` but are not the directory `[agent] assets_dir` names. |
| `compat_unavailable` | No `compat.json` with a `GO` verdict and a Bun range. |
| `asset_digest_changed` | The assets changed since dispatch (the digest covers `package.json`, `bun.lock`, `compat.json`, `fixture.json`, `src/` and `assets/`). |
| `bun_unavailable`, `bun_version_unsupported` | No `bun` on the pane's PATH, or one outside the manifest's tested range. |
| `role_unavailable`, `delivery_unavailable`, `credentials_unavailable` | A module is missing; the reason lists every missing module with the stage that supplies it. |

A refused launch returns to the pane's shell. The next sync ends the run as failed at launch,
attaches the pane's last lines as an `Agent launch failure` log and frees its slot.

Otherwise the launcher generates the generation's two secrets, writes the control credential
`ctl.cap` (0600) and replaces itself with `bun --no-env-file <assets>/src/host/main.ts
--launch-fd <n>`. The secrets and the bind nonce reach the host only on that inherited pipe,
never on a command line, in the environment or in `.corral/`. The host refuses a Bun, lock or Pi
release other than the ones `compat.json` pins, loads the role, delivery and auth modules from
their fixed paths, starts its bridge socket and binds the generation by calling
`corral agent call --op hello`. Then it hands the pane to Pi's own interactive terminal, where
you can watch and type, unless the generation is headless (see Headless runs).

### What the host leaves out

Only the role's tools are active; any other tool call is refused. The host loads no extension
but corral's own, and no MCP or themes. Skills, prompt templates and context files are exactly
the ones [Guidance and session export](#guidance-and-session-export) selects from the
generation's snapshot, plus the skill the role ships inside the installation (the worker's
corral-worker skill); under strict only that skill loads, and nothing is ever read from `~/.pi`,
a project's `.pi` or another harness. Install telemetry, analytics, the cache warmer and the remote model catalog are
off, except explicitly configured local model discovery. Its network is a guarded fetch that sends only POST requests to the bound profile's provider
API and that provider's token-refresh endpoint: for `openai`,
`https://chatgpt.com/backend-api/codex/responses` and `https://auth.openai.com/oauth/token`; for
`anthropic`, only `https://platform.claude.com/v1/oauth/token`, since the host process never
calls `https://api.anthropic.com` itself. Claude Code's own traffic (model requests, and
whatever else it sends; its non-essential traffic is switched off with
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`) leaves from its subprocess, outside this guarded
fetch. Every other request, the other provider's endpoints and bug-report uploads included, is
refused. Bedrock and local inference use their separate configured rules described above. The same holds after a reload and
after the session is replaced.

### Model catalog

Cloud launch and model listing share a corral-owned catalog, including models added in
`$XDG_DATA_HOME/corral/corral-pi/models.json`, or
`~/.local/share/corral/corral-pi/models.json` when `XDG_DATA_HOME` is unset or relative.
Ambient Pi configuration (`~/.pi/agent` and `PI_CODING_AGENT_DIR`) is not loaded.
Add a model under an existing subscription provider with that provider's model API;
no corral source change is needed. Pi's model refresh reloads this file offline.
Cloud remote catalog requests remain disabled and model endpoint restrictions remain in force.
The local inference catalog instead uses its explicitly configured endpoint as described above.
Metadata uses an in-memory catalog store, not an ambient `models-store.json`.
Invalid JSON or model configuration fails startup with `model_catalog_invalid`
and the SDK's diagnostic, including the file path. A failed refresh retains the
last valid catalog and reports the configuration error.

The catalog exposes only the providers approved for the current host. Configuration
can add or change model metadata, but cannot enable another provider, API-key auth,
custom auth headers or an alternate streaming implementation. Parent-installed
subscription and guarded-stream restrictions survive model refresh.

### The bridge

corral talks to a live host over its generation's Unix socket. Any corral process can connect
through `ctl.cap`, which it must send with every request; the host also refuses a peer running
as another user. The commands are `hello`, `get_state`, `submit_input`, `cancel`,
`events_since` and `shutdown`. Input is answered truthfully: `started` (with the turn it
started), `queued` (busy, delivered as a steer or follow-up), `handled` (a command that needs
no model run) or `rejected` (for example, busy input whose policy is `prompt`). When Pi has not
answered within 30 seconds, the reply is `pending`: the input stays admitted, its outcome follows
as an `input_settled` event, and a replay of the same key returns it. An answer never claims the
turn finished.

Input is idempotent by its key for the generation's whole life: a replay returns the first
answer and a changed payload under the same key is rejected. A generation admits 4,096 keys,
then refuses new ones with `input_limit` while replays still answer. Every request and reply is
one line of at most 64 KiB, framed on bytes, so multibyte text split across socket reads arrives
intact. A reply that would exceed the bound is refused as `response_too_large` and the
connection stays usable.

Each host event carries the generation and a monotonic sequence. `events_since` returns at most
one frame of events; continue from its `next` cursor while `more` is true. A reader whose cursor
fell behind the host's buffer gets an explicit gap, which corral records as an
`agent_events_gap` event, a requirement to reconcile from its own records.

### Fixture installations

For development and tests, `bun packages/corral-pi/test/support/fixture-install.ts <dir>` builds
a temporary asset directory with test-only role, delivery and auth modules. They use a synthetic
credential and a fake provider transport, so no provider is ever contacted. Point
`[agent] assets_dir` at the printed directory. `scripts/pi-lane.sh` runs the package's own tests
and typecheck on Bun, with `node`, `npm` and `npx` disabled; its end-to-end dispatch test runs
only inside the private server of `scripts/test-herdr.sh`.

## The worker role

A corral-pi worker run gets Pi's read, edit, write, bash, grep, find and ls tools, plus nine
corral tools. Each corral tool call is one authenticated `corral agent call` whose request ID
comes from the Pi session and the tool call, so a retried call returns its stored receipt instead
of running twice:

| Tool | corral operation |
|---|---|
| `corral_ask`, `corral_withdraw` | open or withdraw a question on the run |
| `corral_wait` | declare a wait on an external event; it needs a deadline |
| `corral_progress`, `corral_checkpoint` | write the run's progress ledger or a resume checkpoint |
| `corral_evidence`, `corral_artifact` | record criterion evidence or attach a file |
| `corral_gate` | start the run's one receipted gate |
| `corral_done` | report the run complete, exactly as `corral done` does |

Corral, not the model, decides what a role may call: the worker's contract
(`assets/worker-contract.md`) and the installed corral-worker skill are appended to its prompt,
and every request is checked against the bound run on the Rust side.

**First turn.** A fresh worker session starts its first turn itself, once Pi binds the role's
extension, which happens after the host's hello: the message names the run's prompt file and
says to call the matching corral tool wherever the prompt names a corral command that changes
state. The prompt's Gate contract section, which dispatch renders from the run's gate contract,
is the worker's gate; its SHA-256 is the `contract_sha256` the worker reports. A reloaded or
resumed session never repeats it.

**Gate.** `corral_gate` answers `started` with the attempt, `rejected` (`submission_refused`
with the reason, for example when the run was fenced before submission) or `uncertain` when
corral cannot tell whether the gate's pane command ran. `started` and `uncertain` keep a wake
armed; `rejected` arms none, so the worker fixes the reason and calls `corral_gate` again. When the
gate finishes, sync posts exactly one `wait_wake` message keyed
`wake:<task>/<run>/<attempt>/<state>`, carrying the run's `gate_ref` and `contract_sha256`, which
the worker passes to `corral_done`. Every wake and nudge is an ordinary message in the run's
inbox. A start that stays uncertain for
`[agent] gate_start_uncertain_secs` wakes the worker once with `gate_start_uncertain` instead.
Wakes survive a corral restart: they are computed from the stored records on every sync.

**Waits.** `corral_wait` registers the wait and its wake subscription together. For a task, use
`task_id` and `task_status` (`done` or `dropped`): sync detects the status event and posts the wake.
A free-text `channel_id` wait requires an explicit receipt; only its deadline is automatic, and a
pending manual wait appears in PM attention after 30 minutes. The tool requires a deadline.
When the wait leaves `pending`, sync posts one `wait_wake` for it.

**Questions.** `corral_ask` returns at once, blocking or not, and the worker may keep working.
A `questions` batch (1-4 items sharing `blocking`, `context_refs` and `needed_by`) opens as one
group keyed by its first member: the PM gets one wake, the group counts once against the blocking
limit, and `corral question answer-group <key>` answers its members, each at its own revision;
the task reaches Blocked only when a question is escalated or becomes unattended. While any of
the run's questions is pending, `corral_done` is refused `question_state` with the question keys,
and the run stays live.

**Turns.** The host reports each turn's start and end. corral classifies a turn in the same
transaction that records its end, from its own records at that moment, never from the screen, so
a question or wake that comes later never changes it: `completed` (a `corral_done` committed in
that turn), `cancelled` (a turn `corral agent cancel` stopped), `provider_failed`,
`owner_engaged` (you typed the turn's input in the pane, or typed
into the pane while the turn ran, which upstream Pi steers into that turn), `awaiting_answer` (a
question of the run was pending), `waiting_external` (a gate or wait would wake the worker) or
`no_report`. The latest `no_report` turn of a host generation gets one automatic `turn_nudge`
message; a later one puts the task in Blocked with `turn_ended_without_report`, which clears when
another turn starts. The nudge is tied to the turn that spent it, an earlier report never counts
for a later turn, and a nudge spent before a change request is not given back. Turns you typed in
are never nudged.

**Vendors.** The allowlist admits `openai`, `anthropic`, explicit `amazon-bedrock` and configured `llama.cpp`; a worker profile with any other vendor
is refused `vendor_refused` before a worktree exists. A vendor also has to be one the installed
host declares in `compat.json` `providers`; the shipped host declares `openai` and
`anthropic`, `amazon-bedrock` and `llama.cpp`, and an older installation without the list is refused
`provider_unsupported_by_host` for Anthropic until you reinstall. Setup discovery lists only the
vendors the installed host declares, so it does not offer an Anthropic profile the host cannot
run. Anthropic profiles are opt-in and use subscription OAuth held only in the corral-owned Pi
auth directory (`corral agent login --provider anthropic`); see
`docs/records/pi-native-claude-route.md` for the evidence and the accepted policy risk.

## The reviewer role

`corral review-run <task> <profile>` with a corral-pi reviewer profile opens the review pane
first, then the review child's next host generation in that pane, and runs
`corral agent launch --bootstrap <path>` there. The bootstrap, not the model, binds the parent
run, the review child and the review's callback token, so every request is checked against the
attempt this host was launched for.

The reviewer gets Pi's read, grep, find, ls and bash tools and no edit or write tool, plus five
corral tools:

| Tool | corral operation |
|---|---|
| `corral_ask`, `corral_withdraw` | open or withdraw a question on the review child |
| `corral_evidence`, `corral_artifact` | record an observation against one of the task's criteria, or attach a file, as review proof |
| `corral_verdict` | record the verdict, exactly as the prompt's `corral verdict --attempt` does |

A reviewer's evidence and artifacts are reviewer-side proof. Each is attached to the parent run
from the review child's staging directory under `.corral/state/review-watch/`, which the review
packet excludes, and an observation is kept as an evidence artifact rather than appended to the
worker's criterion evidence. Recording proof therefore never changes the candidate the review is
bound to, and the reviewer's own verdict still commits. A worker-side artifact or evidence added
after the review started still makes the verdict `review_closed`.

Its contract (`assets/reviewer-contract.md`) is appended to its prompt. A fresh reviewer session
starts its first turn itself with the review prompt file. That prompt's Gate contract (review,
authoritative) section, rendered from the run's gate contract, is the tier the candidate is
judged against; its SHA-256 is recorded as the verdict's `contract_sha256`.

**Verdict.** `corral_verdict` commits the verdict, its notes artifact, the VerdictReport and the
receipt in one update on the review child. A retried call returns the stored receipt, also after
a newer review child started. The verdict closes the child: the turn that recorded it may still
end, then the generation is fenced, and any new request is refused `review_closed`. A verdict the
existing verdict binding refuses is answered as a rejection and recorded as an
`agent_request_rejected` event: `review_closed` when the candidate or the review changed since it
started (`stale review: ...`), and `question_state` while a question of the child is pending or an
answer message is unread. A reviewer that leaves the worktree dirty has its verdict recorded as an
escalation, as for any reviewer.

**Questions.** A reviewer's questions live on its review child, under keys
`<task>/<run>/<review>/Q<n>`. When the child closes without a verdict (a new candidate, a newer
review child, an interrupted review or an ended run), sync cancels its questions with that reason
(`review_superseded`, `review_interrupted` or `run_ended`) and its undelivered messages
`recipient_closed`, and the existing review-run path closes its pane. A later answer for one of
those keys is refused `question_closed`, so an answer never reaches another attempt.

**Turns.** A reviewer turn is classified like a worker's, with `completed` meaning a
`corral_verdict` committed in that turn and `no_verdict` in place of `no_report`. The latest
`no_verdict` turn of a host generation gets one `turn_nudge`; a later one escalates the review to
Blocked at once with `turn_ended_without_report`, as the idle watchdog would after
`review_idle_after_secs`. A healthy reviewer host is never judged by its pane's idle status.

## The PM role

OmO remains the default PM; explicitly select a `corral-pi` profile as `pm.agent` to opt in.
Opening a Pi PM requires an approved operating scope selecting one or more projects. Its
`AgentPm` binding is workspace-level, separate from task runs, with null task and run and an
empty project identity. Mutations check that binding and the current approved project set again
at commit. Adding or dropping approved projects does not change the PM session on resume.
An empty or unapproved scope refuses with the `corral mode autopilot --project <project>`
preview command; review the preview and repeat it with `--confirm <digest>`.

The PM gets typed read-only tools for brief, next, show, list, search, route, artifacts and
lessons, plus receipted question answer, escalate and cancel, reply, comment and capture.
There is no shell, edit, MCP, skills, `pm act`, owner answer, deadline or acceptance tool.
Reload and session replacement retain this loadout. PM answers to any kind of pending question
need a cited basis within the approved write scope and acceptance, and the owner may supersede
any PM answer.

PM tools never run sync or `pm_workset`. A separate board/sync loop is required for wakes:
keep the board running or run `corral sync --watch` outside the PM. A healthy scoped Pi PM
receives each pending question wake through the Pm inbox. Its subject retains the outbox
dedupe key `qwake:<question>:<revision>`. Sync acknowledges the question's outbox only after
publication; if corral crashes between publication and acknowledgement, the next pass finds
the same logical message instead of posting another. The Pi inbox adapter then submits it
to the host, starting an idle turn. Enqueueing alone is not delivery, and observation still
requires the host's `corral_delivery` entry.

PM replacement and the current approved project set are rechecked before publication and at the
delivery boundary, so wakes for projects removed from scope are not delivered. OmO question push
keeps its existing opt-in configuration; without triage, waiting questions still reach
Blocked after `question_unattended_secs`. See [the PM loop](pm.md) for authority and handover.

## The planner and plan reviewer roles

A `corral-pi` planning profile runs the planner of a [plan run](plans.md) and its independent
plan review in their own planning containers (`<task>/plan-D1`, `<task>/plan-P1/PR1`), never
as a worker or the PM. Both roles have upstream read, grep, find and ls only: no shell, edit
or write tool, so the source tree stays read-only even beyond the planner's read-only sandbox.

- The planner has corral_ask, corral_withdraw, corral_plan_write and corral_plan_submit.
  corral_plan_write writes the run's `plan.md`, the path its prompt names; the model never
  chooses the path. Content arrives in chunks of at most 16 KiB: offset 0 starts the file
  over, and each later chunk continues at the `bytes` the previous receipt returned, up to
  1 MiB. corral_plan_submit runs the same submission as `corral plan submit`; a refused plan
  settles with its reason, and the planner fixes the file and submits again.
- The plan reviewer has corral_ask, corral_withdraw, corral_plan_write (its notes report,
  the path the review prompt names) and corral_plan_verdict. Inline verdict notes are recorded on
  the review; without them the written report is the notes. Only corral_plan_write changes the
  report file. A verdict always carries the launcher-bound plan and attempt, as `corral plan verdict --attempt` does, so it is never an
  owner verdict: an approval at most awaits owner acceptance.

Both planning roles, workers and code reviewers also have `corral_content_read`. It reads
same-project task specs and manifests (`T-123`), immutable plan snapshots (`T-123@P1`), and
registered artifacts (`artifacts/T-123/R1/file.md` or `T-123:R1:file.md`) from storage, without
requiring a shell or a restored `.corral` path. Task manifests include review notes and prior
plan/artifact refs. Text pages are at most 4096 UTF-8 bytes (smaller when JSON escaping needs
more receipt space); continue at `next_offset` until null. Binary artifacts use the viewer.
Submitted plan bytes resolve by recorded hash; source and snapshot files are disposable
caches, not freshness authorities. Prompts that still need a regular plan file receive a
decoded cache copy resolved through artifact storage.

Each host sends its dispatch prompt once per planning request. A revision round rewrites
the prompt with its required changes and is sent again; relaunching the same request is not.
Plan submission and verdicts reserve their receipt before the plan service commits, because
that service then closes the planning pane. The service's own final transaction rechecks the
bound generation and container, so a fenced host or a replaced planner run never submits. Sync settles a reservation whose executor is
gone from the plan record: committed when the plan or verdict exists, otherwise abandoned.
Ordinary sync visits a planning-only task while a planning host is open or still holds a
reservation or pending question, and terminalizes the questions of a closed planning host.
A planning question that waits on the owner (escalated, or blocking past
`question_unattended_secs`) moves the task to Needs me from Running (planner) or Review (plan
reviewer), as a worker's or reviewer's does. The planner pane's working state does not undo
that hold. An answer or withdrawal returns the task only from the hold its own question raised.
Interrupted calls of either are therefore `not_executed` without a reservation.

`corral agent doctor` lists `role_enabled` with each planning role's other effective
capabilities, and every planning generation records it in its capability snapshot. Planning
hosts relaunch through the plan pipeline, not `corral agent resume`.

## Starting Pi yourself: `corral pi`

`corral pi` starts the corral-pi host from your own shell, the way `pi` starts plain Pi. Which
host it starts depends on where it runs and its flags:

| Command | Opens |
|---|---|
| `corral pi` in a herdr pane inside a workspace | a new [assistant session](assistants.md) in this pane |
| `corral pi --resume A-<n>` | that assistant's next generation in this pane |
| `corral pi --pm` | a fresh PM conversation in this pane (below) |
| `corral pi --pm -c` | continue the previous PM conversation in this pane |
| `corral pi` outside a workspace, or `corral pi --bare` | plain Pi; outside a workspace with a one-line notice |

The launch line names what opened (`assistant A-1 (role assistant) in pane ...`). `--pm`,
`--resume` and `--over-cap` outside a workspace refuse `not_in_workspace`; `--bare`, `--pm`
and `--resume` exclude one another. Assistants, their soft cap and their resume are described
in [assistant sessions](assistants.md).

**Bound PM in this pane.** `corral pi --pm [--model M] [--provider P] [--effort E]`, run in a
herdr pane inside a corral workspace, makes that pane the PM pane. corral resolves `pm.agent` through
the same checks as `corral pm` and `corral agent resume --pm` (a corral-pi profile, an
approved scope with one or more projects, the asset installation, the provider's login); the
flags replace the profile's model, provider (`openai-codex` or `anthropic`) and effort for this
launch. It then opens the PM's next generation in this pane, writes the one-shot bootstrap and
launches it in-process: the same consume, check and exec `corral agent launch` performs, so the
bootstrap is gone before Bun starts and no path is shown or typed. The host binds with hello as
usual, and `corral agent state --pm` shows the new generation bound to this pane.
Each launch starts a fresh conversation by default. Pass `-c` or `--continue` with `--pm`
to resume the previous written transcript. `corral agent resume --pm` remains an explicit
recovery command and continues that transcript.

It refuses, before writing anything, with:

| Code | Meaning |
|---|---|
| `not_in_herdr` | `HERDR_PANE_ID` is unset; run it in a herdr pane, or use `--bare`. |
| `pane_unknown` | The pane is not on this store's herdr server. |
| `pm_alive` | A live corral-pi PM is bound (the message names its pane; `corral agent resume --pm --replace` replaces it), or a non-Pi PM session is open in another pane. |
| `pm_scope_unapproved`, `pm_scope_ambiguous`, `assets_unavailable`, `credentials_unavailable`, `role_unavailable`, ... | The profile, scope or asset checks of `corral pm`. |
| `unsupported_platform` | Not Unix. |

The launch itself then refuses exactly as `corral agent launch` does. When the bound PM is dead,
`corral pi --pm` does not open a rival generation: it takes the PM over with `corral agent resume
--pm` semantics (the lost generation is fenced, its calls drained and reconciled, one
`recovery_notice` posted), and generation n+1 opens in this pane. A changed approved project is
refused `out_of_scope`, as resume refuses it.

Stored tool receipts have a separate limit of 16,384 per container (previously 2,048).
They preserve operation replay and recovery across launches; starting a fresh conversation
does not discard them. This count is not a conversation-message or model-context limit.

The pane stays yours: see [session ownership](../session-ownership.md). corral never closes it,
treats it as the PM pane only while its bound host is verifiably running, and never types into
it once that host is gone, so `corral agent resume --pm` refuses `pane_adopted` then; run
`corral pi --pm` there again, or `corral pm` for a corral-owned pane. `--replace` still replaces
a live host after verifying its pid, start time and command. Opening the PM in another pane
(`corral pm` after the pane is gone) ends the adoption.

**Plain Pi: `corral pi --bare`.** This starts unbound Pi in the current directory, inside or
outside herdr and a corral workspace. Plain `corral pi` outside any workspace does the same
after printing a one-line notice that no corral workspace was found. There is no bootstrap, generation, nonce, bridge socket,
hello, role or delivery module, and no `corral_*` tool: only Pi's own read, edit, write, bash,
grep, find and ls. It runs the installed assets' pinned Pi SDK: a release installation's compiled
host runs its own bare mode (`corral-pi-host bare`) with its embedded Bun and bundled auth module,
so no Bun is needed on PATH, and a source installation runs under Bun after the same Bun range
and lock checks. `--provider` chooses the initial provider (default `openai-codex`; run `corral agent
login --provider <p>` first), not the session's provider boundary. The model picker exposes every
supported provider with a corral-owned subscription login, so you can switch between OpenAI
and Claude in the same session. Missing optional logins are explained at startup; log in and
restart to include them. Bound runs retain the provider selected by their run contract.
Bare mode uses the same guarded fetch, restrictions and model lanes as the
bound host: Anthropic runs through the Claude Agent SDK lane, with Claude Code's private config
directory in `$XDG_DATA_HOME/corral/corral-pi/bare/agent/claude-code/`. `--model` defaults to
Pi's own default for the provider. Sessions are kept in
`$XDG_DATA_HOME/corral/corral-pi/bare/sessions/` (Pi's agent directory beside it), and nothing
is written to `.corral/`. It refuses `assets_unavailable` without an installation,
`bare_unavailable` when a source installation predates `corral pi` (reinstall with `corral
agent install`), and `credentials_unavailable` without the provider's login.

## Explore children and oracle

`explore` researches one prompt in a separate read-only session. Concurrent calls are FIFO queued
above `subagents.max_parallel` (default 3). Standard enables it for workers and native planning
roles; reviewers need full or an explicit override, and PM cannot use it. Children inherit only
parent-enabled read/grep/find/ls and read LSP/web tools. They have no shell, writes, MCP, hooks,
Corral operations or recursive delegation. Both direct and nested calls use the final execution
check. Each model-visible result names its request, initiating generation, transcript, status
(`completed`, `partial`, `failed`) and stop reason; findings are capped at 32,768 characters with
an explicit truncation notice. Partial or failed results are not proof that the question is answered.
Turn cancellation, host shutdown and parent fencing cancel outstanding work.

The last allowed research request is reserved for tool-free synthesis, within the existing turn
ceiling, not an extra call after exhaustion. Near the deadline, the next request also switches to
synthesis (a five-second reserve, not a promise that synthesis can finish). If interrupted first,
visible notes and up to four bounded successful tool excerpts survive as labelled partial evidence;
no recovery model request runs after cancellation. Accounting uncertainty remains separate from
research completeness: a bounded partial answer can have known completed usage.

`explore({prompt})` remains supported. Optional `profile: "code" | "docs"` selects execution-path
research or official/version-specific documentation research. Optional `effort: "quick" | "standard" |
"deep"` selects off/low/high reasoning (clamped by the SDK to model support); quick uses at most four
turns, including synthesis, while standard/deep use the configured turn ceiling. All presets use the
same configured child model and bound provider; they never expand time/turn limits or switch
credentials. Optional `scope`, `starting_points` and `constraints` build a compact mission packet
with the worktree root and available tools. Supply relevant project rules and required evidence:
children do not inherit the conversation or automatically load the parent's project instructions.
These input fields together are capped at 32,768 characters. Scope is a research instruction, not
an additional filesystem sandbox. The docs profile uses only parent-enabled web tools and reports
missing search configuration rather than inventing sources; it does not enable web access itself.

Use direct tools for trivial lookups. For substantial independent research, give two or three
children non-overlapping questions and run them concurrently (for example using codemode
`Promise.all`), while doing separate useful work in the parent. Collect and reconcile cited findings
and coverage before answering. Helpers are not Corral independent reviewers. This API is still
one-shot; persistent background handles and child follow-up are not provided.

`oracle` is a single tool-free consultation, full-only by default for every role. It requires a
configured model; an absent model gives setup guidance without a provider request. Both tools
reuse the parent's restricted provider runtime and credentials, not ambient Pi settings or another
harness's credential store. Model selection is limited to the bound provider's existing catalog;
select a stronger model there rather than introducing another credential route.

```toml
[agent.capabilities.worker.subagents]
max_parallel = 3       # 1..16
max_turns = 12         # 1..64
timeout_secs = 120     # 1..600, includes queue time
# model = "gpt-6.1-sol" # absent: parent's configured model

[agent.capabilities.worker.oracle]
enabled = true
model = "gpt-6.1-sol"
timeout_secs = 120
```

Before execution, a host lifecycle receipt stores a parent-generation `usage_sessions` reference
under the container's canonical auxiliary path. Starts and terminal outcomes are receipted too;
a request is not silently replayed after a crash. `prepared` means work was not yet admitted;
`running` without a terminal record and `uncertain` both retain possible billable spend. Cancellation
can stop local work without establishing a provider's final charge. Completed partial usage remains
in the auxiliary transcript and the existing collector deduplicates responses across collections;
an absent response is **not** proof of zero spending. Budgets/reservations are a separate capability.
These are in-process children, not Corral runs, worker dispatches or new role authority.

## What else already applies

- **Usage.** The installed collector reads each generation's bound Pi transcript when task
  usage is refreshed, including review children and recorded compaction/branch-summary requests.
  Repeated collections and copied/resumed sessions deduplicate by session/response identity.
  `estimated_usd` is API-equivalent catalog pricing (including subscription models), not an invoice;
  Pi's own cost remains `reported_usd`. Unknown models retain `unpriced_tokens`, not a zero-dollar
  estimate. Missing/unwritten transcripts do not imply a free request.
  The shared `agent::usage::collect_container` readback also supports durable PM/planning records;
  these are separate from implementation task totals. Auxiliary producers can persist
  `AgentRunState.usage_sessions` with parent generation and launcher-selected session references.
  Explore and oracle use this seam before launching billable work; it does not provide a
  PM/planning cost CLI.
  See [the early checkpoint](../records/corral-pi-capabilities.md#early-usage-checkpoint-t-570).
- **Messages.** Messages to a corral-pi run or review child use inbox delivery through the
  `corral_pi` adapter, and every sync gives each open one a delivery pass. They are never typed
  into the pane.
- **Questions that need you.** While a corral-pi question is escalated or marked unattended,
  the task stays in Blocked with the question as its excerpt, even if the agent keeps working
  on something else, until the question is answered, cancelled or withdrawn. A reviewer's
  question does the same for a task in Review: its review stays open and the reviewer stays
  live, and the task returns to Review once the question is settled.
- **Idle and stall detection.** A corral-pi run whose host binding is healthy is never marked
  idle without a report, and its pane is never scanned for provider stalls; the host reports
  its turns and provider errors itself.
- **Cleanup.** When a corral-pi run or review child ends, only its runtime directory (socket and
  control credential) is removed. Its Pi session files under `.corral/agent/sessions/` are kept
  as a terminal transcript, recorded once by an `agent_session_reclassified` event, and its
  undelivered Pi messages are cancelled. This happens once per run or review child. A run whose
  host was lost keeps both, so it can be resumed.

Every `corral agent` subcommand is implemented: `launch`, `install`, `login`, `doctor`, `state`,
`cancel` and `resume`.

## Host loss and recovery

**Detection.** On every sync, corral probes the host of each open corral-pi run and review child
whose binding is healthy. The process hello recorded (its pid and kernel start time) must still
run, the generation's `ctl.cap` and socket must exist, and `get_state` must answer for the bound
generation and session. corral then continues `events_since` from the sequence it saved last, so
a restarted board or a fresh corral process picks up where the last one stopped; a gap is
recorded as `agent_events_gap`. A `busy` answer proves nothing and is retried on the next sync.
Any other failure marks the binding lost and records one `agent_lost` event for that generation,
with its reason:

| Reason | Meaning |
|---|---|
| `host_exited` | No live process has the recorded pid and start time. |
| `control_credential_missing`, `control_credential_unsafe` | The host runs, but its `ctl.cap` is gone or is not a 0600 file of yours, so nothing can control it. |
| `socket_missing`, `host_unreachable` | The host runs, but its socket is gone or does not answer. |
| `host_mismatch` | The socket answers for another generation or session. |

A lost host never ends its run. The run keeps its questions, messages and receipts, and messages
for it stay queued, unsent, for the next generation. A worker or reviewer host that exits to its
pane shell after hello is such a loss (`host_exited`): sync consumes the pane wrapper's exit
record, so it is never treated as a launch failure or a closed review. A live worker's task moves to Blocked with
an excerpt that starts `corral-pi host lost:` and names the three ways on: `corral agent resume`,
`corral retry` or `corral discard`. A lost reviewer whose pane stays keeps its review open and
its child live: the review watchdog never times it out on its idle pane, and the task moves to
Blocked with an excerpt that starts `corral-pi reviewer host lost:` and names
`corral agent resume <task> --review <id>`. A reviewer whose pane is gone is left to the existing
process-lost path, which closes the child; retry the review as a new review child. That holds
when the pane goes away while the reviewer waits for resume too: once the process-lost path
closed the child, the task returns to Review and the review watchdog escalates it as closed
without a verdict, with the child's questions and messages settled. A turn the
host was inside when it died stays recorded as it was, never ended or classified, and does not
hold the next generation's nudges or provider retries.

**`corral agent resume <task> [--run <id>] [--review <id>] [--replace]`** starts the next host
generation of an open run or review child in the same pane. `corral agent resume --pm`
resumes the PM's host in its saved pane, retaining its workspace identity and durable records.

1. It refuses a closed run or review child (`container_closed`) and a pane that is gone
   (`pane_gone`: retry or discard the run; a review child whose pane is gone is closed by the
   review watchdog's existing path and retried as a new review child). Nothing is fenced then.
2. It verifies the old host dead. A host that still runs is refused `host_alive`, unless you pass
   `--replace`: corral then checks that the recorded pid still has the recorded start time and
   runs `src/host/main.ts --launch-fd`, sends it SIGTERM and, if it does not exit within ten
   seconds, SIGKILL. A process that is not the Bun host is refused `host_identity_unverified` and
   is never signalled.
3. It fences the old generation (`agent_fenced`, reason `resume`) and removes its `ctl.cap` and
   socket, so every later request of that generation is refused `agent_fenced`.
4. It waits up to `[agent] drain_secs` for each reservation executor of that generation still
   running, then settles what the fence made settleable (a gate from its attempt's evidence).
5. It reads the old generation's Pi session file and classifies every tool call that has no
   result. Nothing is replayed, and the session file is never rewritten:

   | Call | Class |
   |---|---|
   | corral tool, committed | `result`, with the stored result |
   | corral tool, rejected (a gate refused at submission included) | `rejected`, with its code and reason |
   | `corral_gate` prepared and never submitted | `prepared`: the next `corral_gate` adopts the attempt if its candidate still matches; otherwise admission archives it and prepares a new attempt |
   | corral tool whose executor still ran after `drain_secs` | `in_progress`; one `recovery_update` message reports its final class once it settles |
   | corral tool, uncertain (an unconfirmed gate start, a legacy pane reply) | `outcome_unknown`, with the evidence |
   | single-phase corral tool with no receipt | `not_committed`, naming idempotent effects it may have made first (a `commit_all` commit, content-addressed blobs) |
   | two-phase corral tool (gate, capture, reply) with no reservation, or abandoned before it started | `not_executed` |
   | `edit`, `write`, `bash` | `outcome_unknown`; for bash, processes it started may still run |
   | `read`, `grep`, `find`, `ls` | `interrupted_no_effect`, unless a hook ran or may have run around it: then `outcome_unknown` naming the hook invocations |
   | LSP tools | `outcome_unknown`: a language server subprocess may have started or acted, even for built-in servers |
   | a hook that never completed (`hook:<phase>:<name>`) | `outcome_unknown`; hook scripts are never replayed |
   | `browser_navigate`, `browser_screenshot` | `outcome_unknown`; page scripts may have sent requests, and nothing is replayed |

   A definitely unsubmitted gate or reviewer preparation whose candidate changed (for
   example, after a rebase) is superseded on the next admission. Its complete preparation
   is attached as evidence before the replay pointer is removed, and
   `review_preparation_superseded` records the old authorization and artifact. Matching
   preparations still require their original operating origin. Uncertain or submitted
   launches are never disposed merely because the candidate changed. Retained preparations
   hold neither process capacity nor finishing priority over other tasks' changes/dispatch;
   they must first be re-admitted.

   When an edit, write or bash call is among them, corral attaches `git status --porcelain=v2`
   and `git diff --stat` of the worktree as an evidence artifact titled
   `Worktree after host loss (<container> generation <n>)`.

   An envelope the old generation was sent but never confirmed is settled without a resend
   (`delivery_fenced`): a `corral_delivery` entry for it in the shared Pi session observes its
   messages; otherwise each is marked failed `delivery_unknown` and the notice lists it as
   delivery unknown. Only `corral inbox retry <id>` sends such a message again.
6. It opens the next generation in the same pane, posts one `recovery_notice` message listing
   every classified call (its subject carries them as data), runs `corral agent launch` there,
   logs `agent_call_reconciled` for each corral call and `agent_resumed`, and returns a task it
   held in Blocked to Running. The new host
   continues the old generation's Pi session, so the conversation and the tool-call request IDs
   carry over; Pi gives the calls left open synthetic results only in what it sends the model.
   A reviewer's task returns to Review before its new host launches, so that host is admitted.
   The notice is delivered before any message queued while the host was lost, a Pi question's
   answer or correction included. It is never written into Pi's session file: the host records a
   placeholder line in its place and gives the model the notice's text in every request (a
   `recovery_update` is handled the same way). The notice's whole envelope fits the Pi host's
   8 KiB input; when its classes do not fit, it lists those that do and names the evidence file
   `Complete recovery notice (<container> generation <n>)`, which keeps them all.

**`corral agent cancel <task> [--turn <n>]`** asks the live host to cancel its current turn (or
turn `n`). It never fences: the run, its questions and its receipts stay live, a call in flight
keeps its own receipt, and a gate whose start was already under way is still submitted once.
The host is the authority on what it cancelled: its own `turn_ended` report of that turn says
`cancelled`, so the turn is classified `cancelled` in the transaction that ends it, however its
reports, the cancel's reply and sync interleave. It never gets a nudge, never spends the
generation's nudge, and a provider error in it is never retried or escalated (a `corral_done`
or verdict it committed still makes it `completed`). A turn that ended before either fact is
reclassified from the host's answer, which names the turn it cancelled. No speculative mark is
written before asking the host. A missing, negative or different-turn reply cannot retract
cancellation confirmed by a report or another caller. A host
that is not healthy is refused (`host_lost`, `host_fenced` or `host_unbound`); resume it
instead.

**Ending a lost run.** Discard, retry or drop end the run as usual. Its questions are cancelled
`run_ended`, a gate reservation still prepared settles `abandoned` (`not_submitted`, binding
`run_ended`) so no receipt stays reserved, its runtime directory is removed and its Pi session
is kept as a terminal transcript.

**Provider errors.** The host reports each provider error with a class. An `auth` failure puts
the task in Blocked with an excerpt that starts `corral-pi provider authentication failed:` and
asks you to run `corral agent login`; for a worker it clears when another turn starts, and a
reviewer's review is escalated by the review watchdog (`provider_escalated`, class `auth`), to be
restarted after the login. There is no automatic paid fallback outside the configured availability policy. A transient failure (`overload`,
`rate_limit` or another `provider` error) follows the provider-stall backoff: the host gets one
`turn_nudge` to continue after 60, 120 and 300 seconds (labelled "corral automatic retry n/3", each logged
`provider_retry`), and once those are used up the task goes to Blocked with the reason
(`provider_escalated`). A retry's turn that is still running keeps the episode and the retries it
spent; a turn that ends without a provider error ends the stall (`provider_recovered`).

## Cumulative model budgets

For any Pi role, configure limits in its capability table (for example):

```toml
[agent.capabilities.worker.budget]
max_tokens = 2000000
max_usd = 25.0
```

Supplying either positive limit enables enforcement in strict, standard and full. `enabled = false`
explicitly disables enforcement, not accounting. An enabled budget needs at least one limit.
Catalog-7 hosts account main requests, retries, compaction/branch-summary requests, explore and oracle
through one provider wrapper, even without limits. No new model tool can change these values.

Reservations commit in the bound container before transport starts. Admission conservatively reserves
**the model's full context window plus its bounded output**, with highest configured model price tiers
for USD (API-equivalent, not subscription billing). Small limits can therefore refuse a request even
when its likely actual usage would fit. Zero-priced nonlocal models are a pricing gap: a USD cap
refuses them and also refuses new calls if earlier unpriced usage exists. Local llama.cpp can have
explicit zero USD while still consuming tokens. Unbounded/invalid model metadata is refused.
The Claude Agent SDK subprocess lane can make hidden additional requests and does not bind an output
cap; it is explicitly unbounded, so hard-budget admission refuses it rather than claiming enforcement.
Uncapped lane usage is still recorded, and interrupted unbounded usage prevents later hard-cap admission.

Successful, nonzero terminal usage replaces its reservation once by provider/model/response identity.
Interrupted, errored, deferred or missing-usage calls retain their reservation; copies and rewinds do
not clear it. A refused admission or exhausted balance stops the host and cancels active providers,
parent work and children. Cancellation is best effort: provider billing may lag, so this is not a
zero-overrun billing guarantee. Inspect the host's `budget` state for cumulative tokens/nanodollars,
uncertain request count, pricing gaps, active requests and stop status.

Edit configuration and explicitly resume to apply new limits in the new generation's snapshot. Spent
and uncertain reservations remain; an unchanged exhausted cap stays stopped. Editing configuration
under an active host does not change its limits. Generations before catalog 7 have no reservation
history: state labels that limitation, and a hard cap on a container with such bound history refuses
admission rather than pretending earlier usage was zero. Use a new run for a fully enforced budget.
The offline usage collector remains separate and can report historical usage without amending limits.

## Rollback

corral-pi is opt-in, so rolling back means removing what you opted into. Nothing else changes:
OmO runs never consult a corral-pi hook.

1. Select OmO profiles for worker, reviewer and `pm.agent` defaults, close the old PM pane,
   then open a fresh PM with `corral pm` (a live pane is otherwise reused). The new OmO
   launch fences the old Pi generation without deleting its records.
   Remove unused `corral-pi` profiles and `[agent] assets_dir` from the
   configuration. New work then uses the selected OmO profiles.
2. End any live corral-pi run as you would any other run (discard, retry or drop). Run-end
   cleanup removes only that run's runtime directory (socket and `ctl.cap`).
3. Optionally delete the owned host files: `$XDG_DATA_HOME/corral/corral-pi/assets/`,
   `installed.json` and, to revoke the login, `auth/`.

Keep `.corral/`. Tasks, request receipts, questions and the Pi sessions under
`.corral/agent/sessions/` are corral records, and a lost run needs its session to resume.
After handing the PM back to OmO, read `corral pm brief --json`, `corral pm next --json`
and `corral question list --json`. Questions, receipts and pending decisions survive the
switch; no question is answered and no decision is resolved merely by changing harness.
