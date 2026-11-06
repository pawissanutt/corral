# /// script
# requires-python = ">=3.11"
# ///
"""Cargo configuration checks used by the opt-in mbx adapter."""

import datetime
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
from typing import BinaryIO, NoReturn, Protocol, TypeAlias

Toml: TypeAlias = (
    str | int | float | bool | datetime.datetime | datetime.date | datetime.time
    | list["Toml"] | dict[str, "Toml"]
)


class TomlDecoder(Protocol):
    def load(self, fp: BinaryIO, /) -> dict[str, Toml]: ...


toml_decoder: TomlDecoder = tomllib


def fail(message: str) -> NoReturn:
    print(f"corral cargo cache: {message}", file=sys.stderr)
    raise SystemExit(2)


def cargo_builtin(cargo: str, toolchain: list[str], command: str) -> bool:
    """Check built-ins without aliases or external commands affecting the inventory."""
    version = subprocess.run(
        [cargo, *toolchain, "--version"], stdout=subprocess.PIPE, check=False,
    )
    if version.returncode:
        fail("cannot determine the workspace Cargo version")
    with tempfile.TemporaryDirectory(prefix="corral-cargo-builtins-") as temporary:
        directory = Path(temporary)
        for parent in (directory, *directory.parents):
            if (parent / ".cargo" / "config").exists() or (
                parent / ".cargo" / "config.toml"
            ).exists():
                fail("cannot isolate Cargo's built-in command inventory")
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith("CARGO_ALIAS_")
        }
        environment.update({"CARGO_HOME": temporary, "PATH": temporary})
        rustup = Path(cargo).resolve()
        if not toolchain and not environment.get("RUSTUP_TOOLCHAIN") and rustup.name == "rustup":
            active = subprocess.run(
                [str(rustup), "show", "active-toolchain"],
                stdout=subprocess.PIPE, text=True, check=False,
            )
            fields = active.stdout.split()
            if active.returncode or not fields:
                fail("cannot determine the workspace Rust toolchain")
            environment["RUSTUP_TOOLCHAIN"] = fields[0]
        isolated_version = subprocess.run(
            [cargo, *toolchain, "--version"], cwd=temporary, env=environment,
            stdout=subprocess.PIPE, check=False,
        )
        if isolated_version.returncode or isolated_version.stdout != version.stdout:
            fail("cannot isolate the workspace Cargo toolchain")
        result = subprocess.run(
            [cargo, *toolchain, "--color=never", "--list"],
            cwd=temporary, env=environment, stdout=subprocess.PIPE, text=True, check=False,
        )
        lines = result.stdout.splitlines()
        if result.returncode or not lines or lines[0] != "Installed Commands:":
            fail("cannot identify Cargo's built-in commands")
        for line in lines[1:]:
            parts = line.strip().split(None, 1)
            if parts and parts[0] == command:
                return len(parts) < 2 or not parts[1].startswith("alias:")
        return False


def reject_alias(cargo: str, args: list[str], toolchain: list[str]) -> None:
    for arg in args:
        if arg == "--":
            break
        if arg in ("-C", "--directory", "-Z") or arg.startswith(
            ("-C", "--directory=", "-Z")
        ):
            fail("Cargo directory-changing or unstable options are not supported in mbx mode")
    index = 0
    command = ""
    while index < len(args):
        arg = args[index]
        if arg == "--":
            break
        if arg in ("--color", "--config", "-Z"):
            if index + 1 >= len(args):
                fail(f"{arg} requires a value")
            index += 2
            continue
        if not arg.startswith(("-", "+")):
            command = arg
            break
        index += 1
    if not command:
        fail("cannot identify Cargo subcommand")
    if not command.isascii() or not all(char.isalnum() or char in "-_" for char in command):
        fail("Cargo subcommands must use ASCII letters, digits, hyphens or underscores")
    key = "CARGO_ALIAS_" + command.replace("-", "_").upper()
    found = command in ("b", "c", "d", "r", "t", "rm") or key in os.environ

    cwd = Path.cwd()
    home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    config_dirs = [*(parent / ".cargo" for parent in (cwd, *cwd.parents)), home]
    for directory in config_dirs:
        for name in ("config", "config.toml"):
            try:
                with (directory / name).open("rb") as config_file:
                    config = toml_decoder.load(config_file)
            except FileNotFoundError:
                continue
            except (OSError, tomllib.TOMLDecodeError) as error:
                fail(f"cannot inspect Cargo configuration {directory / name}: {error}")
            if "include" in config:
                fail("Cargo configuration includes are not supported in mbx mode")
            match config.get("alias", {}):
                case dict() as aliases:
                    if command in aliases:
                        found = True
                case _:
                    fail("cannot inspect Cargo aliases")
            # Cargo's legacy config takes precedence over config.toml in one directory.
            break
    if found and not cargo_builtin(cargo, toolchain, command):
        fail("Cargo aliases are not supported in mbx mode; use the direct command")


if __name__ == "__main__":
    cargo, *args = sys.argv[1:]
    toolchain = [args[0]] if args and args[0].startswith("+") else []
    reject_alias(cargo, args, toolchain)
