#!/usr/bin/env python3
"""Linux mbx 1.29.2 target policy and whole-workflow collection lease.

The .lock protocol is version-specific (upstream crates/mbx/src/target.rs).
Never unlink a lease file: collectors and builds must lock the same inode.
"""

import json
import os
from pathlib import Path
import stat
import subprocess
import sys
from typing import NoReturn


def fail(message: str) -> NoReturn:
    print(f"corral cargo cache: {message}", file=sys.stderr)
    raise SystemExit(2)


def managed_path(path: Path, root: Path) -> Path | None:
    resolved = path.resolve()
    for candidate in (resolved, *resolved.parents):
        if candidate.parent == root.resolve() / "v1" and (
            len(candidate.name) == 64
            and all(char in "0123456789abcdef" for char in candidate.name)
        ):
            return candidate
    return None


def view_directory(root: Path, workspace: Path) -> Path:
    # Use the installed BLAKE3 CLI, not a second implementation of mbx's hash.
    # This key is the canonical workspace path, with no newline or terminator.
    try:
        result = subprocess.run(
            ["b3sum", "--no-names"], input=os.fsencode(workspace).decode("utf-8", "replace").encode(),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        fail(f"managed targets require b3sum: {error}")
    digest = result.stdout.decode("ascii", "replace").strip()
    if result.returncode or len(digest) != 64 or any(char not in "0123456789abcdef" for char in digest):
        fail("b3sum could not determine the mbx target lease identity")
    return root.resolve() / "v1" / digest


def verify_owner(view: Path, workspace: Path) -> None:
    # An absent record can be collection winning just before lease acquisition.
    # The next mbx compilation restores it while the workflow holds the lease.
    try:
        data = json.loads(view.with_suffix(".json").read_text())
    except FileNotFoundError:
        return
    except (OSError, ValueError) as error:
        fail(f"cannot verify managed target record: {error}")
    if not isinstance(data, dict) or data.get("version") != 1 or data.get("workspace_root") != str(workspace):
        fail("managed target record does not belong to this workspace")


def settings(cache: Path, workspace: Path, target: Path, explicit: bool) -> dict[str, str]:
    root = cache / "corral-managed-targets"
    if root.is_symlink() or (root / "v1").is_symlink():
        fail("the Corral managed-target root must not be a symlink")
    view = managed_path(target, root)
    expected = view_directory(root, workspace)
    if view is not None:
        if view != expected:
            fail("managed target path belongs to another workspace")
        verify_owner(view, workspace)
    default = workspace / "target"
    # Never auto-adopt an existing real target or an unrelated symlink. An explicit
    # Cargo target keeps Cargo's existing placement policy, including default paths.
    eligible = not explicit and target == default and (
        not os.path.lexists(target) or (target.is_symlink() and view is not None)
    )
    return {
        "MBX_TARGET_ROOT": str(root),
        "MBX_TARGET_VIEWS": "1" if eligible else "0",
        "MBX_TARGET_LANES": "0",
        "MBX_TARGET_MAX_SIZE": "200GiB",
        "MBX_TARGET_MAX_AGE": "7d",
        "MBX_GC_MAX_TOTAL_SIZE": "230GiB",
    }


def verify_protection() -> None:
    """Check the re-exec marker against an inherited native lease descriptor."""
    import fcntl

    workspace = os.environ.get("CORRAL_MBX_PROTECTED_WORKSPACE")
    if workspace is None or workspace != str(Path.cwd().resolve()):
        fail("protected workflow workspace does not match")
    target = Path(os.environ.get("CORRAL_MBX_PROTECTED_TARGET", ""))
    if not target.is_absolute():
        fail("protected workflow has no absolute target path")
    root = Path(os.environ.get("CORRAL_MBX_CACHE_DIR", "")) / "corral-managed-targets"
    view = view_directory(root, Path(workspace))
    if os.environ.get("CORRAL_MBX_PROTECTED_FD") == "private":
        # Existing private targets are deliberately outside mbx collection.
        if managed_path(target, root) is not None:
            fail("protected workflow has no managed-target lease")
        return
    actual = managed_path(target, root)
    if actual is not None and actual != view:
        fail("protected workflow target belongs to another workspace")
    try:
        fd = int(os.environ.get("CORRAL_MBX_PROTECTED_FD", ""))
        held = os.fstat(fd)
        expected = view.with_suffix(".lock").stat()
        if not stat.S_ISREG(held.st_mode) or (held.st_dev, held.st_ino) != (
            expected.st_dev, expected.st_ino
        ):
            fail("protected workflow inherited the wrong lease file")
        # This is the descriptor inherited from our supervisor, not a new open.
        fcntl.flock(fd, fcntl.LOCK_SH)
    except (OSError, ValueError) as error:
        fail(f"protected workflow lost its target lease: {error}")


def protect_script(
    environment: dict[str, str], workspace: Path, target: Path,
    script: list[str],
) -> int:
    """Hold mbx's native lease from before target placement through workflow exit."""
    import fcntl

    child_environment = os.environ.copy()
    child_environment.update({
        "CORRAL_MBX_PROTECTED_WORKSPACE": str(workspace),
        "CORRAL_MBX_PROTECTED_TARGET": str(target),
        "CORRAL_MBX_PROTECTED_FD": "private",
    })
    command = ["bash", script[0], "--corral-mbx-protected", *script[1:]]
    root = Path(environment["MBX_TARGET_ROOT"])
    if environment["MBX_TARGET_VIEWS"] != "1" and managed_path(target, root) is None:
        return subprocess.run(command, env=child_environment, check=False).returncode

    # Acquire before placement, exactly as mbx does. This also covers a target
    # whose directory/record was collected: mbx restores it under the same lease.
    view = view_directory(root, workspace)
    lease_path = view.with_suffix(".lock")
    lease_path.parent.mkdir(parents=True, exist_ok=True)
    # Opening and locking is the same Linux protocol as Rust File::lock_shared.
    # Closing (not explicitly unlocking) lets surviving descendants retain it.
    with lease_path.open("a+b") as lease:
        fcntl.flock(lease.fileno(), fcntl.LOCK_SH)
        actual = managed_path(target, root)
        if actual is not None and actual != view:
            fail("managed target changed ownership before workflow start")
        verify_owner(view, workspace)
        child_environment["CORRAL_MBX_PROTECTED_FD"] = str(lease.fileno())
        return subprocess.run(
            command, env=child_environment, pass_fds=(lease.fileno(),), check=False
        ).returncode


if __name__ == "__main__":
    if sys.argv[1:] != ["--verify-protection"]:
        raise SystemExit("usage: cargo_cache_targets.py --verify-protection")
    verify_protection()
