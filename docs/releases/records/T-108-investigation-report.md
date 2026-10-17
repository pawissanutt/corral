# T-108 distribution investigation

Date: October 17, 2026. Branch: `corral/T-108`.
Inspected candidate: `3994c321f498942828aa712ba7d4c6b2da364336`.

## Existing distribution

The existing `.github/workflows/release.yml` builds Linux musl and macOS
archives for x86_64 and aarch64 when a `v*` tag is pushed. It uploads each
archive with an adjacent SHA-256 file and immediately creates a GitHub release.
There is no pull-request validation trigger, Windows target, or installer smoke
test in that workflow. Extend this workflow rather than add a second release
pipeline.

The existing `install.sh` detects Linux/macOS and both architectures, supports
`CORRAL_VERSION`, verifies the downloaded checksum, and installs into
`~/.local/bin` by default. It offers consent-controlled herdr installation.
`scripts/install-test.sh` contains isolated Unix installer tests.
There is no PowerShell installer.

The manifest still declares version `0.1.0`. GitHub inspection found the
published `v0.1.0` release dated October 14, 2026, with four archives and four
adjacent checksum files. Its successful release workflow run is
`38579665027`, for commit `d2650bb8c0d973254748d8196c67d05fbc60c819`.
That historical success is not native-platform evidence for the current tree.

## Blocking portability findings

Packaging changes alone cannot deliver the requested native binaries from
the current source:

- `crates/corral-ops/src/cleanup_worktree.rs:4` unconditionally imports Unix
  `OsStrExt`. Process residency discovery in the same file reads `/proc`.
- `crates/corral-ops/src/gate.rs:2` unconditionally imports Unix
  `OpenOptionsExt`; gate ownership reads `/dev/urandom` and creates a
  mode-restricted file.
- `crates/corral-ops/src/hook_command.rs:4` unconditionally imports Unix
  process extensions. Hook waiting uses Linux pidfds; orphan detection reads
  `/proc/*/environ`.
- `crates/corral-ops/src/watchdog.rs:28` implements process inspection using
  Linux `/proc` files, and termination uses Linux pidfds.

These are production paths, not only platform-specific tests. Native Windows
needs actual implementations for these contracts. macOS also needs replacements
for the Linux process-identity and pidfd operations. Disabling these safety
mechanisms or returning success without performing them would not be a valid
distribution fix.

Primary references:

- https://docs.rs/rustix/latest/src/rustix/process/pidfd.rs.html
- https://herdr.dev/docs/install/
- https://herdr.dev/docs/windows-beta/

Herdr itself documents native Windows distribution; the identified blocker is
corral's current runtime code, not an assumption that herdr is Unix-only.

## Owner decisions needed

1. Expand T-108 beyond distribution to implement native Windows/macOS process
   control, cleanup and gate/hook primitives, or split that prerequisite into
   separate work before completing T-108. WSL is not a substitute for the
   acceptance requirement of native Windows CI.
2. Choose the next release version/tag. `v0.1.0` already exists; no version bump,
   replacement tag, or new publication has been chosen.
3. Authorize the eventual branch push and native GitHub Actions run, and specify
   whether publication should remain an owner-executed step after validation.
   No remote writes have been performed.

## Checks and remaining work

Read repository guidance, workflows, installer and installer tests, manifests,
and the production portability paths listed above. Inspected GitHub
authentication, workflow inventory, release inventory, asset names and previous
workflow result with read-only `gh` commands. Searched existing corral records
before investigating.

No source changes, builds, tests, gate, native CI runs, artifact downloads,
checksum verification or installation smoke checks have been performed.
Only this investigation report is changed. All implementation and acceptance
verification remain pending. No servers, background jobs, or cleanup obligations
were created. Do not call `corral done` on this investigation.
