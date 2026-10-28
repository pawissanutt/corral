# Install and update

The installers download a packaged release binary, check its SHA-256 sidecar,
and run the candidate's version probe before replacing an existing installation.
Neither installation nor normal use needs Rust, Cargo, a C compiler, or
administrator access.

The examples below pin [v0.4.0](../releases/v0.4.0.md). Installers download
published release assets, not a tag alone; older releases may lack Windows
packages. The release workflow publishes the five native packages below
with SHA-256 sidecars and installer-proof records after validation.

## Supported packages

| System | Native architecture | Release asset |
|---|---|---|
| Linux | x86_64 / amd64 | `corral-x86_64-unknown-linux-musl.tar.gz` |
| Linux | aarch64 / arm64 | `corral-aarch64-unknown-linux-musl.tar.gz` |
| macOS | Intel x86_64 | `corral-x86_64-apple-darwin.tar.gz` |
| macOS | Apple silicon arm64 | `corral-aarch64-apple-darwin.tar.gz` |
| Windows | x86_64 / AMD64 | `corral-x86_64-pc-windows-msvc.zip` |

Each archive has an adjacent `<archive>.sha256` file. From v0.4.0 the Linux and
macOS archives also contain `corral-pi-host`, the compiled [Pi-native agent
host](#optional-the-pi-native-agent-host); the Windows archive does not. Linux
`corral` uses musl rather than requiring a particular glibc version;
`corral-pi-host` is a glibc build (any mainstream distribution, not Alpine). macOS packages target macOS
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

Install the latest published release:

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
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
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
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
Invoke-RestMethod https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 | Invoke-Expression
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
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
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

Re-running the same installer updates the binary in place. The default
`CORRAL_VERSION=latest` selects GitHub's latest published release, not an
unreleased main-branch build or a prerelease. An explicit `vX.Y.Z` tag stays
pinned on subsequent invocations where that setting remains present. There
is no automatic updater: re-run to update, or choose an older available tag
to downgrade.

Pin `v0.4.0` on Linux/macOS once its release assets are published:

```sh
curl -fsSLo install.sh https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh
CORRAL_VERSION=v0.4.0 sh install.sh
```

For a different destination, the variable applies to the installer process,
not just to curl:

```sh
CORRAL_INSTALL_DIR="$HOME/bin" sh install.sh
sh install.sh --dry-run
```

On Windows, after downloading `install.ps1`:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Version v0.4.0
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -InstallDir "$HOME\bin"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -DryRun
```

PowerShell also accepts `CORRAL_VERSION` and `CORRAL_INSTALL_DIR` environment
variables, useful with the one-line command. `CORRAL_REPO` overrides the
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

## Optional: the Pi-native agent host

The [Pi-native host](pi-native.md) is opt-in and needs nothing above unless you configure a
`pi-native` profile. It runs on Linux and macOS.

From a release (v0.4.0 and later) `install.sh` installs `corral-pi-host` beside `corral`
and runs `corral agent install` for you. The host is one executable that embeds Bun and
every dependency, so no Bun, checkout or network install is involved. Then log in once:

```sh
corral agent login      # pick OpenAI (ChatGPT) or Anthropic (Claude Pro/Max)
corral agent doctor
```

`corral agent install` copies the host into
`$XDG_DATA_HOME/corral/pi-native/assets/<digest>/` (`~/.local/share/...` without
`XDG_DATA_HOME`) with the `compat.json` and role files it writes there, and records the
digest; re-run it after replacing `corral-pi-host` by hand. Anthropic profiles also need
the `claude` CLI (Claude Code) on PATH.

From a checkout (contributors, or a binary built with `cargo install`), the host runs from
source on a Bun inside the tested range its `compat.json` records (currently exactly 1.4.2):

```sh
corral agent install --from <corral checkout>/packages/corral-pi
```

That copies the package and runs `bun install --frozen-lockfile` in the copy. A binary
built from a checkout defaults `--from` to that checkout's `packages/corral-pi` when no
`corral-pi-host` sits beside it. `scripts/build-pi-host.sh <out>` builds the release host
locally. Neither install nor login touches `~/.codex`, `~/.claude` or another agent's
store.

## Contributors

Building from source needs Rust at the manifest's minimum version and the
native linker toolchain:

```sh
cargo install --path crates/corral --locked
```

See the [release runbook](../releases/README.md) for native CI evidence,
artifact names, version selection, and owner-controlled publication.
