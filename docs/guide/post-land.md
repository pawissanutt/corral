# Durable post-land operations

Landing a commit and installing a usable binary are separate outcomes.
`corral hooks list --json` exposes both, with the exact command, environment,
cwd, timeout, exit status, full log path, installed identity and retained
policy for each hook attempt. Operations also become evidence artifacts;
the autopilot acceptance record includes their receipts and digest.

Hooks are opt-in, project-scoped, ordered argv commands. There is no implicit
shell, inherited environment, automatic retry or coalescing. Keep ordinary
pre-land gates: a post-land check cannot make a broken candidate acceptable.
Only a successful base advancement triggers hooks. Keep-branch approval,
owner-wait and a failed merge do not trigger them.

## Corral recipe

Replace executable paths and HOME/PATH with your installation. The following
example uses the docs fixture's project name `app`; configure
`projects.corral.post_land` for the corral project.

```toml
[[projects.app.post_land]]
name = "check"
argv = ["/home/pawis/.cargo/bin/cargo", "check", "--workspace", "--all-targets"]
cwd = "."
env = { HOME = "/home/pawis", PATH = "/home/pawis/.cargo/bin:/usr/bin:/bin", RUSTC_WRAPPER = "" }
timeout_secs = 1800
max_attempts = 3

[[projects.app.post_land]]
name = "install"
argv = ["/home/pawis/.cargo/bin/cargo", "install", "--locked", "--path", "crates/corral"]
cwd = "."
env = { HOME = "/home/pawis", PATH = "/home/pawis/.cargo/bin:/usr/bin:/bin", RUSTC_WRAPPER = "" }
timeout_secs = 1800
max_attempts = 3
identity = { binary = "/home/pawis/.cargo/bin/corral", args = ["build-identity"], expected_stdout = "{sha}" }

[[projects.app.post_land]]
name = "smoke"
argv = ["/home/pawis/.cargo/bin/corral", "--help"]
cwd = "."
env = { HOME = "/home/pawis", PATH = "/home/pawis/.cargo/bin:/usr/bin:/bin" }
timeout_secs = 30
max_attempts = 3
```

`cwd` is relative to the base checkout, cannot contain parent traversal, and
cannot resolve outside it. Names are unique within the ordered array and
use letters, digits, underscores or hyphens. Timeout is 1 through 86,400
seconds; `max_attempts` is 1 through 10 including the first execution.
Environment keys beginning with CORRAL_ are reserved. Corral adds
CORRAL_LANDED_SHA, CORRAL_BUILD_SHA, CORRAL_PROJECT and CORRAL_HOOK_TOKEN.
Every command has null stdin and a separate process group. Output is written
directly to a file without truncation; timeouts terminate the command group.

An identity-bearing install preserves an existing regular executable before
running. A failed install or failed identity proof restores that executable
where possible; restoration errors remain in the failed receipt. The identity
probe must exit successfully and print exactly the expected trimmed stdout,
and the executable's SHA-256 must remain stable across that probe.
Expected stdout must contain `{sha}`; a static version is not proof of the
landed commit.
Corral's `build-identity` command prints the compile-time CORRAL_BUILD_SHA
(or `unknown` for an ordinary build). Cargo rebuilds that value on every
SHA change. An install is expected to be idempotent; never use a hook for
non-repeatable external effects.

## Failure, restart and retry

Receipts under `.corral/state/post-land/` are fsynced and atomically replaced.
Their keys bind project, repository, landed SHA, hook name (within the operation)
and the complete ordered policy digest. The original policy is retained even
if configuration changes. Corral holds the repository landing lock across the
merge and all hooks; a later corral landing cannot advance main while a prior
operation is incomplete. A changed or dirty base checkout is reported as
uncertain rather than checked under the wrong SHA. External Git operations do
not participate in corral's lock.

Failures stop the sequence. The task remains Done and the commit remains
landed, but operational readiness blocks project dispatch and tasks depending
on that project's Done tasks. There is one persisted notification attempt per
failure incident, also exposed as a `blocked` event. Notification intent is
saved before delivery, so a crash can omit a notification rather than duplicate
it; the receipt remains authoritative.

Sync reconciles receipts even without herdr. You can also run
`corral hooks reconcile` explicitly. Successful receipts never rerun.
Interrupted commands remain uncertain while a recorded process identity or
operation environment token is alive. After exit, an identity-bearing install
can be proved from its executable without reinstalling. Without proof, side
effects are uncertain, not presumed successful or automatically repeated.
No generic runner can promise exactly-once arbitrary process side effects.

Use `corral hooks retry app <full-landed-sha>` for a bounded explicit retry.
Only the first unsuccessful hook runs again, followed by still-pending hooks.
An uncertain command additionally requires `--acknowledge-uncertain`; retry
is refused while a known operation process remains alive or the attempt bound
is exhausted. Read the receipt and log before acknowledging uncertain effects.
No operation resets, reverts, pushes, or removes the landed commit.
