# Release runbook

## Publication state and decisions

The owner selected **`v0.3.0`** and the manifest declares `0.3.0`.
Tags `v0.1.0` and `v0.2.0` are already published. The owner has authorized native
Windows/macOS runtime ports and pushing only `corral/T-108` for native
Actions validation. This does not authorize a force-push, remote tag,
GitHub Release, package publication, or shared-main write.

Before publication, obtain explicit tag/publication authorization separately.
Task-branch validation and the instructions below do not grant that permission.
Do not replace `v0.1.0`, reuse its
historical green workflow as candidate evidence, or silently publish to test
the installer.

## One pipeline, five native targets

`.github/workflows/release.yml` is the existing release pipeline, extended
to run validation on pull requests, pushes to main or `corral/**` task
branches, and manual dispatch.
Tag pushes also run validation and packaging, then publish only after all
checks succeed.

| Target | Native runner | Package |
|---|---|---|
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `.tar.gz` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `.tar.gz` |
| `x86_64-apple-darwin` | `macos-15-intel` | `.tar.gz` |
| `aarch64-apple-darwin` | `macos-15` | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `windows-2022` | `.zip` |

Linux build jobs install musl tools. Validation installs AppArmor, bubblewrap,
strace, and ShellCheck. It loads a bubblewrap-specific namespace permission
from `scripts/ci-bwrap.apparmor` and requires a read-only sandbox probe; the
host-wide user-namespace restriction is unchanged.
Python 3.12 performs packaging and
installer proof, and Rust stable compiles/tests the project; neither is a
consumer installation dependency. macOS deployment target is 14.0. The
Linux validation job runs the repository's full gate in a fresh plugin-free
herdr environment, plus installer regressions and gate-fixture tests.
Native jobs run core/Git, engine-adapter, CLI protocol, and focused native
runtime tests before building the packaged binary.

Package basename: `corral-<target>`. Each archive contains the executable,
`LICENSE`, and `README.md`, and has a sidecar `<archive>.sha256` containing
SHA-256 and the exact archive basename.

Each native job runs `scripts/release-install-test.py` against its actual
packaged binary. A loopback HTTP fixture presents both latest and pinned
release paths to the real installer. Installation runs in a clean user
directory with cargo/rustc and herdr absent from its PATH and stdin closed.
The installed executable must pass `--version`. The proof also exercises
updates, missing downloads/checksums, checksum mismatch, invalid archives,
unusable executables, and unsupported architectures while verifying the
previous executable is unchanged.

This is pre-publication asset/installer proof, not a claim that an
unpublished public GitHub release can already be downloaded.

## Evidence to retain

Workflow artifact `gate-log` contains the Linux gate output. Each target
artifact contains:

- its archive and SHA-256 sidecar;
- `<target>.json` with version, source revision, native runner OS/architecture, archive name
  and digest;
- `installer-proof-<target>.json` with actual installer exit codes, output,
  clean-install version and failure-case observations.

When all jobs succeed, capture the workflow run URL and candidate SHA,
download the artifacts, verify every sidecar, and attach the logs/manifests
and results to the task. For the published release also capture the tag,
release URL, asset inventory and downloaded checksum verification. Failed
jobs upload whatever package evidence exists but never permit publication.
Native Windows/macOS results cannot be inferred from Linux cross-checks.
The selected release notes are in [v0.3.0.md](v0.3.0.md).

## Prepare the selected version locally

For the owner-selected `v0.3.0`:

1. Update `[workspace.package].version` in `Cargo.toml` and refresh the
   workspace packages in `Cargo.lock` with Cargo. Keep crate versions
   inherited from the workspace.
2. Update installation documentation's preparation note to the chosen
   release and record release notes covering supported targets and runtime
   prerequisites.
3. Run the isolated full gate, commit on the task branch, rebase onto current
   main, and run the gate again on the rebased candidate. Follow `AGENTS.md`
   for isolation; never attach tests to the owner's herdr server.

## Native CI before publishing

The owner-authorized T-108 validation push is limited to this task branch:

```sh
git push -u origin corral/T-108
```

That task-branch push triggers the native validation matrix directly, without
creating a tag, release or pull request. Later manual dispatches can use
`gh workflow run release.yml --ref corral/T-108` after the workflow's
`workflow_dispatch` trigger exists on the default branch. Do not work around
an unavailable manual trigger by pushing a release tag.

Inspect the resulting workflow URL and exact commit, and require all five
native jobs plus the Linux gate to pass. Read the installed-version proof,
not just the build status. Download artifacts using `gh run download` for
the selected run. Native CI is still pending until those results exist.

## Owner-authorized publication

Publishing is a separate external write. After landing the verified
candidate on main and obtaining explicit tag/publication authorization,
set `RELEASE_TAG=v0.3.0` and inspect
`git status`, `git rev-parse HEAD`, and the manifest before running:

```sh
git tag -a "$RELEASE_TAG" -m "$RELEASE_TAG"
git push origin "$RELEASE_TAG"
```

Tag pushes must match the manifest version. The same workflow runs the full
validation/native matrix and installer smoke tests again, verifies all
packaged checksums, and creates the GitHub release with generated notes.
Publication has only job-local `contents: write`; other jobs use read access.
Manual dispatch and pull requests do not create releases.

After publication, perform clean installs from the public GitHub release
on native Windows, Linux and macOS using the commands in the
[install guide](../guide/install.md), both latest and the exact tag.
Attach installed `--version` output, release inventory, archive/checksum
verification and workflow results. A failure means the release/install
acceptance is not complete; do not silently retag or replace assets.
