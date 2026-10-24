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

## What else already applies

- **Usage.** A pi-native run's usage note reads `pi-native usage collector not installed`
  until the collector lands; no cost is estimated for it.
- **Messages.** Messages to a pi-native run or review child use inbox delivery through the
  `pi_native` adapter. Until that adapter is installed they stay queued and are never typed into
  the pane.
- **Idle and stall detection.** A pi-native run whose host binding is healthy is never marked
  idle without a report, and its pane is never scanned for provider stalls; the host reports
  its turns and provider errors itself.
- **Cleanup.** When a pi-native run or review child ends, only its runtime directory (socket and
  control credential) is removed. Its Pi session files under `.corral/agent/sessions/` are kept
  as a terminal transcript. A run whose host was lost keeps both, so it can be resumed.

The `corral agent` subcommands (`install`, `login`, `doctor`, `state`, `cancel`, `resume`,
`launch`) are already listed in `corral agent --help`; each answers `op_unavailable`, naming
the stage that implements it, until that stage lands.
