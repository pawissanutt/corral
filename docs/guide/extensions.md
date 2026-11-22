# Extensions

## Configured commands (`corral-next`)

In `.corral-next/corral.toml`, name an extension and give it one or more shell commands:

```toml
[extensions.example]
projects = ["site"]  # omit for every project
facts = "my-indexer facts"
on = ["landed"]
run = "my-notifier"

[extensions.example.tools]
context = { run = "my-indexer context", about = "Find starting points" }
```

`enabled` defaults to `true`. `corral-next config check` validates the block without
running anything and rejects overlapping enabled `facts` providers for a project.

`corral-next ext check example [--project site]` runs each line once, even for a disabled
extension. **This runs real commands and can have side effects.** The project comes from
`--project`, then `CORRAL_PROJECT`, then the first eligible project by name. If no project
is eligible (for example, `projects = []`), omit `--project` to run from the workspace root;
any inherited `CORRAL_PROJECT` is ignored. Each line gets one JSON object with `v: 1` and
`sample: true` on stdin and runs with a 10-second deadline and a 1 MiB stdout limit. The
check prints each line's stdout or error and exits nonzero if any line fails. Configuring
an extension does not run it during normal task operations.

## PATH commands (`corral`)

The older `corral` CLI can run external commands named `corral-ext-<name>` that you install on `PATH`.
This is the first stage of the extension framework in the approved T-381 plan: command
discovery only. Corral does not register, configure, sandbox or talk a protocol to these
commands; it starts them exactly as a shell would start any other program.

```sh
corral ext list
corral ext list --json
corral ext run <name> [args...]
corral <name> [args...]
```

## Forwarding

`corral <name> args...` runs `corral-ext-<name>` when `<name>` is not a built-in command or
a reserved name. Corral replaces itself with the extension (`execvp`-style), so:

- the arguments after `<name>` reach the extension exactly as given, with no shell
  interpretation;
- the extension inherits the terminal: stdin, stdout and stderr are corral's own;
- the extension's exit status is corral's exit status.

A leading global `--root <ROOT>` is consumed by corral and not forwarded. If no
`corral-ext-<name>` is on `PATH`, you get clap's ordinary unknown-subcommand error (exit 2),
the same as before extensions existed.

Extension names contain only ASCII letters, digits, `-` and `_` and start with a letter or
digit. Only absolute `PATH` directories are searched, in order; empty and relative entries
(which would mean the current directory) are skipped. The first executable regular file
wins.

## Built-in and reserved names

A built-in command always wins: `corral list` runs the built-in list even when
`corral-ext-list` is on `PATH`. Reserved names never fall through to an extension either.
They are every built-in subcommand (including `ext` and `help`) plus names proposed or
announced for future built-ins: `accept`, `migrate`, `read` and `report`. One table in
`crates/corral/src/ext_cmd.rs` lists them, and a test fails if a new built-in is missing
from it.

`corral ext run <name> args...` is the collision-proof form. It runs `corral-ext-<name>`
from `PATH` whatever the name, so `corral ext run list --json` runs `corral-ext-list --json`.
`<name>` is the forwarding boundary: every argument after it reaches the extension
unchanged, including flags corral itself recognizes, so `corral ext run list --help` passes
`--help` to the extension. Corral's own options (`--root`, `--help`) go before `<name>`.
When nothing matches it exits 1 with an error naming the missing executable.

## Listing

`corral ext list` prints every executable `corral-ext-*` on `PATH` with its full path, in
`PATH` order. It reads only directory listings and file metadata and never executes a
candidate. A candidate with a reserved name is marked as reachable only through
`corral ext run`. A later candidate with the same name is marked as shadowed by the earlier
`PATH` entry. `--json` prints an array of `{name, path, reserved, shadowed}` objects.

Registrations (`corral-extension.toml` manifests recorded by an explicit owner command) are
a later stage of the plan; until then the list shows `PATH` candidates only.
