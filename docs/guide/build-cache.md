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

`CORRAL_MBX_TARGETS` optionally selects `private` (the default) or `managed`.
Managed mode adds bounded, per-checkout target collection; it never shares one
writable target between candidates. Keep this setting in the review policy `env`
with the other cache inputs so gate receipts capture it.

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
- a target or intermediate build directory is inside an mbx-managed target view
  in private mode, or a foreign managed view in managed mode;
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
| `MBX_TARGET_VIEWS` | `0` by default | Private targets; managed-mode exceptions below |
| `MBX_TARGET_ROOT` | `$CORRAL_MBX_CACHE_DIR/corral-disabled-targets` by default | Separate, empty reserved root |
| `MBX_GC_AUTO` | `1` | Automatically collect mbx-owned cache state |
| `MBX_GC_MAX_SIZE` | `20GiB` | Action store budget |
| `MBX_GC_INCREMENTAL_MAX_SIZE` | `10GiB` | Learned incremental state budget |
| `MBX_GC_INCREMENTAL_MAX_AGE` | `14d` | Incremental state age limit |
| `MBX_GC_MAX_TOTAL_SIZE` | `30GiB` | Combined mbx-owned state budget |
| `MBX_LINKER` | `system` | No linker replacement |
| `MBX_SCHEDULER`, `MBX_SCHEDULER_TESTS` | `0` | Cargo and nextest keep their own job and test policy |
| `MBX_SHARE_WORKSPACE_ROOT` | `0` | No workspace-root path remapping |
| `MBX_DISPLAY` | `plain` | Log-friendly output |
| `MBX_SUMMARY` | `full` | Report bypass reasons and per-build compiler timing |

The budgets are logical collection targets, not physical disk quotas. Active builds and
state between collections can exceed them. Private worktree targets are never collected.
Use a dedicated cache: the adapter refuses its reserved disabled-target root if it already
contains records, rather than running collection against them.

### Opt-in managed targets

Set `CORRAL_MBX_TARGETS = "managed"` alongside the three required settings.
Managed mode also requires `b3sum` on `PATH` (already installed on the pilot
host). It computes the pinned native lease identity; a missing or invalid hash
fails before a workflow starts. The managed profile uses these overrides:

| mbx setting | Value |
| --- | --- |
| `MBX_TARGET_ROOT` | `$CORRAL_MBX_CACHE_DIR/corral-managed-targets` |
| `MBX_TARGET_VIEWS` | `1` for fresh default targets and this checkout's existing managed link |
| `MBX_TARGET_LANES` | `0` (retain the existing build/check layout) |
| `MBX_TARGET_MAX_SIZE` | `200GiB` across managed targets |
| `MBX_TARGET_MAX_AGE` | `7d` unused |
| `MBX_GC_MAX_TOTAL_SIZE` | `230GiB`, including targets, action and incremental state |

Each eligible checkout gets its own `target` symlink into the managed root.
Existing real `target/` directories, unrelated symlinks and explicit target
choices stay private: the adapter disables automatic adoption for those builds.
Consequently, enabling this mode does **not** reclaim existing private targets.
Migration or deletion of those targets requires a separate owner decision.

`scripts/gate.sh`, `scripts/work-budget.sh` and `scripts/pi-lane.sh` hold a
shared native mbx target lease across their whole workflow, not just compilation.
This protects prebuilt nextest binaries and the gaps between commands. The helper
takes the lease **before** mbx places or restores a target; it creates no target
directory itself and moves no existing outputs. The native key is the BLAKE3 hash
of the canonical workspace path. Checking that key also rejects another
checkout's dangling link when collection has deleted its ownership record.
The helper validates ownership and fails rather than starting an unprotected
managed workflow.

The lease uses the Linux file-lock protocol of pinned mbx 1.29.2
(`crates/mbx/src/target.rs` upstream). It is not a public lease CLI: `mbx reserve`
reserves scheduler capacity, and `mbx exec` does not provide this Rust target
lease. An upgrade must requalify the protocol. Locks are released on exit;
inherited descriptors keep surviving workflow children protected after a
supervisor crash. Never unlink lease files.

After a workflow finishes, its target is collectible. Any later manual command
that reuses prebuilt outputs must also hold a lease; a sequence of bare Cargo or
binary invocations does not acquire whole-workflow protection. For a custom
Bash workflow, use `python3 scripts/cargo_cache.py --protect-script /absolute/workflow.sh`;
the script receives `--corral-mbx-protected` as its first argument and must shift
it, then run `python3 /absolute/scripts/cargo_cache_targets.py --verify-protection`
from the workspace root before consuming outputs. A single adapter Cargo command
uses mbx's own per-command lease.

Automatic collection can evict inactive live checkouts under budget pressure;
it does not promise every retained checkout a warm target. Active leases can
prevent meeting the budget temporarily. Managed-mode collection sees only its
managed root, never private directories or the owner's general mbx store.

### Rust flags and the linker

In mbx mode, when neither `RUSTFLAGS` nor `CARGO_ENCODED_RUSTFLAGS` is explicitly
supplied, the adapter sets `CARGO_ENCODED_RUSTFLAGS` to an empty string. This takes
precedence over Cargo's target/build rustflags arrays, including flags inherited from
`CARGO_HOME/config.toml`; an empty project-level array alone would not override them.
No Cargo configuration file is modified. Plain-Cargo mode is unchanged.

This avoids the observed mbx 1.29.2 `unknown-codegen-option` bypass caused by inherited
`-C linker-features=-lld`. An inherited `-C link-arg=-B...` also prevents native-link
caching. Without these overrides the default x86-64 Linux Rust toolchain uses its
bundled LLD through the system compiler driver, rather than the owner's custom mold
flags. The adapter does not install a linker or require clang.

Explicit `RUSTFLAGS` and `CARGO_ENCODED_RUSTFLAGS` are preserved, including empty
values and Cargo's encoded-flags precedence. Put required flags in the review policy
`env` so they are captured in gate identity; incompatible explicit flags can still
cause bypasses. This default also masks repository and command-line Cargo rustflags
arrays: pass required sanitizer, CPU, cfg or linker flags explicitly via the environment.
A flag change produces new cache keys, so the first gate can rebuild; improved cache
eligibility is not a measured wall-time speedup.

## Private targets

In the default private mode, every worktree builds into its usual target directory. mbx restores artifacts into it,
but Corral never shares a `CARGO_TARGET_DIR` or build dir between candidates, so one
worktree's build can't overwrite another candidate's tested binaries. An explicit
`cargo build --target-dir` or `CARGO_TARGET_DIR` still works and carries the same external
serialization rule as before (see AGENTS.md, "Build and test").
If `CARGO` is set, use the absolute path of the real Cargo launcher. The adapter uses
that same launcher to check paths and run mbx. A rustup Cargo proxy is supported; keep
the `cargo` proxy path, rather than its resolved `rustup` executable.

In private mode, a target that an earlier mbx setup adopted as a managed view is
rejected, not converted. Managed mode accepts only its dedicated root and matching
checkout ownership. Neither mode converts an existing foreign managed view.

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

The gate logs the selected profile and mbx reports cache hits, misses, bypass reasons
and compiler timing on stderr with the full summary. Compiler time avoided is summed
across actions, not wall time saved. Routine probes and rustdoc bypasses are expected.
To inspect the dedicated store without changing it:

```sh
MBX_CACHE_DIR="$CORRAL_MBX_CACHE_DIR" \
MBX_TARGET_ROOT="$CORRAL_MBX_CACHE_DIR/corral-disabled-targets" \
  "$CORRAL_MBX_BIN" cache stats --json
```

For managed-mode statistics, replace `corral-disabled-targets` above with
`corral-managed-targets`; the target root matters for both statistics and collection.

The adapter's fast safety regressions run with `python3 scripts/test_cargo_cache.py`.
They also run from the existing `python3 scripts/test_gate.py` entry point.
The real-mbx managed-target journey uses only disposable checkouts and a temporary
cache, never the configured live store:

```sh
CORRAL_TEST_MBX_BIN=/absolute/path/to/mbx python3 scripts/test_cargo_cache_managed.py
```

It verifies distinct candidate binaries, collection during the deliberate gap
between compilation and execution, and collection after the workflow leases end.
Without the explicit binary setting, this standalone test skips.

## Rolling back

Set `CORRAL_BUILD_CACHE = "off"` in the review policy (or remove the key) and run a fresh
gate. Changing the policy changes the captured inputs, so old receipts don't carry over.
Don't delete the shared cache as a first step. For a managed checkout, choose a
fresh private target (`CARGO_TARGET_DIR=/absolute/private/path`) before disabling
managed mode or switching to plain Cargo; do not leave a bare multi-command workflow
using a collectible managed link. No rollback command unlinks or deletes targets.

mbx can restore hardlinked, read-only outputs into a target. If plain Cargo trips over one
after rollback, build into a fresh private target. Ask before removing an old target, and
never chmod objects in the shared cache.

## What the pilot doesn't do

- It doesn't install mbx, write global or workspace mbx config, or add a Cargo shim.
- It doesn't adopt, move, clean up or delete existing private targets. Old private
  worktree targets stay as large as they are. Managed mode creates fresh managed
  targets and collects targets already owned by its dedicated managed root.
- It doesn't collect private worktree targets or reclaim old private worktree targets.
- It doesn't intercept bare `cargo` commands.
- It doesn't change which tests run, features, the toolchain or scheduling.
- It covers Corral's own builds only, on Linux/WSL. Other platforms and other projects
  Corral manages stay on plain Cargo.

Reuse was measured on a small pure-Rust fixture (four of four compiler actions restored
in a second checkout, roughly 31% less allocation across three targets). A real Corral
gate with proc macros and native build scripts hasn't been benchmarked yet, so expect hit
rates to vary with feature unions, compiler versions and generated code.
