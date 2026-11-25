# Release runbook

The current release pipeline is `.github/workflows/release.yml`. The classic `v0.4.0`
tag is historical and has no published release assets; do not move or reuse it. The
published `v0.4.1` release does not contain the Windows desktop installer. Tagging and
publishing a new release require owner authorization.

## Candidate and evidence

The pipeline builds `corral-next`, `corrald-next` and the compiled `corral-pi` host
on native Linux x86_64 and ARM64 (GNU) and macOS Intel and ARM64 runners. It also
builds a portable web app archive and a per-user Windows x86_64 desktop installer.
The Windows installer does not provide a native daemon. The detailed support matrix,
prerequisites and installation paths are in the [app README](../../apps/corral-app/README.md).
Classic release installation remains in the [classic guide](../guide/classic/install.md).

Each archive and installer has a SHA-256 sidecar. Native CI exercises the candidate
archives through the installers, including fresh install, simulated upgrade, checksum
refusal, and runtime probes. Windows checks the web installer and installs, opens,
upgrades, and uninstalls the desktop app. Publication depends on these checks and verifies
the complete asset set and checksums.
A simulated upgrade is not evidence of an upgrade from a separately published release.
Capture the candidate SHA, workflow URL, native job logs and artifact checksums; hosted
platform success cannot be inferred from a local Linux run.

## Dependency refreshes

Refresh the two Cargo lockfiles from their own workspaces to preserve their
respective Rust version constraints. Keep the app's `wasm-bindgen` crate and
the release workflow's `wasm-bindgen-cli` on the same version.

When refreshing the Pi host's Bun lock, keep exactly one
`@smithy/node-http-handler`. Corral's Bedrock transport guard patches that
copy's prototypes, so both Pi and the AWS SDK must resolve it. The manifest
override keeps Pi's exact upstream pin from installing a separate copy.

## Authorized publication

Before publication select a previously unused tag matching `[workspace.package].version`,
refresh both workspace and app lockfiles, verify the candidate and its native CI, and
obtain explicit publication authorization. After landing the authorized candidate, inspect
`git status`, `git rev-parse HEAD`, existing tags and the manifest, then create and push the
matching annotated tag. The tag workflow publishes only after its required jobs pass.
Never force-move a historical tag or publish merely to test installation. After publication,
verify the public asset inventory, checksums, and fresh install and upgrade on each supported
platform, and retain those results with the release evidence.
