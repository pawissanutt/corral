#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# ///
"""Run Cargo through the explicitly configured Corral mbx profile.

Usage: bash scripts/cargo.sh <cargo arguments>
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import NoReturn, Protocol, TypeAlias

Json: TypeAlias = str | int | float | bool | None | list["Json"] | dict[str, "Json"]


class JsonDecoder(Protocol):
    def loads(self, data: bytes, /) -> Json: ...


decoder: JsonDecoder = json


def fail(message: str) -> NoReturn:
    print(f"corral cargo cache: {message}", file=sys.stderr)
    raise SystemExit(2)


def absolute_setting(name: str) -> Path:
    value = os.environ.get(name, "")
    if not value or not Path(value).is_absolute():
        fail(f"{name} must be an absolute path")
    return Path(value)


def xdg_directory(name: str, fallback: str) -> Path:
    path = Path(os.environ.get(name, ""))
    return path if path.is_absolute() else Path.home() / fallback


def cargo_launcher() -> str:
    requested = os.environ.get("CARGO") or "cargo"
    if os.environ.get("CARGO") and not Path(requested).is_absolute():
        fail("CARGO must be an absolute executable path in mbx mode")
    selected = shutil.which(requested)
    if selected is None:
        fail("cannot find Cargo")
    cargo = Path(os.path.abspath(selected))
    shim = xdg_directory("XDG_DATA_HOME", ".local/share") / "mbx" / "bin" / "cargo"
    if cargo.parts[-3:] == ("command-wrappers", "bin", "cargo") or (
        shim.exists() and cargo.samefile(shim)
    ):
        fail("CARGO must name a Cargo launcher, not an mbx or mise command wrapper")
    return str(cargo)


def metadata_arguments(args: list[str]) -> tuple[list[str], str | None]:
    """Preserve the Cargo options that select the workspace and target directory."""
    metadata: list[str] = []
    target = None
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--":
            break
        if arg in ("--manifest-path", "--config", "--target-dir", "-Z"):
            if index + 1 >= len(args):
                fail(f"{arg} requires a value")
            value = args[index + 1]
            if arg == "--target-dir":
                target = value
            else:
                metadata.extend((arg, value))
            index += 2
            continue
        if arg.startswith("--target-dir="):
            target = arg.removeprefix("--target-dir=")
        elif arg.startswith(("--manifest-path=", "--config=")) or (
            arg.startswith("-Z") and len(arg) > 2
        ):
            metadata.append(arg)
        index += 1
    return metadata, target


def reject_managed_target(path: Path) -> None:
    # mbx 1.29.2 records each managed view at <root>/v1/<digest>.json.
    # Ordinary symlinks remain supported. Check ancestors as a caller can
    # explicitly choose a subdirectory inside an existing managed view.
    resolved = path.resolve()
    for candidate in (resolved, *resolved.parents):
        if (
            candidate.parent.name == "v1"
            and len(candidate.name) == 64
            and all(char in "0123456789abcdef" for char in candidate.name)
            and candidate.with_suffix(".json").exists()
        ):
            fail(f"target is an mbx-managed view: {path}; use a private target")


def metadata_paths(data: bytes) -> tuple[Path, Path, Path]:
    match decoder.loads(data):
        case {
            "workspace_root": str() as root,
            "target_directory": str() as target,
            "build_directory": str() as build,
        }:
            return Path(root).resolve(), Path(target), Path(build)
        case _:
            fail("Cargo metadata omitted the workspace or target directory")


def main() -> None:
    if sys.platform != "linux":
        fail("the mbx profile is currently qualified only on Linux")
    inherited = sorted(key for key in os.environ if key.startswith("MBX_"))
    if inherited:
        fail("unset ambient MBX_* settings before using the Corral profile")

    binary = absolute_setting("CORRAL_MBX_BIN")
    cache = absolute_setting("CORRAL_MBX_CACHE_DIR")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        fail("CORRAL_MBX_BIN must name an executable")
    version = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, check=False
    )
    if version.returncode != 0 or version.stdout.strip() != "mbx 1.29.2":
        fail("CORRAL_MBX_BIN must be mbx 1.29.2")

    config_home = xdg_directory("XDG_CONFIG_HOME", ".config")
    if (config_home / "mbx" / "config.toml").exists():
        fail("ambient mbx configuration is not supported by the Corral profile")

    args = sys.argv[1:]
    cargo = cargo_launcher()
    toolchain = [args[0]] if args and args[0].startswith("+") else []
    command = args[len(toolchain):]
    if command in (["--version"], ["-V"], ["--help"], ["-h"]) or command[:1] in (
        ["help"], ["search"], ["login"], ["logout"], ["owner"], ["yank"],
    ):
        os.execvp(cargo, [cargo, *args])
    alias_check = subprocess.run(
        [sys.executable, str(Path(__file__).with_name("cargo_config.py")), cargo, *args],
        check=False,
    )
    if alias_check.returncode:
        raise SystemExit(alias_check.returncode)
    metadata_args, explicit_target = metadata_arguments(args)
    metadata_environment = os.environ.copy()
    if explicit_target is not None:
        absolute_target = os.path.abspath(explicit_target)
        metadata_environment["CARGO_TARGET_DIR"] = absolute_target
        metadata_args.extend(("--config", f"build.target-dir={json.dumps(absolute_target)}"))
    result = subprocess.run(
        [cargo, *toolchain, "metadata", "--offline", "--no-deps",
         "--format-version", "1", *metadata_args],
        stdout=subprocess.PIPE, env=metadata_environment, check=False,
    )
    if result.returncode:
        raise SystemExit(result.returncode)
    try:
        workspace, target, build = metadata_paths(result.stdout)
    except (ValueError, KeyError, TypeError) as error:
        fail(f"cannot determine Cargo workspace and target: {error}")
    if (workspace / ".mbx.toml").exists():
        fail("workspace .mbx.toml is not supported by the Corral profile")
    if cache.resolve().is_relative_to(workspace):
        fail("CORRAL_MBX_CACHE_DIR must be outside the workspace")
    reject_managed_target(target)
    reject_managed_target(build)
    managed_root = cache / "corral-disabled-targets"
    records = managed_root / "v1"
    if managed_root.is_symlink() or records.is_symlink():
        fail("the Corral managed-target root must not be a symlink")
    try:
        has_records = any(path.suffix == ".json" for path in records.iterdir())
    except FileNotFoundError:
        has_records = False
    except OSError as error:
        fail(f"cannot inspect managed target records: {error}")
    if has_records:
        fail("the Corral cache contains managed target records")

    environment = os.environ.copy()
    environment.update({
        "CARGO": cargo,
        "MBX_CACHE_DIR": str(cache),
        "MBX_TARGET_VIEWS": "0", "MBX_TARGET_ROOT": str(managed_root),
        "MBX_GC_AUTO": "1",
        "MBX_GC_MAX_SIZE": "20GiB", "MBX_GC_MAX_TOTAL_SIZE": "30GiB",
        "MBX_GC_INCREMENTAL_MAX_SIZE": "10GiB", "MBX_GC_INCREMENTAL_MAX_AGE": "14d",
        "MBX_LINKER": "system",
        "MBX_SCHEDULER": "0", "MBX_SCHEDULER_TESTS": "0",
        "MBX_SHARE_WORKSPACE_ROOT": "0", "MBX_DISPLAY": "plain",
    })
    os.execve(binary, [str(binary), *args], environment)


if __name__ == "__main__":
    main()
