# Build cache (mbx pilot)

Corral's Rust builds can share compiler outputs between worktrees through
[mbx](https://github.com/jdx/mr-boxington). It's an opt-in pilot. Unless you turn it on,
every build is plain Cargo.

## How builds reach the adapter

`scripts/cargo.sh` takes Cargo arguments verbatim and picks a backend from
`CORRAL_BUILD_CACHE`:

| Value | Behavior |
| --- | --- |
| unset or `off` | `exec cargo "$@"`. No mbx lookup, no cache state. |
| `mbx` | Runs `scripts/cargo_cache.py`, which validates the profile and execs mbx. |
| anything else | Fails with exit status 2. |

The gate compiles through this adapter: nextest binary preparation, Clippy, the
platform `cargo check` lanes, doctests and the final `cargo build -p corral`. So do
the standalone branch of `scripts/work-budget.sh` and the Pi lane's Rust compilation.
Prebuilt nextest runs keep using the metadata the gate captured and don't recompile.

In mbx mode, help, version and the `search`, `login`, `logout`, `owner` and `yank`
commands go directly to Cargo after the basic profile checks. They don't need a workspace
and don't use the build cache.

Bare `cargo` commands are **not** intercepted. The adapter isn't a PATH shim or a
`RUSTC_WRAPPER`. A bare `cargo build` still works, it just skips the shared cache. To
share artifacts from a manual or agent command, call the adapter directly:

```sh
bash scripts/cargo.sh build -p corral
bash scripts/cargo.sh test -p corral-core
```

## Settings in mbx mode

All three settings are required when `CORRAL_BUILD_CACHE=mbx`:

| Variable | Requirement |
| --- | --- |
| `CORRAL_BUILD_CACHE` | `mbx` |
| `CORRAL_MBX_BIN` | Absolute path to an executable whose `--version` prints exactly `mbx 1.29.2` |
| `CORRAL_MBX_CACHE_DIR` | Absolute path outside the Cargo workspace, shared by every participating worktree |

For Linux x86-64, the pinned upstream release can be installed without a global
Cargo shim:

```sh
dir="$HOME/.local/opt/corral-mbx-1.29.2"
mkdir -p "$dir"
curl -fL https://github.com/jdx/mr-boxington/releases/download/v1.29.2/mbx-x86_64-unknown-linux-gnu.tar.gz -o "$dir/mbx.tar.gz" &&
(
  cd "$dir" || exit 1
  printf '%s  %s\n' '73e135d3e9e8b8ddc938b42407179a239917b7f02d3fa5f601f6783e5c0d36d6' mbx.tar.gz | sha256sum --check &&
  tar -xzf mbx.tar.gz
) &&
"$dir/mbx" --version
```

Set `CORRAL_MBX_BIN` to the absolute path printed by `printf '%s/mbx\n' "$dir"`.
The pinned checksum is for this one Linux x86-64 archive, not other platforms.

Pick a dedicated Corral cache directory on the same filesystem as your worktrees. Don't
point it at your general mbx store or under the temporary HOME/XDG directories created
by `scripts/test-herdr.sh`. An absolute path under your normal home directory is fine;
the temporary test environment must still be given that same absolute path.

Before the first build, the adapter refuses to start when:

- the host isn't Linux (WSL counts as Linux);
- any `MBX_*` variable is already set in the environment;
- `$XDG_CONFIG_HOME/mbx/config.toml` exists (or `~/.config/mbx/config.toml` when
  `XDG_CONFIG_HOME` is unset or relative);
- the workspace root has a `.mbx.toml`;
- the cache directory is inside the workspace;
- a target or intermediate build directory is inside an mbx-managed target view;
- the profile's reserved managed-target root already contains target records, cannot be
  read, or is a symlink (including its `v1` directory);
- the selected Cargo launcher is an mbx or mise command wrapper, or an explicitly
  supplied `CARGO` is not an absolute executable path;
- the requested Cargo subcommand is an alias or uses characters other than ASCII
  letters, digits, hyphens and underscores. Invoke the direct command instead;
- applicable Cargo configuration contains includes, or a directory-changing or unstable
  Cargo option (`cargo -C`, `cargo --directory` or `cargo -Z`) is used. Cargo discovers configuration
  from the current directory and its parents, plus `CARGO_HOME`, not from
  `cargo build --manifest-path` alone.

Each failure starts with the prefix corral cargo cache: and exits with status 2. Builds do
not silently fall back to Cargo, and an mbx build error stays a failed command.

Global and workspace mbx configuration aren't supported yet, and neither are your own
`MBX_*` overrides. The adapter sets the whole profile itself:

| mbx setting | Value | Why |
| --- | --- | --- |
| `MBX_CACHE_DIR` | `$CORRAL_MBX_CACHE_DIR` | The shared store |
| `MBX_TARGET_VIEWS` | `0` | Each worktree keeps its own private target directory |
| `MBX_TARGET_ROOT` | `$CORRAL_MBX_CACHE_DIR/corral-disabled-targets` | Separate, empty managed-target root |
| `MBX_GC_AUTO` | `1` | Automatically collect mbx-owned cache state |
| `MBX_GC_MAX_SIZE` | `20GiB` | Action store budget |
| `MBX_GC_INCREMENTAL_MAX_SIZE` | `10GiB` | Learned incremental state budget |
| `MBX_GC_INCREMENTAL_MAX_AGE` | `14d` | Incremental state age limit |
| `MBX_GC_MAX_TOTAL_SIZE` | `30GiB` | Combined mbx-owned state budget |
| `MBX_LINKER` | `system` | No linker replacement |
| `MBX_SCHEDULER`, `MBX_SCHEDULER_TESTS` | `0` | Cargo and nextest keep their own job and test policy |
| `MBX_SHARE_WORKSPACE_ROOT` | `0` | No workspace-root path remapping |
| `MBX_DISPLAY` | `plain` | Log-friendly output |

The budgets are logical collection targets, not physical disk quotas. Active builds and
state between collections can exceed them. They do not cap or collect worktree targets.
Use a dedicated cache: the adapter refuses its reserved managed-target root if it already
contains records, rather than running collection against them.

## Private targets

Every worktree builds into its usual target directory. mbx restores artifacts into it,
but Corral never shares a `CARGO_TARGET_DIR` or build dir between candidates, so one
worktree's build can't overwrite another candidate's tested binaries. An explicit
`cargo build --target-dir` or `CARGO_TARGET_DIR` still works and carries the same external
serialization rule as before (see AGENTS.md, "Build and test").
If `CARGO` is set, use the absolute path of the real Cargo launcher. The adapter uses
that same launcher to check paths and run mbx. A rustup Cargo proxy is supported; keep
the `cargo` proxy path, rather than its resolved `rustup` executable.

A target that an earlier mbx setup already adopted as a managed view is rejected, not
converted. Move to a fresh private target yourself; the adapter won't unlink or adopt it.

## Turning it on for gates

Put the settings in the Corral project's review policy `env`. Corral captures those values
as verification inputs and replays them when it runs the gate, so the mode is part of the
receipt instead of whatever a pane happened to export:

```toml
[projects.corral]
path = "."

[projects.corral.review]
env = { CORRAL_HERDR_SESSION = "corral-test", CORRAL_BUILD_CACHE = "mbx", CORRAL_MBX_BIN = "/home/you/.local/opt/corral-mbx-1.29.2/mbx", CORRAL_MBX_CACHE_DIR = "/home/you/corral-build-cache" }
```

Use your existing project key and path, and keep any `env` keys you already have.
The cache and executable paths above are examples. Check the binary first:

```sh
/home/you/.local/opt/corral-mbx-1.29.2/mbx --version   # must print: mbx 1.29.2
```

For a one-off manual build, export the same three variables in your shell and run
`bash scripts/cargo.sh ...`.

## Diagnostics

The gate logs the selected profile and mbx reports cache hits, misses and bypasses on
stderr. To inspect the dedicated store without changing it:

```sh
MBX_CACHE_DIR="$CORRAL_MBX_CACHE_DIR" \
MBX_TARGET_ROOT="$CORRAL_MBX_CACHE_DIR/corral-disabled-targets" \
  "$CORRAL_MBX_BIN" cache stats --json
```

The adapter's fast safety regressions run with `python3 scripts/test_cargo_cache.py`.
They also run from the existing `python3 scripts/test_gate.py` entry point.

## Rolling back

Set `CORRAL_BUILD_CACHE = "off"` in the review policy (or remove the key) and run a fresh
gate. Changing the policy changes the captured inputs, so old receipts don't carry over.
Don't delete the shared cache as a first step.

mbx can restore hardlinked, read-only outputs into a target. If plain Cargo trips over one
after rollback, build into a fresh private target. Ask before removing an old target, and
never chmod objects in the shared cache.

## What the pilot doesn't do

- It doesn't install mbx, write global or workspace mbx config, or add a Cargo shim.
- It doesn't adopt, move, clean up or delete existing targets. Old worktree targets stay
  as large as they are.
- It doesn't collect private worktree targets or reclaim old worktree targets.
- It doesn't intercept bare `cargo` commands.
- It doesn't change which tests run, features, the toolchain or scheduling.
- It covers Corral's own builds only, on Linux/WSL. Other platforms and other projects
  Corral manages stay on plain Cargo.

Reuse was measured on a small pure-Rust fixture (four of four compiler actions restored
in a second checkout, roughly 31% less allocation across three targets). A real Corral
gate with proc macros and native build scripts hasn't been benchmarked yet, so expect hit
rates to vary with feature unions, compiler versions and generated code.
