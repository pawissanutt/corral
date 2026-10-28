#!/usr/bin/env python3
"""Native clean-install proof using real release packages over loopback HTTP."""
from __future__ import annotations

import functools
import hashlib
import http.server
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import tomllib
import zipfile
from pathlib import Path


def fingerprint(path: Path) -> str:
    """Hash the exact downloaded or installed bytes."""
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(dist: Path, target: str) -> None:
    """Exercise install, update and failure paths without access to Rust or herdr."""
    repo = Path(__file__).resolve().parent.parent
    metadata: dict[str, dict[str, dict[str, str]]] = tomllib.loads((repo / "Cargo.toml").read_text())
    version = metadata["workspace"]["package"]["version"]
    windows = target.endswith("-windows-msvc")
    asset = f"corral-{target}{'.zip' if windows else '.tar.gz'}"
    shell = shutil.which("powershell.exe" if windows else "sh")
    assert shell, "native shell missing"
    evidence: list[dict[str, str | int]] = []
    with tempfile.TemporaryDirectory(prefix="corral-release-") as temporary:
        root = Path(temporary)
        web = root / "web"
        latest = web / "releases" / "latest" / "download"
        pinned = web / "releases" / "download" / f"v{version}"
        for location in (latest, pinned):
            location.mkdir(parents=True)
            for name in (asset, asset + ".sha256"):
                _ = shutil.copyfile(dist / name, location / name)
        handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(web))
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                environment = {
                    key: value for key, value in os.environ.items()
                    if not key.startswith(("CORRAL_", "HERDR_", "CARGO_", "RUSTUP_"))
                    and key.upper() != "PSMODULEPATH"
                }
                environment["HOME"] = str(root / "home")
                environment["XDG_DATA_HOME"] = str(root / "home" / ".local" / "share")
                environment["LOCALAPPDATA"] = str(root / "localappdata")
                environment["CORRAL_BASE_URL"] = f"http://127.0.0.1:{server.server_port}/releases"
                tools = root / "tools"
                if windows:
                    environment["PATH"] = os.pathsep.join((
                        str(Path(os.environ["WINDIR"]) / "System32"),
                        str(Path(shell).parent),
                    ))
                    installed = root / "localappdata" / "Programs" / "corral" / "bin" / "corral.exe"
                    command = [shell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                               "-File", str(repo / "install.ps1")]
                else:
                    tools.mkdir()
                    for name in ("sh", "uname", "curl", "tar", "gzip", "awk", "mktemp",
                                 "rm", "mkdir", "cp", "chmod", "mv", "sha256sum", "shasum"):
                        executable = shutil.which(name)
                        if executable:
                            (tools / name).symlink_to(executable)
                    environment["PATH"] = str(tools)
                    installed = root / "home" / ".local" / "bin" / "corral"
                    command = [shell, str(repo / "install.sh")]
                assert shutil.which("cargo", path=environment["PATH"]) is None
                assert shutil.which("rustc", path=environment["PATH"]) is None
                assert shutil.which("bun", path=environment["PATH"]) is None

                def install(case: str, success: bool) -> None:
                    """Run the public installer with closed stdin and record the outcome."""
                    result = subprocess.run(
                        command, env=environment, stdin=subprocess.DEVNULL,
                        capture_output=True, text=True, timeout=120, check=False,
                    )
                    print(f"\n==> {case}: exit {result.returncode}\n{result.stdout}{result.stderr}", flush=True)
                    evidence.append({
                        "case": case, "exit_code": result.returncode,
                        "stdout": result.stdout, "stderr": result.stderr,
                    })
                    assert (result.returncode == 0) == success, case
                    assert not list(installed.parent.glob(".corral-install*")), case

                def smoke() -> None:
                    """Execute the installed native program rather than the build output."""
                    result = subprocess.run(
                        [str(installed), "--version"], env=environment, stdin=subprocess.DEVNULL,
                        capture_output=True, text=True, timeout=30, check=True,
                    )
                    assert result.stdout.strip() == f"corral {version}", result.stdout
                    if windows:
                        return
                    # The compiled Pi-native host installed beside corral and registered by
                    # `corral agent install`, with no Bun on PATH.
                    host = installed.with_name("corral-pi-host")
                    probe = subprocess.run(
                        [str(host), "--version"], env=environment, stdin=subprocess.DEVNULL,
                        capture_output=True, text=True, timeout=60, check=True,
                    )
                    assert probe.stdout.startswith("corral-pi-host pi "), probe.stdout
                    data = Path(environment["XDG_DATA_HOME"]) / "corral" / "pi-native"
                    record = json.loads((data / "installed.json").read_text())
                    assert (Path(record["path"]) / "corral-pi-host").is_file(), record
                    assert (Path(record["path"]) / "compat.json").is_file(), record

                # Given a clean user home, latest installs into the documented default.
                install("clean-latest-no-rust", True)
                smoke()
                prior = fingerprint(installed)

                # Given an existing install, an explicit tag updates the same path.
                environment["CORRAL_VERSION"] = f"v{version}"
                install("pinned-update", True)
                smoke()
                assert fingerprint(installed) == prior
                environment["CORRAL_VERSION"] = "latest"

                # Given a failed artifact download, neither updates nor clean installs leave a binary.
                (latest / asset).unlink()
                install("failed-download-preserves-existing", False)
                assert fingerprint(installed) == prior
                environment["CORRAL_INSTALL_DIR"] = str(root / "empty install")
                install("failed-download-clean-install", False)
                assert not (root / "empty install" / installed.name).exists()
                del environment["CORRAL_INSTALL_DIR"]
                _ = shutil.copyfile(dist / asset, latest / asset)

                # Given a missing checksum or a mismatch, refuse replacement.
                (latest / (asset + ".sha256")).unlink()
                install("missing-checksum-preserves-existing", False)
                assert fingerprint(installed) == prior
                _ = (latest / (asset + ".sha256")).write_text(f"{'0' * 64}  {asset}\n")
                install("checksum-mismatch-preserves-existing", False)
                assert fingerprint(installed) == prior

                # Given a checksummed but invalid archive, extraction cannot replace the binary.
                _ = (latest / asset).write_bytes(b"not a release archive")
                _ = (latest / (asset + ".sha256")).write_text(f"{fingerprint(latest / asset)}  {asset}\n")
                install("invalid-archive-preserves-existing", False)
                assert fingerprint(installed) == prior

                # Given a valid archive with an unusable executable, probe before replacing.
                if windows:
                    with zipfile.ZipFile(latest / asset, "w") as bundle:
                        bundle.writestr("corral.exe", b"not an executable")
                else:
                    with tarfile.open(latest / asset, "w:gz") as bundle:
                        contents = b"#!/bin/sh\nexit 1\n"
                        entry = tarfile.TarInfo("corral")
                        entry.mode = 0o755
                        entry.size = len(contents)
                        bundle.addfile(entry, io.BytesIO(contents))
                _ = (latest / (asset + ".sha256")).write_text(f"{fingerprint(latest / asset)}  {asset}\n")
                install("unusable-binary-preserves-existing", False)
                assert fingerprint(installed) == prior
                smoke()

                # Given unsupported platform data, reject before attempting installation.
                if windows:
                    environment["PROCESSOR_ARCHITEW6432"] = "ARM64"
                else:
                    (tools / "uname").unlink()
                    _ = (tools / "uname").write_text("#!/bin/sh\ncase \"$1\" in\n-s) echo Linux;;\n-m) echo riscv64;;\nesac\n")
                    (tools / "uname").chmod(0o755)
                install("unsupported-architecture-preserves-existing", False)
                assert fingerprint(installed) == prior
            finally:
                server.shutdown()
                thread.join(timeout=10)
                assert not thread.is_alive(), "HTTP fixture did not stop"
    proof = {
        "target": target,
        "archive": asset,
        "archive_sha256": fingerprint(dist / asset),
        "version": version,
        "rust_on_installer_path": False,
        "cases": evidence,
    }
    assert (dist / (asset + ".sha256")).read_text().split() == [proof["archive_sha256"], asset]
    _ = (dist / f"installer-proof-{target}.json").write_text(json.dumps(proof, indent=2) + "\n")
    print(f"native installer proof passed: {target}, {len(evidence)} cases, corral {version}", flush=True)


if __name__ == "__main__":
    verify(Path(sys.argv[1]).resolve(), sys.argv[2])
