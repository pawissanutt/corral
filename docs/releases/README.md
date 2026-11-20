# Release runbook

The current release pipeline is `.github/workflows/release.yml`. The classic `v0.4.0`
tag is historical and has no published release assets; do not move or reuse it. The current workspace
version is `0.4.1`, a new candidate tag, not a claim that it has been published.
Tagging and publishing are external writes and require separate owner authorization.

## Candidate and evidence

The pipeline builds `corral-next`, `corrald-next` and the compiled `corral-pi` host
on native Linux x86_64 and ARM64 (GNU) and macOS Intel and ARM64 runners. It also
builds one portable web app archive, and tests its installer on Windows. Native
Windows daemon and native app release packages are not provided. The detailed support
matrix, prerequisites and installation paths are in the [app README](../../apps/corral-app/README.md).
Classic release installation remains in the [classic guide](../guide/install.md).

Each archive has a SHA-256 sidecar. Native CI exercises the actual candidate archives
through the installers, including fresh install, simulated upgrade, checksum refusal,
and runtime probes; Windows exercises the web installer. Publication depends on the
build and smoke jobs, and verifies the expected archive checksums. A simulated previous
installation is not evidence of an upgrade from a separately published current release.
Capture the candidate SHA, workflow URL, native job logs and artifact checksums; hosted
platform success cannot be inferred from a local Linux run.

## Authorized publication

Before publication select a previously unused tag matching `[workspace.package].version`,
refresh both workspace and app lockfiles, verify the candidate and its native CI, and
obtain explicit publication authorization. After landing the authorized candidate, inspect
`git status`, `git rev-parse HEAD`, existing tags and the manifest, then create and push the
matching annotated tag. The tag workflow publishes only after its required jobs pass.
Never force-move a historical tag or publish merely to test installation. After publication,
verify the public asset inventory, checksums, and fresh install and upgrade on each supported
platform, and retain those results with the release evidence.
