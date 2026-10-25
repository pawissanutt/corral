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
pi-native profile takes no `args`. Only the `openai` vendor is allowed; any other vendor (an
Anthropic model, for example) is refused when you dispatch, before a worktree exists.

Other `[agent]` settings apply only to pi-native runs: `question_unattended_secs` (default
600), `gate_start_uncertain_secs` (default 300) and `drain_secs` (default 60).

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
`installed_assets_changed` until you reinstall. The installation carries the worker and reviewer
roles; until the PM stage lands, a pi-native PM open refuses `role_unavailable pm (owner pm-role)`.

## Credentials

`corral agent login` runs once, interactively, in your own pane. It uses Pi's OpenAI Codex
(ChatGPT Plus/Pro subscription) OAuth flow from the installed `src/auth/login.ts`, offering a
browser or a device-code login. The credential is written to
`$XDG_DATA_HOME/corral/pi-native/auth/auth.json`: the directory is 0700, the file 0600. During
login the network is limited to POST requests to OpenAI's OAuth token and device-code
endpoints. A fixture installation is refused, because its credential is synthetic. A failed
login prints only the failing step and an HTTP status, never the provider's response, so no
token reaches the terminal or a log.

A running host reads that file through the installed `src/auth/index.ts` and lets Pi refresh the
token in place. Every write holds an exclusive kernel lock (`flock`) on `auth.json.lock`, and the
kernel releases it if a host dies, so concurrent hosts never double-refresh and no stale lock
needs recovery. Without a login, the host
stops with `credentials_unavailable` and asks you to run `corral agent login`. Only OAuth
credentials are stored. There is no API-key or other paid fallback, and corral never reads or
writes `~/.codex`, `~/.pi`, `~/.claude` or another harness's store.

## Readback

`corral agent doctor [--json]` reports, and `corral doctor` adds when a pi-native profile is
configured:

| Check | Reports |
|---|---|
| `pi_native_bun` | The Bun on PATH, its version and whether it is inside the GO range. |
| `pi_native` | The asset directory and its source (installed or `[agent] assets_dir`), its digest, the GO verdict, the auth module (`corral-pi`, `fixture` or `missing`), the installed roles (`roles: worker, reviewer` until the PM stage lands) and each role's readiness with the refusal it would get. |
| `pi_native_credentials` | Whether a credential is present and its permissions are safe, from file metadata only; its contents are never read into the output. |
| `pi_native_question_wake` | Where a new question's wake goes: the bound Pi-native PM's inbox, the OmO PM push (when `pm.push_kinds` includes `question_opened`), or nowhere, in which case a waiting question reaches Needs me after `question_unattended_secs`. |

`corral agent state <task> [--run <id>] [--review <id>] [--json]` (or `--pm`) shows one
container's binding: harness, role, host health, current generation, generation, receipt and
turn counts, open question keys, and the same installation summary.

## What is checked before a run starts

Dispatch, `corral review-run` and opening the PM check a pi-native profile before any worktree,
pane or prompt exists, and refuse with a stable code naming what is missing:

| Code | Meaning |
|---|---|
| `vendor_refused` | The profile's vendor is not on the pi-native allowlist (`openai`). |
| `assets_unavailable` | No asset installation: `[agent] assets_dir` is unset and `corral agent install` has not run, or the directory does not exist. |
| `fixture_assets_not_allowed` | A test fixture installation (it carries `fixture.json`) was reached other than through `[agent] assets_dir`. |
| `compat_unavailable` | The assets have no `compat.json` recording a `GO` verdict. |
| `role_unavailable <role>` | The assets lack `src/roles/<role>/index.ts`; the reason names the stage that adds it. |
| `delivery_unavailable` | The assets lack `src/delivery/index.ts`. |
| `credentials_unavailable` | The assets lack `src/auth/index.ts`. |

Until every stage has landed, a pi-native dispatch therefore stops at one of these refusals.
`corral doctor` reports the same readback for each role when a pi-native profile is configured,
and says nothing about Pi otherwise.

## How a worker host starts

When a pi-native worker run reaches its pane, dispatch opens the run's next host generation and
writes a one-shot bootstrap file, mode 0600, into that generation's runtime directory, mode 0700:
`$XDG_RUNTIME_DIR/corral/<store>/<container>/g<n>/`, or `/tmp/corral-<uid>/...` without
`XDG_RUNTIME_DIR`. The pane then runs `corral agent launch --bootstrap <path>`, plus
`--assets <dir>` when `[agent] assets_dir` is set. A reviewer host launches the same way from
`corral review-run` (see The reviewer role); the PM host will once its stage lands.

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
off. Its network is a guarded fetch that sends only POST requests to the pinned provider API
(`https://chatgpt.com/backend-api/codex/responses`) and that provider's token-refresh endpoint;
every other request, bug-report uploads included, is refused. The same holds after a reload and
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
that turn), `provider_failed`, `owner_engaged` (you typed the turn's input in the pane, or typed
into the pane while the turn ran, which upstream Pi steers into that turn), `awaiting_answer` (a
question of the run was pending), `waiting_external` (a gate or wait would wake the worker) or
`no_report`. The latest `no_report` turn of a host generation gets one automatic `turn_nudge`
message; a later one puts the task in Needs me with `turn_ended_without_report`, which clears when
another turn starts. The nudge is tied to the turn that spent it, an earlier report never counts
for a later turn, and a nudge spent before a change request is not given back. Turns you typed in
are never nudged.

**Vendors.** Only `openai` models run pi-native; a worker profile with an Anthropic model is
refused `vendor_refused` before a worktree exists.

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
  as a terminal transcript, and its undelivered Pi messages are cancelled. This happens once per
  run or review child. A run whose host was lost keeps both, so it can be resumed.

`corral agent launch`, `install`, `login`, `doctor` and `state` are implemented. `cancel` and
`resume` are already listed in `corral agent --help`; each answers `op_unavailable`, naming the
stage that implements it, until that stage lands.

## Rollback

Pi-native is opt-in, so rolling back means removing what you opted into. Nothing else changes:
OmO runs never consult a pi-native hook.

1. Remove every profile whose harness is `pi-native`, and `[agent] assets_dir`, from the
   configuration. New dispatches, review runs and PM opens then use OmO, and `corral doctor`
   stops reporting Pi.
2. End any live pi-native run as you would any other run (discard, retry or drop). Run-end
   cleanup removes only that run's runtime directory (socket and `ctl.cap`).
3. Optionally delete the owned host files: `$XDG_DATA_HOME/corral/pi-native/assets/`,
   `installed.json` and, to revoke the login, `auth/`.

Keep `.corral/`. Tasks, request receipts, questions and the Pi sessions under
`.corral/agent/sessions/` are corral records, and a lost run needs its session to resume.
