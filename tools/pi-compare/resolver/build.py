#!/usr/bin/env python3
"""Build the offline resolver from a disposable manifest, not a workspace package.

Run from the repository root so Cargo uses the repository's .cargo/config.toml.
The build directory must be new; use tools/pi-compare/records/<name> to keep it ignored.
"""
import json
from pathlib import Path
import shutil
import subprocess
import sys


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: python3 tools/pi-compare/resolver/build.py NEW_BUILD_DIR TARGET_DIR")
    source = Path(__file__).resolve().parent
    root = source.parents[2]
    if Path.cwd().resolve() != root:
        raise SystemExit("run from the repository root to retain Cargo configuration")
    build = Path(sys.argv[1]).resolve()
    target = Path(sys.argv[2]).resolve()
    if root not in build.parents:
        raise SystemExit("build directory must be inside this worktree")
    build.mkdir(parents=True, exist_ok=False)
    (build / "src").mkdir()
    core = json.dumps(str(root / "crates/corral-core"))
    manifest = (source / "Cargo.toml.template").read_text().replace("@CORE_PATH@", core)
    (build / "Cargo.toml").write_text(manifest)
    shutil.copyfile(source / "Cargo.lock", build / "Cargo.lock")
    shutil.copyfile(source / "src/main.rs", build / "src/main.rs")
    subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(build / "Cargo.toml"),
                    "--target-dir", str(target)], stdin=subprocess.DEVNULL, check=True)
    print(target / "debug/pi-profile-resolver")


if __name__ == "__main__":
    main()
