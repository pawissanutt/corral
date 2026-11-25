# Install and update (classic)

> **Classic CLI.** This guide documents the classic `corral` CLI and board (v0.4.0), which `install.sh --legacy`
> installs. The current engine is `corral-next` with the Corral app: see the [guide index](../README.md).

This guide covers the classic `corral` CLI and board. For the current app and
engine, see the [app README](../../../apps/corral-app/README.md). The main
installers default to the current product; classic installs must explicitly select
legacy mode and a published legacy tag such as v0.2.0.

The installers download a packaged release binary, check its SHA-256 sidecar,
and run the candidate's version probe before replacing an existing installation.
Neither installation nor normal use needs Rust, Cargo, a C compiler, or
administrator access.

The examples below pin published [v0.2.0](../../releases/v0.2.0.md), which provides
the five native packages below with SHA-256 sidecars. Installers download release
assets, not a tag alone. The [v0.4.0 Pi notes](../../releases/v0.4.0.md) describe
source work, not published classic v0.4.0 assets; the current release workflow
publishes different, next-engine assets.

## Supported packages

| System | Native architecture | Release asset |
|---|---|---|
| Linux | x86_64 / amd64 | `corral-x86_64-unknown-linux-musl.tar.gz` |
| Linux | aarch64 / arm64 | `corral-aarch64-unknown-linux-musl.tar.gz` |
| macOS | Intel x86_64 | `corral-x86_64-apple-darwin.tar.gz` |
| macOS | Apple silicon arm64 | `corral-aarch64-apple-darwin.tar.gz` |
| Windows | x86_64 / AMD64 | `corral-x86_64-pc-windows-msvc.zip` |

Each archive has an adjacent `<archive>.sha256` file. The published v0.2.0
archives do not include the [corral-pi agent host](#optional-the-corral-pi-agent-host).
Linux `corral` uses musl rather than requiring a particular glibc version. macOS packages target macOS
14 or later. Windows packages statically link the MSVC runtime and target 64-bit Windows
10/11; the native smoke runner is Windows Server 2022. The x86_64 Windows
package is not advertised as native ARM64 support: ARM64 and 32-bit Windows
are rejected. Other operating systems/architectures fail before downloading.

For download and extraction:

- Linux/macOS: a POSIX shell, `uname`, `awk`, `mktemp`, `tar`/gzip, normal file
  utilities, curl or wget, and either `sha256sum` or `shasum`.
- Windows: Windows PowerShell 5.1 or later with its built-in
  `Invoke-WebRequest`, `Get-FileHash`, and `Expand-Archive`. The installer uses
  TLS 1.2 for HTTPS. No external archive or checksum utility is needed.

Before opening the board, install [herdr](https://herdr.dev/docs/install/)
at least 0.9.3, Git, and an authenticated agent CLI (`omo`, `omp`, or
`claude`). Windows also needs Git for Windows with Git Bash (`bash.exe`) on
PATH for configured shell commands, gates, and agent-pane launch commands.
Windows resource admission sampling also uses Windows PowerShell 5.1.
The terminal engine must be native to the same operating system as corral;
WSL is a separate Linux installation, not a native Windows smoke test.
Read-only planner runs require the existing Linux bubblewrap sandbox.
That planner mode is not available natively on Windows/macOS; corral refuses
to launch it when an enforcing sandbox is unavailable instead of launching
an unrestricted planner. Ordinary coding runs and configured gates are
separate from planner mode.

## Linux

Install the classic v0.2.0 release:

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | CORRAL_VERSION=v0.2.0 sh -s -- --legacy --yes
export PATH="$HOME/.local/bin:$PATH"
corral --version
```

Add `export PATH="$HOME/.local/bin:$PATH"` to `~/.profile` or your shell's
startup file and open a new terminal. The default destination is
`~/.local/bin/corral`.

`--yes` accepts installation or upgrade of herdr if its installed version is
too old or unavailable. Omit `--yes` to install only corral without engine
consent. With closed/piped stdin the installer never waits for a prompt and
prints the official herdr installation command instead.

## macOS

Install using Terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | CORRAL_VERSION=v0.2.0 sh -s -- --legacy --yes
export PATH="$HOME/.local/bin:$PATH"
corral --version
```

The destination is also `~/.local/bin/corral`. For the default zsh shell,
add the PATH line to `~/.zprofile` and open a new terminal. The installer
selects the native Apple silicon or Intel package from `uname -m`; an Intel
shell running under Rosetta selects the Intel package.

## Native Windows

Run in PowerShell:

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 -OutFile install.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Legacy -Version v0.2.0
$env:PATH = "$env:LOCALAPPDATA\Programs\corral\bin;$env:PATH"
corral --version
```

The default destination is
`%LOCALAPPDATA%\Programs\corral\bin\corral.exe`. To keep it available in new
terminals, add `%LOCALAPPDATA%\Programs\corral\bin` to your **user** PATH using
Environment Variables in Windows Settings, then reopen the terminal. The
installer does not silently alter persistent PATH or execution policy.

If your policy blocks executing downloaded text, download and inspect the
script first, then run it under your organization's permitted policy:

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 -OutFile install.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Legacy -Version v0.2.0
```

The Windows installer prints setup guidance and does not install herdr
automatically. Its official installer command is:

```powershell
Invoke-RestMethod https://herdr.dev/install.ps1 | Invoke-Expression
```

Install Git for Windows, expose Git Bash on PATH, and authenticate your agent
CLI before opening the board. `bash.exe` must resolve to Git Bash, not the
Windows WSL launcher.

corral sends POSIX commands to engine panes. Merge these fields into the
`[terminal]` table in `%APPDATA%\herdr\config.toml` (adjust the executable
path if Git is installed elsewhere):

```text
[terminal]
default_shell = 'C:\Program Files\Git\bin\bash.exe'
shell_mode = "login"
```

New panes will use Git Bash; existing panes retain their shell. Open Git
Bash to start corral with the bundled Unix utilities on PATH:

```sh
export PATH="$(cygpath -u "$LOCALAPPDATA")/Programs/corral/bin:$PATH"
corral --version
corral up
```

The engine's [shell configuration](https://herdr.dev/docs/configuration/)
documents these settings. Native Windows engine plugins are currently a
[preview capability](https://herdr.dev/docs/windows-beta/).

## Pin a version, update, or change the destination

Re-running the same installer replaces the binary in place. Keep `--legacy` (or
`-Legacy` on Windows) and an explicit published legacy tag on every invocation;
`latest` can refer to a newer, incompatible release. There is no automatic updater.

Download the installer and select v0.2.0 on Linux/macOS:

```sh
curl -fsSLo install.sh https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh
CORRAL_VERSION=v0.2.0 sh install.sh --legacy
```

For a different destination, the variable applies to the installer process,
not just to curl:

```sh
CORRAL_VERSION=v0.2.0 CORRAL_INSTALL_DIR="$HOME/bin" sh install.sh --legacy
CORRAL_VERSION=v0.2.0 sh install.sh --legacy --dry-run
```

On Windows, after downloading `install.ps1`:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Legacy -Version v0.2.0
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Legacy -Version v0.2.0 -InstallDir "$HOME\bin"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Legacy -Version v0.2.0 -DryRun
```

PowerShell also accepts `CORRAL_VERSION` and `CORRAL_INSTALL_DIR` environment
variables; legacy mode still requires `-Legacy`. `CORRAL_REPO` overrides the
GitHub owner/repository; `CORRAL_BASE_URL` overrides the releases root for
mirrors or testing. PowerShell also accepts `-Repo` and `-BaseUrl`.
Only use mirrors whose binaries and checksum files you trust.

Failed archive/checksum downloads, malformed manifests, checksum mismatches,
invalid archives, version mismatches, and candidates that cannot execute fail
with a nonzero exit. The previous executable is retained and staging files
are removed. Updates stage on the installation filesystem and replace only
after a successful probe. On Windows, close a running corral before updating
if file sharing prevents replacement; a sharing violation fails rather than
removing the working binary.

SHA-256 detects a corrupted or mismatched download; it is not a signature or
independent proof of publisher identity. Archives and checksum files come
from the same release. macOS notarization and Windows Authenticode signing
are not part of this release pipeline.

## Optional: the corral-pi agent host

The published v0.2.0 archives do not include a Pi host. Classic Pi support exists in
the historical v0.4.0 source, but that tag has no published release assets and is not
covered by the no-toolchain installation above. The current engine's Pi host is a
separate component; see the [current app install guide](../../../apps/corral-app/README.md).

## Contributors

Building from source needs Rust at the manifest's minimum version and the
native linker toolchain. The checkout builds the new engine's CLI and daemon:

```sh
cargo install --path crates/corral-next --locked
cargo install --path crates/corral-daemon --locked    # installs corrald-next
```

With [`just`](https://just.systems) installed, the checkout's `justfile` wraps the routine
steps for the new engine; `just` alone lists them:

| Recipe | Does |
|---|---|
| `just install` | Builds and installs `corral-next` and `corrald-next`, then restarts a running daemon on them |
| `just upgrade` | `just install`, then `pi-install` and `app-web-install` from the same commit |
| `just daemon-restart` | Lets the running pass finish, stops the daemon, then starts its service again (`daemon-stop`, `daemon-start`) |
| `just daemon-status`, `just daemon-logs` | Health, and the service journal |
| `just daemon-service` | Writes `corrald-next.service` for the workspace, then enables and starts it |
| `just pair` | Pairs a browser or phone with the daemon |
| `just pi-install` | Copies `packages/corral-pi` to the data directory, installs its dependencies and writes the `corral-pi` launcher |
| `just pi-check`, `just pi-test` | Type-checks and tests the corral-pi host |
| `just app` | Runs the [app](../../../apps/corral-app/README.md) in this terminal against the workspace daemon |
| `just app-web`, `just app-web-install` | Rebuilds the packaged web app; the second also copies it to the directory the web service serves. `just app-serve` serves the build in the foreground |
| `just app-check`, `just app-test` | Checks and tests the app |

The stop, start, logs and service recipes drive the systemd user service
`corrald-next.service`. Recipes address the workspace found above the checkout, which a
worktree under the workspace finds too; from a checkout elsewhere, pass `root=<workspace>`
first: `just root=/path/to/workspace daemon-status`. The service sets `KillMode=process`:
stopping or restarting the daemon leaves the session hosts it started running.
`systemctl --user kill corrald-next.service` stops the daemon and those hosts together.

See the [release runbook](../../releases/README.md) for native CI evidence,
artifact names, version selection, and owner-controlled publication.
