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

## What is checked before a run starts

Dispatch, `corral review-run` and opening the PM check a pi-native profile before any worktree,
pane or prompt exists, and refuse with a stable code naming what is missing:

| Code | Meaning |
|---|---|
| `vendor_refused` | The profile's vendor is not on the pi-native allowlist (`openai`). |
| `assets_unavailable` | No asset installation: `[agent] assets_dir` is unset and nothing is installed, or the directory does not exist. |
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
`--assets <dir>` when `[agent] assets_dir` is set. Reviewer and PM hosts launch the same way once
their stages land.

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
but corral's own, and no MCP, skills, prompt templates, themes or context files such as
`AGENTS.md`. Install telemetry, analytics, the cache warmer and the remote model catalog are
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

## What else already applies

- **Usage.** A pi-native run's usage note reads `pi-native usage collector not installed`
  until the collector lands; no cost is estimated for it.
- **Messages.** Messages to a pi-native run or review child use inbox delivery through the
  `pi_native` adapter, and every sync gives each open one a delivery pass. Until that adapter is
  installed they stay queued and are never typed into the pane.
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

`corral agent launch` is implemented. The other `corral agent` subcommands (`install`, `login`,
`doctor`, `state`, `cancel`, `resume`) are already listed in `corral agent --help`; each answers
`op_unavailable`, naming the stage that implements it, until that stage lands.
