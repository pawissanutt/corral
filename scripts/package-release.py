#!/usr/bin/env python3
"""Package the built native binary and its SHA-256 sidecar."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path


def package(target: str) -> None:
    """Build the existing release asset layout from one Cargo target."""
    windows = target.endswith("-windows-msvc")
    binary_name = "corral.exe" if windows else "corral"
    binary = Path(os.environ.get("CARGO_TARGET_DIR", "target")) / target / "release" / binary_name
    metadata: dict[str, dict[str, dict[str, str]]] = tomllib.loads(Path("Cargo.toml").read_text())
    version = metadata["workspace"]["package"]["version"]
    dist = Path("dist")
    dist.mkdir(exist_ok=True)
    archive = dist / f"corral-{target}{'.zip' if windows else '.tar.gz'}"
    files = [(binary, binary_name), (Path("LICENSE"), "LICENSE"), (Path("README.md"), "README.md")]
    if windows:
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for source, name in files:
                bundle.write(source, name)
    else:
        with tarfile.open(archive, "w:gz") as bundle:
            for source, name in files:
                bundle.add(source, arcname=name)
    with archive.open("rb") as contents:
        digest = hashlib.file_digest(contents, "sha256").hexdigest()
    _ = archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    manifest = {
        "version": version,
        "target": target,
        "runner_os": platform.system(),
        "runner_arch": platform.machine(),
        "source_revision": os.environ.get("GITHUB_SHA", "local"),
        "archive": archive.name,
        "sha256": digest,
    }
    _ = (dist / f"{target}.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    package(sys.argv[1])
