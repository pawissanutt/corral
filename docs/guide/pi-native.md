# Pi-native agents (opt-in, in progress)

corral is gaining an opt-in agent host that runs the Pi coding agent under Bun in a run pane
and talks to corral through authenticated, idempotent calls instead of screen or idle
detection. OmO stays the default worker, reviewer and PM. Nothing changes unless you configure
a profile whose harness is `pi-native`.

The host lands in stages. This page states what works today; the stages that are not yet
installed refuse explicitly rather than half-working.

## Configuring a profile

`pi-native` is a built-in harness: you do not declare a `[harnesses.pi-native]` table. A
profile selects it like any other harness:

```toml
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
# A development or fixture asset directory; leave unset to use the managed installation.
assets_dir = "/path/to/pi-assets"
```

Model and effort travel in the host's one-shot bootstrap, not on the command line, so a
pi-native profile takes no `args`. The `openai` and `anthropic` vendors are allowed (for
example `model = "claude-opus-5-5"` with `vendor = "anthropic"`); any other vendor is refused
when you dispatch, before a worktree exists.

Other `[agent]` settings apply only to pi-native runs: `question_unattended_secs` (default
600), `gate_start_uncertain_secs` (default 300) and `drain_secs` (default 60).

`[agent] ui` sets how the run pane looks (default `"rich"`). In rich mode the pane uses the
Dracula palette (a dark theme; on a light terminal background use `plain`) and shows a
two-line header (role, task and run, generation, model, effort and worktree), names the
terminal tab after the task and role (`T-503 worker`), and adds footer status for the corral
binding (`● corral g2`, or `○ corral unbound`) and, once one happens, a count of refused network
calls and denied tools. Each `corral_*` call is one line, such as `corral ask clarification: ...`
followed by `✓ key=Q-3` or `✗ rejected  <code>: <reason>`. Expanding tool output
shows the full receipt. `ui = "plain"` keeps stock Pi. The setting is read at dispatch, so it
applies to hosts launched after the change.

## Installing the host

`corral agent install [--from <dir>] [--json]` installs the host from the `packages/corral-pi`
package (by default the one in the checkout the binary was built from). It refuses a fixture
installation (`fixture_assets_not_allowed`) and a Bun outside the package's `compat.json`
range (`bun_unavailable`, `bun_version_unsupported`), then copies only what a host runs:
`package.json`, `bun.lock`, `compat.json`, `src/` (without `*.test.ts`) and `assets/`, never
`test/` or `fixture.json`. It runs `bun install --frozen-lockfile` in that copy and stores it as
`$XDG_DATA_HOME/corral/pi-native/assets/<digest>/` (`~/.local/share/corral/pi-native/...`
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
Log in a model provider for pi-native
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
provider's credential is its own file in `$XDG_DATA_HOME/corral/pi-native/auth/`:
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
`corral agent login --provider anthropic`. Only OAuth credentials are stored. There is no
API-key or other paid fallback: the host drops Pi's Anthropic API-key handler and clears every
`ANTHROPIC_*` variable (API key, auth and OAuth tokens, workload federation) before Pi loads, so
after a `/logout` or a removed credential the provider has no auth rather than a paid key, and
any Anthropic request without the subscription OAuth token is refused before it is sent. corral
never reads or writes `~/.codex`, `~/.pi`, `~/.claude` or another harness's store.

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

## Readback

`corral agent doctor [--json]` reports, and `corral doctor` adds when a pi-native profile is
configured:

| Check | Reports |
|---|---|
| `pi_native_bun` | The Bun on PATH, its version and whether it is inside the GO range. |
| `pi_native` | The asset directory and its source (installed or `[agent] assets_dir`), its digest, the GO verdict, the auth module (`corral-pi`, `fixture` or `missing`), the installed roles and each role's readiness with the refusal it would get. |
| `pi_native_credentials` | Whether any provider is logged in, each provider's state (`openai-codex`, `anthropic`) and whether the files' permissions are safe, from file metadata only; their contents are never read into the output. |
| `pi_native_question_wake` | Where a new question's wake goes: the bound Pi-native PM's inbox, the OmO PM push (when `pm.push_kinds` includes `question_opened`), or nowhere, in which case a waiting question reaches Needs me after `question_unattended_secs`. |

`corral agent state <task> [--run <id>] [--review <id>] [--json]` (or `--pm`) shows one
container's binding: harness, role, host health, current generation, generation, receipt and
turn counts, open question keys, and the same installation summary.

## What is checked before a run starts

Dispatch, `corral review-run` and opening the PM check a pi-native profile before any worktree,
pane or prompt exists, and refuse with a stable code naming what is missing:

| Code | Meaning |
|---|---|
| `vendor_refused` | The profile's vendor is not on the pi-native allowlist (`openai`, `anthropic`). |
| `provider_unsupported_by_host` | The vendor is on the allowlist but the installed host's `compat.json` `providers` list does not include it. A `compat.json` without `providers` declares `openai` only. |
| `assets_unavailable` | No asset installation: `[agent] assets_dir` is unset and `corral agent install` has not run, or the directory does not exist. |
| `fixture_assets_not_allowed` | A test fixture installation (it carries `fixture.json`) was reached other than through `[agent] assets_dir`. |
| `compat_unavailable` | The assets have no `compat.json` recording a `GO` verdict. |
| `role_unavailable <role>` | The assets lack `src/roles/<role>/index.ts`; the reason names the stage that adds it. |
| `delivery_unavailable` | The assets lack `src/delivery/index.ts`. |
| `credentials_unavailable` | The assets lack `src/auth/index.ts`, or (managed installation only) the profile's provider has no login; the reason names the provider and `corral agent login --provider <provider>`. An `[agent] assets_dir` installation skips the login check, and its host refuses at launch instead. |

Until every stage has landed, a pi-native dispatch therefore stops at one of these refusals.
`corral doctor` reports the same readback for each role when a pi-native profile is configured,
and says nothing about Pi otherwise.

## How a worker host starts

When a pi-native worker run reaches its pane, dispatch opens the run's next host generation and
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
you can watch and type.

### What the host leaves out

Only the role's tools are active; any other tool call is refused. The host loads no extension
but corral's own, and no MCP, prompt templates, themes or context files such as `AGENTS.md`.
The only skill is the one the role ships inside the installation (the worker's corral-worker
skill); skills in the worktree, your home directory or another harness are never loaded. Install telemetry, analytics, the cache warmer and the remote model catalog are
off. Its network is a guarded fetch that sends only POST requests to the bound profile's provider
API and that provider's token-refresh endpoint: for `openai`,
`https://chatgpt.com/backend-api/codex/responses` and `https://auth.openai.com/oauth/token`; for
`anthropic`, only `https://platform.claude.com/v1/oauth/token`, since the host process never
calls `https://api.anthropic.com` itself. Claude Code's own traffic (model requests, and
whatever else it sends; its non-essential traffic is switched off with
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`) leaves from its subprocess, outside this guarded
fetch. Every other request, the other provider's endpoints and bug-report uploads included, is
refused. The same holds after a reload and
after the session is replaced.

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

A pi-native worker run gets Pi's read, edit, write, bash, grep, find and ls tools, plus nine
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

**Waits.** `corral_wait` registers the wait and its wake subscription together, as a channel wait
the host declares `wait_v1` for. A wait without a deadline is refused `unobservable_wait`,
because its deadline is the only wake corral can guarantee. When the wait leaves `pending`, sync posts one `wait_wake` for it.

**Questions.** `corral_ask` returns at once, blocking or not, and the worker may keep working;
the task reaches Needs me only when a question is escalated or becomes unattended. While any of
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
message; a later one puts the task in Needs me with `turn_ended_without_report`, which clears when
another turn starts. The nudge is tied to the turn that spent it, an earlier report never counts
for a later turn, and a nudge spent before a change request is not given back. Turns you typed in
are never nudged.

**Vendors.** The allowlist admits `openai` and `anthropic`; a worker profile with any other vendor
is refused `vendor_refused` before a worktree exists. A vendor also has to be one the installed
host declares in `compat.json` `providers`; the shipped host declares `openai` and
`anthropic`, and an older installation without the list is refused
`provider_unsupported_by_host` for Anthropic until you reinstall. Setup discovery lists only the
vendors the installed host declares, so it does not offer an Anthropic profile the host cannot
run. Anthropic profiles are opt-in and use subscription OAuth held only in the corral-owned Pi
auth directory (`corral agent login --provider anthropic`); see
`docs/records/pi-native-claude-route.md` for the evidence and the accepted policy risk.

## The reviewer role

`corral review-run <task> <profile>` with a pi-native reviewer profile opens the review pane
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
Needs me at once with `turn_ended_without_report`, as the idle watchdog would after
`review_idle_after_secs`. A healthy reviewer host is never judged by its pane's idle status.

## The PM role

OmO remains the default PM; explicitly select a `pi-native` profile as `pm.agent` to opt in.
Opening a Pi PM requires an approved operating scope selecting exactly one project. Its
`AgentPm` binding is separate from task runs, with null task and run, one current PM and one
authorized project. Mutations check that binding and the target project again at commit.

The PM gets typed read-only tools for brief, next, show, list, search, route, artifacts and
lessons, plus receipted question answer, escalate and cancel, reply, comment and capture.
There is no shell, edit, MCP, skills, `pm act`, owner answer, deadline or acceptance tool.
Reload and session replacement retain this loadout. Clarification and technical-choice
answers need a cited basis within the approved write scope and acceptance; owner-only and
permission questions escalate, and the owner may supersede any PM answer.

PM tools never run sync or `pm_workset`. A separate board/sync loop is required for wakes:
keep the board running or run `corral sync --watch` outside the PM. A healthy scoped Pi PM
receives each pending question wake through the Pm inbox. Its subject retains the outbox
dedupe key `qwake:<question>:<revision>`. Sync acknowledges the question's outbox only after
publication; if corral crashes between publication and acknowledgement, the next pass finds
the same logical message instead of posting another. The Pi inbox adapter then submits it
to the host, starting an idle turn. Enqueueing alone is not delivery, and observation still
requires the host's `corral_delivery` entry.

PM replacement and project changes are rechecked before publication and at the delivery
boundary, so an old project's wake is never sent to a new project's PM. OmO question push
keeps its existing opt-in configuration; without triage, waiting questions still reach
Needs me after `question_unattended_secs`. See [the PM loop](pm.md) for authority and handover.

## What else already applies

- **Usage.** A pi-native run's usage note reads `pi-native usage collector not installed`
  until the collector lands; no cost is estimated for it.
- **Messages.** Messages to a pi-native run or review child use inbox delivery through the
  `pi_native` adapter, and every sync gives each open one a delivery pass. They are never typed
  into the pane.
- **Questions that need you.** While a Pi-native question is escalated or marked unattended,
  the task stays in Needs me with the question as its excerpt, even if the agent keeps working
  on something else, until the question is answered, cancelled or withdrawn. A reviewer's
  question does the same for a task in Review: its review stays open and the reviewer stays
  live, and the task returns to Review once the question is settled.
- **Idle and stall detection.** A pi-native run whose host binding is healthy is never marked
  idle without a report, and its pane is never scanned for provider stalls; the host reports
  its turns and provider errors itself.
- **Cleanup.** When a pi-native run or review child ends, only its runtime directory (socket and
  control credential) is removed. Its Pi session files under `.corral/agent/sessions/` are kept
  as a terminal transcript, recorded once by an `agent_session_reclassified` event, and its
  undelivered Pi messages are cancelled. This happens once per run or review child. A run whose
  host was lost keeps both, so it can be resumed.

Every `corral agent` subcommand is implemented: `launch`, `install`, `login`, `doctor`, `state`,
`cancel` and `resume`.

## Host loss and recovery

**Detection.** On every sync, corral probes the host of each open pi-native run and review child
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
for it stay queued, unsent, for the next generation. A live worker's task moves to Needs me with
an excerpt that starts `Pi-native host lost:` and names the three ways on: `corral agent resume`,
`corral retry` or `corral discard`. A lost reviewer whose pane stays keeps its review open and
its child live: the review watchdog never times it out on its idle pane, and the task moves to
Needs me with an excerpt that starts `Pi-native reviewer host lost:` and names
`corral agent resume <task> --review <id>`. A reviewer whose pane is gone is left to the existing
process-lost path, which closes the child; retry the review as a new review child. That holds
when the pane goes away while the reviewer waits for resume too: once the process-lost path
closed the child, the task returns to Review and the review watchdog escalates it as closed
without a verdict, with the child's questions and messages settled. A turn the
host was inside when it died stays recorded as it was, never ended or classified, and does not
hold the next generation's nudges or provider retries.

**`corral agent resume <task> [--run <id>] [--review <id>] [--replace]`** starts the next host
generation of an open run or review child in the same pane. `corral agent resume --pm`
resumes the PM's host in its saved pane, retaining its project scope and durable records.

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
   | `corral_gate` prepared and never submitted | `prepared`: the next `corral_gate` adopts the attempt and starts it once |
   | corral tool whose executor still ran after `drain_secs` | `in_progress`; one `recovery_update` message reports its final class once it settles |
   | corral tool, uncertain (an unconfirmed gate start, a legacy pane reply) | `outcome_unknown`, with the evidence |
   | single-phase corral tool with no receipt | `not_committed`, naming idempotent effects it may have made first (a `commit_all` commit, content-addressed blobs) |
   | two-phase corral tool (gate, capture, reply) with no reservation, or abandoned before it started | `not_executed` |
   | `edit`, `write`, `bash` | `outcome_unknown`; for bash, processes it started may still run |
   | `read`, `grep`, `find`, `ls` and the LSP tools | `interrupted_no_effect` |

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
   held in Needs me to Running. The new host
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
the task in Needs me with an excerpt that starts `Pi-native provider authentication failed:` and
asks you to run `corral agent login`; for a worker it clears when another turn starts, and a
reviewer's review is escalated by the review watchdog (`provider_escalated`, class `auth`), to be
restarted after the login. There is no paid fallback. A transient failure (`overload`,
`rate_limit` or another `provider` error) follows the provider-stall backoff: the host gets one
`turn_nudge` to continue after 60, 120 and 300 seconds (labelled "corral automatic retry n/3", each logged
`provider_retry`), and once those are used up the task goes to Needs me with the reason
(`provider_escalated`). A retry's turn that is still running keeps the episode and the retries it
spent; a turn that ends without a provider error ends the stall (`provider_recovered`).

## Rollback

Pi-native is opt-in, so rolling back means removing what you opted into. Nothing else changes:
OmO runs never consult a pi-native hook.

1. Select OmO profiles for worker, reviewer and `pm.agent` defaults, close the old PM pane,
   then open a fresh PM with `corral pm` (a live pane is otherwise reused). The new OmO
   launch fences the old Pi generation without deleting its records.
   Remove unused `pi-native` profiles and `[agent] assets_dir` from the
   configuration. New work then uses the selected OmO profiles.
2. End any live pi-native run as you would any other run (discard, retry or drop). Run-end
   cleanup removes only that run's runtime directory (socket and `ctl.cap`).
3. Optionally delete the owned host files: `$XDG_DATA_HOME/corral/pi-native/assets/`,
   `installed.json` and, to revoke the login, `auth/`.

Keep `.corral/`. Tasks, request receipts, questions and the Pi sessions under
`.corral/agent/sessions/` are corral records, and a lost run needs its session to resume.
After handing the PM back to OmO, read `corral pm brief --json`, `corral pm next --json`
and `corral question list --json`. Questions, receipts and pending decisions survive the
switch; no question is answered and no decision is resolved merely by changing harness.
