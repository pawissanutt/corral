# Configuration

Configure the workspace engine in `.corral-next/corral.toml` and the Pi host in
`.corral-next/pi.toml`. See the [daemon guide](daemon.md) for browser and device access.

## Running slots (`corral-next`)

In `.corral-next/corral.toml`, `[limits].running` caps the number of tasks in flight across
all projects (default `4`). An optional `[projects.<key>].running` also caps the tasks in
that project. Both count one slot per task in **Running**, **Blocked**, or **Review**,
not one per session or process.

A task holds its slot from dispatch through the gate, review, and landing pipeline, even
while waiting for owner approval with no session running. It frees the slot when it reaches
Done or Dropped, or is moved back to Backlog or Ready. Send-backs, retries, and replies
reuse the slot of a task that still holds one, even when the board is full. A retry from
outside those three statuses checks admission again. New dispatches and moves into a
slot-holding status from outside one need a free slot, even with `--force`.

The limit bounds unfinished work, not just active workers. For example, with the default
limit, four tasks waiting in Review can prevent new dispatches even when no worker is
running. Clear stalled reviews or reconsider the cap if you need more coding work to start
while reviews wait; raising it also allows more unfinished work to accumulate.

## Session wrappers (`corral-next`)

Set `wrap` in `[runner]` in `.corral-next/corral.toml` to run task sessions in another
local environment. A `[projects.<key>].wrap` replaces the workspace template for that
project. Omitting both leaves the launch command unchanged. The wrapper applies to worker
and reviewer launch commands in `process` and `herdr` mode, not to gate commands or
standalone Build sessions.

`{command}` is required (`corral-next config check` rejects a template without it). It
expands to the entire rendered harness line as one shell argument; use `sh -c {command}`
to run it inside the chosen environment. `{root}` is the workspace root, `{worktree}` is
the session's working directory, and `{dir}` is the run directory containing `prompt.md`.
Paths are absolute and shell-escaped, so no surrounding quotes are needed. Placeholders are
for ordinary shell arguments (or parts of arguments, such as `--volume {root}:{root}`),
not nested shell source inside command substitutions or here-documents. To try wrapping
without a container, use `wrap = "env FOO=1 sh -c {command}"`.

For example, with a local `corral-dev` image containing the selected harness, `sh` and Git,
and with Corral installed on the host at `/opt/corral/corral-next`:

```toml
[runner]
mode = "process"
wrap = """exec podman run --rm --init --userns=keep-id \\
  --volume {root}:{root} --volume {worktree}:{worktree} --workdir {worktree} \\
  --volume /opt/corral:/opt/corral:ro \\
  --env CORRAL_NEXT_ROOT --env CORRAL_TASK --env CORRAL_RUN --env CORRAL_ACTOR \\
  corral-dev sh -c {command}"""
```

Keep paths identical inside and outside the container: linked Git worktrees reference the
host checkout, and the generated prompt invokes `corral-next` by absolute path. Adjust the
binary mount to your installation and use an image that can run that binary. The root mount
makes the workspace state and run files available to `corral-next done`. Mount the source
checkout if it lives outside these mounts, and make a Git author identity available in the
container so `done` can commit changes. Podman doesn't forward project environment variables
automatically; add `--env NAME` for any the harness needs.

Run session management commands such as `corral-next stop` on the host. Keep the wrapper in
the foreground and ensure it forwards termination to the payload; `--rm` removes the container
when it exits, not on its own if the payload refuses to exit. In `herdr` mode, add `-it` for a
harness that needs a terminal and omit `exec` to keep the pane's shell after exit.

The example covers session placement and submission. The `done` CLI starts its detached
pipeline in the environment that invoked it, so automatic gates or reviewer placement from
inside a container need their usual tools and access there. If container exit interrupts that
helper, host `corral-next sync` can advance a submission still pending in Review.

The built-in `pi` harness can be wrapped: Corral adds its `--dir` and `--socket` arguments
before wrapping, and still uses the Unix socket for readiness and session control. For Podman,
install `corral-pi` in the image and add `--env CORRAL_FRESH`. Also bind-mount the host socket
directory at the same absolute path with the same user access (typically
`$XDG_RUNTIME_DIR/corral-next`, or `/tmp/corral-next-<uid>` if unset). Mount the directory,
not an individual socket file: Pi creates the socket at startup. Without that shared directory
Corral cannot connect to the wrapped host.

## build

For app and terminal Build sessions, set child concurrency, child profiles and delegation
policy in `.corral-next/pi.toml` at the Corral workspace root. A standalone Build reads
that file at the Git top-level, or in the start directory outside Git. Resume rereads
current settings from the recorded root, not the directory used to run the resume command.

`[build] max_parallel` sets the per-session cap for running children (1–32, default 32).
Set a lower value, such as `3`, to limit concurrency. This does not change the maximum of
32 children in one `task` call or any board [running-task limits](#running-slots-corral-next). Eligible non-Build
fleets and Explore also default to 32. Where the engine supplies role capability settings,
explicit lower `fleet.max_parallel` and `subagents.max_parallel` values still apply. These
limits are separate from `[build]`.

```toml
[build]
max_parallel = 3

[build.profiles.trace]
model = "openai-codex/gpt-6.1-sol"
effort = "medium"
description = "Read-only execution-path tracing"
role = "Trace the active call path; cite file and line for the enforcing check and note unresolved branches."

[build.delegation]
mode = "proactive"

[[build.delegation.routes]]
match = "Trace how a request is validated before it reaches storage"
kind = "explore"
profile = "trace"
strategy = "prefer_child"

[[build.delegation.routes]]
match = "Implement an independent storage adapter against a stable interface"
kind = "slice"
profile = "default"
```

Each child profile names a `provider/id`. `description` helps the parent choose; optional
`role` supplies guidance for children launched by the Build `task` tool (nonblank inline
text, up to 2,000 characters). It does not change the host-generated `build_review` brief
or grant tools, expand ownership, or override host restrictions. The composed Build
assignment (role, shared context, brief and acceptance check) must fit the existing
32,768-character brief limit. `effort` is optional.
The provider must be known to Corral and approved for the host. For a Build `explore`
child on `openai-codex`, its profile can also set `service_tier = "priority"` (Fast).
Omitting the tier or setting it to `"default"` leaves requests on the normal tier.
The tier doesn't change reasoning effort, doesn't apply to implementation or review
children, and isn't carried to another provider on fallback. For quick, bounded
codebase exploration, start with `effort = "low"` and use `"medium"` for harder tracing.
A named profile with no effort asks Pi for `off`, subject to the model's supported levels. You can configure
`[build.profiles.default]`; otherwise `default` inherits the parent's launch model and effort.
It is also used for delegated work that matches no route.

`standard` (the default) keeps the usual delegation judgment; `proactive` encourages
starting substantial independent slices or useful bounded read-only exploration earlier.
Proactive works without any profiles or routes. For each assignment, the parent picks the
first clearly applicable route in file order, in either mode:

| Route field | Meaning |
|---|---|
| `match` | Description of the scoped work, not a keyword to trigger on. |
| `kind` | `explore` for read-only research or `slice` for implementation. Both use the Build `task` tool. |
| `profile` | A name in `[build.profiles]` or `default`; this is not the standalone Explore tool's `code` or `docs` setting. |
| `strategy` | `when_delegating` (default) picks the kind and profile for work already being delegated. `prefer_child` can justify one meaningful standalone slice or a substantive bounded read-only question. Neither delegates trivial work nor allows overlapping writers. |

Invalid delegation settings or unknown profile references refuse launch. A valid route
can still be unavailable at runtime; the parent reports it rather than silently substituting
another child profile. It can continue inline only when allowed by the existing controls.
To roll back proactive mode, remove `mode` or set it to `"standard"`; routes still apply.
Remove the entire `[build.delegation]` section and its route tables to restore standard mode
without routes. The concurrency cap is independent of this rollback. See [build sessions](build-sessions.md#delegation)
for workflow and ownership.
