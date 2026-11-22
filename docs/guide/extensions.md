# Extensions (corral-next)

An extension is an owner-trusted set of shell command lines in
`.corral-next/corral.toml`. There is no registration or sandbox: commands have the
owner's permissions. Use only commands you trust. For example, with this repository as
the project (replace the absolute paths with paths in your checkout):

```toml
[extensions.examples]
projects = ["corral"] # omit for all projects; [] selects none
# enabled = true        # default
facts = "python3 '/absolute/path/to/corral/examples/extensions/facts.py' tags"

[extensions.examples.tools]
context = { run = "python3 '/absolute/path/to/corral/examples/extensions/context.py'",  about = "Find task starting points", record = true }
```

`projects` restricts eligibility; `enabled = false` disables normal use. Tool names and
the `facts` slot must not have overlapping enabled providers for a project. Tool `about`
and `record` are optional; `record` defaults to false. `corral-next config check` validates
configuration without executing it. React's `on` and `run` fields deliver events
(see React below).

## Rules for every line

The runner is Unix-only. Each configured command string must be nonblank and contain no
NUL. Corral runs each line using `sh -c`, sends one JSON object on stdin followed by EOF,
and reads stdout. Requests include `"v": 1`; ignore unknown fields. Success requires
exit status zero; failures report the stderr tail (up to 4 KiB). Each invocation has a
deadline and stdout limit; a timed-out process group is killed. Commands receive the
project environment plus `CORRAL_NEXT_ROOT`, `CORRAL_PROJECT` when applicable,
`CORRAL_EXTENSION` and `CORRAL_ACTOR=extension`. An extension can write back using the
public `corral-next` CLI. Commands may have side effects.

`corral-next ext check examples [--project corral]` actually runs each configured line,
even when disabled, with `sample: true`; it uses a 10-second deadline and 1 MiB stdout
limit per line. It reports outputs or errors, not protocol conformance. It selects the
explicit `--project`, otherwise an inherited `CORRAL_PROJECT`, otherwise the first eligible
project by name, and runs from that project's repository. The selected project must be
eligible; an inherited out-of-scope project therefore fails the check. Without an explicit
project and with no eligible project, it ignores the inherited selector and runs from the
workspace root. Sample tool requests contain only `v`, `sample`, and an empty `args`
array; sample facts requests use `query: snapshot` and `rev: HEAD`.

## Tool

`corral-next tool` lists eligible tools. Run one with
`corral-next tool --project=corral -- context [args...]` (omit `--project` when context
selects the project). The line runs in the caller's working directory, with a 60-second
deadline and 1 MiB stdout limit. Its stdout is passed through unchanged, including under
`--json`. When `record = true` and a task and run are present, the first successful
output per tool and run is recorded as an artifact. Configured tools are listed in agent
run prompts; dispatch does not run them.

A normal request looks like:

```json
{"v":1,"args":["extra"],"role":"worker","repo":"/absolute/worktree","base":"commit","head":"submitted-commit","task":{"id":"T-1","title":"Fix login","body":"Details","parent":null,"depends_on":[]},"run":"R1"}
```

`task`, `run`, and `base` can be null outside a run. `head` is the worker's submitted
commit (also for a reviewer run), or null when there is none. When present, a tool must use `head`, not the
worktree's mutable `HEAD`, to inspect the submitted change. `role` reflects the caller;
`repo` is the run worktree or project repository. The answer is arbitrary stdout, not JSON.

The included `context.py` searches tracked worktree files for literal words from the task
title using `git grep`, returning at most 100 lines. Without a task title it reports that
there is nothing to search. It needs Python 3 and Git, not an index service.

## Ask: facts

`facts` is an optional slot for enriching built-in system facts. The facts reader supports
provider calls, but normal daemon/CLI system-map integration is not yet wired here. It
keeps built-in facts when the provider fails. A call runs from the project repository and
has a 10-minute deadline and 64 MiB stdout limit. Requests are:

```json
{"v":1,"query":"snapshot","repo":"/absolute/repo","rev":"commit","want":["files","definitions","references"]}
{"v":1,"query":"delta","repo":"/absolute/repo","base":"commit","head":"commit","want":["changes","dependents","tests"]}
```

Answers are UTF-8 JSON Lines: one object per line, ending in exactly one `done` row.
All rows must be valid, even for unrequested categories; malformed output, a missing
`done` or command failure discards the answer. Only requested categories listed in
`done.provided` contribute; an absent category means unavailable, not empty.

| `type` | Fields |
|---|---|
| `file` | `path`, `read` (`complete`, `unsupported`, `failed`) |
| `definition` | `path`, `name`, `line`, `end_line`, `kind` |
| `reference` | `from`, `to`, `kind` (`import`, `use`, `call`), `count` |
| `change` | `change` (`added`, `removed`, `changed`), `fact` (`definition` or `reference`), `path`, `name`, `line`; reference changes also need `to_path`, `to_name` |
| `dependent` | `path`, `name`, `line`, `of` |
| `test` | `path`, `name` |
| `done` | `provided` (array), `skipped` and `omitted` (reason-to-count objects) |

Lines are one-based, end lines cannot precede start lines, and reference counts are
positive. Paths (including reference `from` and `to`) are exact repository-relative file
paths in the snapshot, or the base for removed delta changes and the head otherwise;
unknown paths are dropped. A dependent's `of` is the full name of the changed definition.
Providing references replaces built-in syntactic references, even when the provider returns
none. Results are cached by tree and provider configuration; inputs should correspond to the
requested revision.

`examples/extensions/facts.py` reads a tracked ctags-format file at the requested Git
revision, not the mutable worktree. It supports numeric line addresses (other addresses
and unsafe paths are counted as skipped), reports only `definitions`, and leaves delta
categories unavailable.
Pass a repository-relative tags path as its first argument; the default is `tags`. Keep the
tags file updated and committed with the code it describes. For a sample check it emits an
empty definitions answer without reading the file.

### Try the examples without configuring Corral

Run these from this repository; the facts fixture uses an isolated temporary Git repository:

```sh
python3 examples/extensions/test_examples.py
```

This exercises both scripts with actual JSON requests and checks their output. You can also
configure them as above and use `corral-next ext check examples` to exercise the runner.

## React: events (`on` and `run`)

The daemon runs an enabled extension's `run` line once per event whose kind is in `on` (every
kind when `on` is omitted) and whose task's project is in `projects` (with `projects` omitted,
events without a project match too). Each event is one JSON object on stdin: `v`, `seq`, `ts`,
`task`, `project`, `run`, `kind`, `actor`, `detail`. Events go in sequence order, one at a
time per extension, with a 60-second deadline. No engine operation waits for a delivery.

Each extension has a cursor, `.corral-next/extensions/<name>.json`, holding the last
sequence it finished. A newly configured extension starts at the newest event when a pass
first reads the configuration containing it, before that pass runs any line, and does not
replay history. A disabled or removed extension keeps its cursor and resumes from it when
enabled or added again under the same name.

Delivery is at least once: if the daemon stops during a delivery, or before saving the
cursor after one, that event is delivered again, so a line should tolerate a repeat.
Attempts never overlap. Each line runs under a small supervisor, a copy of
the daemon or CLI executable, that holds the extension's delivery lock. When the line's
shell exits, when the 60-second deadline passes, or when the daemon or CLI that started it
dies, the supervisor kills the line's process group, including anything the line left
running there, and only then releases the lock. After a crash the event is therefore
delivered again only once the interrupted attempt has stopped. A process that leaves the
line's group (for example with `setsid`) is not killed and does not hold up delivery.
If the supervisor itself is killed, the lock is released at once; on Linux its shell dies
with it, but processes the shell started may keep running.

A failing event blocks later events for that extension and is retried on a later pass (the
daemon waits 30 seconds); after three failures it is skipped and counted. The `settings`
topic lists each extension with its shapes, whether it is enabled, its last error and its
skipped count.

Without a daemon nothing is delivered. `corral-next ext deliver` runs one pass, for CLI or
cron setups; it retries a failed event at once and exits nonzero while an event is failing.
A per-extension lock keeps it and the daemon from delivering the same extension at once;
the one that finds it held reports the extension as busy.
