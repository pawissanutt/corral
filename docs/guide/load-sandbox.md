# Load sandbox

Intentional load generation on the shared host is forbidden. Workers, reviewers and
gates must use `scripts/load-sandbox.sh` as the **only allowed way to load-test**.
Never start host CPU/memory burners, spin loops, `stress-ng`, or parallel stress
harnesses. If the sandbox is unavailable, report the blocked check; do not fall back
to host load. Ordinary targeted tests do not require artificial contention.

```bash
scripts/load-sandbox.sh --burners 2 -- cargo test -p corral-core
scripts/load-sandbox.sh --cpus 4 --memory 4g --pids-limit 512 --burners 4 -- bun test packages/corral-pi
```

Options precede `--`; the remaining argv is passed unchanged (no host shell eval).
Defaults are 2 CPUs, 2g memory, 256 PIDs, zero burners. CPU quotas above 4, zero or
unlimited memory/PID limits, and invalid values are refused before Podman starts.
Burners are optional Bash loops **inside** the same container as the command, so
all children share its quota. Exit status is preserved. EXIT, INT, TERM and HUP
cleanup forcibly remove the named container, including background test children.
As with any shell trap, SIGKILL or host failure cannot run cleanup; recover with
`podman ps -a --filter label=corral.load-sandbox=true` and remove stranded containers
before another check. Podman must enforce cgroups v2 CPU, memory and PID controllers.

## Files and toolchains

Run from the selected Git worktree. Only that worktree is mounted read-write at its
original absolute path. The Cargo bin directory and Rustup toolchains are mounted
read-only at their original paths; the resolved Bun executable (including npm-installed
Bun) is mounted read-only on the container PATH when available. Toolchains must be
installed on the host beforehand; the sandbox does not auto-install Rust toolchains.
Host credentials, sockets, Cargo configuration/caches outside the worktree, and home
are not mounted. HOME, Cargo's writable cache and CARGO_TARGET_DIR live in the
container and disappear on removal; no shared host target directory is used.
Commands can modify worktree files, so use a disposable worktree for destructive tests.
This is resource isolation, not a sandbox for hostile code.

The default image is `docker.io/library/rust:1.94-trixie` (Bash, glibc and build tools).
Use `--image IMAGE` for a locally provisioned image with additional test dependencies
(e.g. Python, Git, herdr, nextest or strace). Images must provide `/bin/bash` and be
compatible with the mounted host toolchains. Podman may pull missing images; no
host package installation is performed. Network access follows Podman's default;
Cargo may download dependencies into its container-local cache. Keep herdr checks
inside the repository's fresh plugin-free `scripts/test-herdr.sh` environment **in
the container**, never the owner's live server. The wrapper does not change gate
tiers, authorize an extra gate, or replace the run's receipted gate.

## Deterministic smoke checks

```bash
python3 scripts/test_load_sandbox.py
python3 scripts/test_load_sandbox.py --live
```

The first uses a fake Podman to verify preflight refusal above 4 CPUs, argv and
limits, read-only toolchain mounts, exit propagation, partial-create cleanup and
interrupt cleanup. It creates no load. The opt-in live check uses the locally
available `docker.io/library/debian:trixie-slim` image (override with
`LOAD_SANDBOX_TEST_IMAGE`), verifies cgroup limits and mounted Cargo/Rust/Bun,
command failure and SIGTERM cleanup, and compares labeled container inventories
before/after, including CPU refusal. It briefly runs one **container-only** burner.
Readiness is synchronized through stdout, not sleeps, elapsed-time or load-average
assertions. Run live checks without concurrent sandbox invocations so inventory
comparisons are meaningful. An unavailable Podman/controller/toolchain is a failure,
not a reason to run load on the host.
