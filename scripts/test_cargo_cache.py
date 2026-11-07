#!/usr/bin/env python3
# /// script
# requires-python = ">=3.12"
# ///
"""Exercise the Cargo adapter's process and target-safety boundaries."""

import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from typing import override
import unittest


ADAPTER = Path(__file__).with_name("cargo.sh")


class CargoCacheTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.root: Path = Path()
        self.work: Path = Path()
        self.bin: Path = Path()
        self.target: Path = Path()
        self.mbx: Path = Path()
        self.environment: dict[str, str] = {}

    @override
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="corral-mbx-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.work = self.root / "checkout"
        self.work.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.target = self.work / "target"
        self.write_executable(
            self.bin / "cargo",
            """#!/usr/bin/env python3
import json, os, sys
if "--list" in sys.argv:
    print("Installed Commands:\\n    build    Build a package\\n    check    Check a package")
elif sys.argv[1:] == ["--version"]:
    print("cargo 1.94.1")
elif "metadata" in sys.argv:
    target = os.environ.get("CARGO_TARGET_DIR", os.environ["FIXTURE_TARGET"])
    print(json.dumps({"workspace_root": os.environ["FIXTURE_WORKSPACE"],
                      "target_directory": target,
                      "build_directory": os.environ.get("FIXTURE_BUILD", target)}))
else:
    print(sys.argv[1])
    sys.exit(17)
""",
        )
        self.mbx = self.bin / "mbx"
        self.write_executable(
            self.mbx,
            """#!/usr/bin/env python3
import json, os, sys
if sys.argv[1:] == ["--version"]:
    print("mbx 1.29.2")
else:
    print(json.dumps({key: os.environ.get(key) for key in
                      ("MBX_TARGET_VIEWS", "MBX_GC_AUTO", "MBX_LINKER",
                       "MBX_SCHEDULER", "MBX_CACHE_DIR", "MBX_TARGET_ROOT")}))
    sys.exit(19)
""",
        )
        self.environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("MBX_", "CORRAL_BUILD_CACHE", "CORRAL_MBX_"))
        }
        _ = self.environment.pop("CARGO", None)
        self.environment.update({
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.root),
            "CARGO_HOME": str(self.root / "cargo-home"),
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "FIXTURE_WORKSPACE": str(self.work),
            "FIXTURE_TARGET": str(self.target),
            "CORRAL_MBX_BIN": str(self.mbx),
            "CORRAL_MBX_CACHE_DIR": str(self.root / "cache"),
        })

    def write_executable(self, path: Path, text: str) -> None:
        _ = path.write_text(text.replace("#!/usr/bin/env python3", f"#!{sys.executable}", 1))
        path.chmod(0o755)

    def run_adapter(self, *args: str, **environment: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(ADAPTER), *args], cwd=self.work,
            env={**self.environment, **environment}, text=True,
            capture_output=True, check=False, timeout=10,
        )

    def test_off_mode_preserves_arguments_and_exit_status(self) -> None:
        result = self.run_adapter("argument with spaces")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(result.stdout.strip(), "argument with spaces")

    def test_mbx_mode_applies_profile_and_preserves_failure(self) -> None:
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 19, result.stderr)
        self.assertEqual(json.loads(result.stdout), {
            "MBX_TARGET_VIEWS": "0",
            "MBX_GC_AUTO": "1",
            "MBX_LINKER": "system",
            "MBX_SCHEDULER": "0",
            "MBX_CACHE_DIR": str(self.root / "cache"),
            "MBX_TARGET_ROOT": str(self.root / "cache" / "corral-disabled-targets"),
        })

    def test_mbx_masks_config_rustflags_without_overwriting_explicit_flags(self) -> None:
        self.environment.pop("RUSTFLAGS", None)
        self.environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
        cargo_home = self.root / "cargo-home"
        cargo_home.mkdir()
        config = cargo_home / "config.toml"
        original = (
            '[target.x86_64-unknown-linux-gnu]\n'
            'rustflags = ["-C", "linker-features=-lld", "-C", "link-arg=-B/owner/mold"]\n'
        )
        _ = config.write_text(original)
        self.write_executable(
            self.mbx,
            """#!/usr/bin/env python3
import json, os, sys
if sys.argv[1:] == ["--version"]:
    print("mbx 1.29.2")
else:
    print(json.dumps({key: os.environ.get(key) for key in
                      ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "MBX_SUMMARY")}))
    sys.exit(19)
""",
        )
        cases = (
            ({}, {"RUSTFLAGS": None, "CARGO_ENCODED_RUSTFLAGS": ""}),
            ({"RUSTFLAGS": "-C debuginfo=1"},
             {"RUSTFLAGS": "-C debuginfo=1", "CARGO_ENCODED_RUSTFLAGS": None}),
            ({"RUSTFLAGS": ""}, {"RUSTFLAGS": "", "CARGO_ENCODED_RUSTFLAGS": None}),
            ({"CARGO_ENCODED_RUSTFLAGS": "--cfg\x1fexplicit"},
             {"RUSTFLAGS": None, "CARGO_ENCODED_RUSTFLAGS": "--cfg\x1fexplicit"}),
        )
        for overrides, expected in cases:
            with self.subTest(overrides=overrides):
                result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx", **overrides)
                self.assertEqual(result.returncode, 19, result.stderr)
                self.assertEqual(json.loads(result.stdout), {**expected, "MBX_SUMMARY": "full"})
        self.assertEqual(config.read_text(), original)

    def test_ambient_mbx_configuration_is_rejected(self) -> None:
        result = self.run_adapter(
            "check", CORRAL_BUILD_CACHE="mbx", MBX_REMOTE_URL="https://example.invalid"
        )
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")

    def test_relative_xdg_config_checks_home_fallback(self) -> None:
        config = self.root / ".config" / "mbx"
        config.mkdir(parents=True)
        _ = (config / "config.toml").write_text('[remote]\nurl = "https://example.invalid"\n')
        result = self.run_adapter(
            "check", CORRAL_BUILD_CACHE="mbx", XDG_CONFIG_HOME="relative-config"
        )
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")

    def test_cargo_override_is_used_for_target_preflight(self) -> None:
        view = self.root / "managed" / "v1" / ("b" * 64)
        view.mkdir(parents=True)
        _ = view.with_suffix(".json").write_text("{}")
        alternative = self.bin / "other-cargo"
        self.write_executable(
            alternative,
            f"""#!/usr/bin/env python3
import json, pathlib, sys
if "--list" in sys.argv:
    print("Installed Commands:\\n    check    Check a package")
else:
    pathlib.Path({str(self.root / "probed")!r}).touch()
    print(json.dumps({{"workspace_root": {str(self.work)!r}, "target_directory": {str(view)!r},
                      "build_directory": {str(view)!r}}}))
""",
        )
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx", CARGO=str(alternative))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertTrue((self.root / "probed").exists())

    def test_alias_is_rejected_without_disabling_external_subcommands(self) -> None:
        cargo_home = self.root / "cargo-home"
        cargo_home.mkdir()
        _ = (cargo_home / "config.toml").write_text(
            '[alias]\nunsafe = "build --target-dir /managed"\nunrelated = 42\n'
        )
        alias = self.run_adapter("unsafe", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(alias.returncode, 2)
        plugin = self.run_adapter("custom", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(plugin.returncode, 19, plugin.stderr)

    def test_environment_alias_is_rejected(self) -> None:
        result = self.run_adapter(
            "custom", CORRAL_BUILD_CACHE="mbx", CARGO_ALIAS_CUSTOM="build --target-dir /managed"
        )
        self.assertEqual(result.returncode, 2)

    def test_builtin_command_takes_precedence_over_same_named_alias(self) -> None:
        result = self.run_adapter(
            "check", CORRAL_BUILD_CACHE="mbx", CARGO_ALIAS_CHECK="build --target-dir /managed"
        )
        self.assertEqual(result.returncode, 19, result.stderr)

    def test_dotted_alias_cannot_bypass_configuration_checks(self) -> None:
        result = self.run_adapter(
            "unsafe.nested", CORRAL_BUILD_CACHE="mbx",
            CARGO_ALIAS_UNSAFE_NESTED="build --target-dir /managed",
        )
        self.assertEqual(result.returncode, 2)

    def test_unicode_alias_normalization_cannot_bypass_configuration_checks(self) -> None:
        result = self.run_adapter(
            "\u1c8a", CORRAL_BUILD_CACHE="mbx",
            **{"CARGO_ALIAS_\u1c89": "build --target-dir /managed"},
        )
        self.assertEqual(result.returncode, 2)

    def test_cargo_config_includes_fail_closed(self) -> None:
        cargo_home = self.root / "cargo-home"
        cargo_home.mkdir()
        _ = (cargo_home / "config.toml").write_text('include = ["other.toml"]\n')
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 2)

    def test_existing_managed_records_are_not_exposed_to_automatic_gc(self) -> None:
        records = self.root / "cache" / "corral-disabled-targets" / "v1"
        records.mkdir(parents=True)
        record = records / ("d" * 64 + ".json")
        _ = record.write_text("{}")
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 2)
        self.assertTrue(record.exists())

    def test_managed_target_root_cannot_redirect_automatic_gc(self) -> None:
        cache = self.root / "cache"
        cache.mkdir()
        (cache / "corral-disabled-targets").symlink_to(self.root, target_is_directory=True)
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 2)

    def test_explicit_target_overrides_managed_default_target(self) -> None:
        view = self.root / "managed" / "v1" / ("c" * 64)
        view.mkdir(parents=True)
        _ = view.with_suffix(".json").write_text("{}")
        result = self.run_adapter(
            "check", "--target-dir", str(self.root / "private-target"),
            CORRAL_BUILD_CACHE="mbx", FIXTURE_TARGET=str(view),
        )
        self.assertEqual(result.returncode, 19, result.stderr)
        result = self.run_adapter(
            "check", "--target-dir", str(self.root / "private-target"),
            CORRAL_BUILD_CACHE="mbx", FIXTURE_TARGET=str(view), FIXTURE_BUILD=str(view),
        )
        self.assertEqual(result.returncode, 2)

    def test_cargo_help_does_not_require_a_workspace(self) -> None:
        self.environment["FIXTURE_WORKSPACE"] = str(self.root / "missing")
        result = self.run_adapter("--help", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(result.stdout.strip(), "--help")

    def test_mise_cargo_wrapper_cannot_change_launcher_after_preflight(self) -> None:
        wrapper_dir = self.root / "command-wrappers" / "bin"
        wrapper_dir.mkdir(parents=True)
        self.write_executable(wrapper_dir / "cargo", "#!/bin/sh\nexit 29\n")
        result = self.run_adapter(
            "check",
            CORRAL_BUILD_CACHE="mbx",
            PATH=f"{wrapper_dir}:{self.environment['PATH']}",
        )
        self.assertEqual(result.returncode, 2)

    def test_mbx_cargo_shim_cannot_change_launcher_after_preflight(self) -> None:
        data = self.root / "data"
        shim_dir = data / "mbx" / "bin"
        shim_dir.mkdir(parents=True)
        self.write_executable(shim_dir / "cargo", "#!/bin/sh\nexit 29\n")
        result = self.run_adapter(
            "check",
            CORRAL_BUILD_CACHE="mbx",
            XDG_DATA_HOME=str(data),
            PATH=f"{shim_dir}:{self.environment['PATH']}",
        )
        self.assertEqual(result.returncode, 2)

    def test_terminating_adapter_terminates_running_backend(self) -> None:
        self.write_executable(
            self.mbx,
            """#!/usr/bin/env python3
import signal, sys
if sys.argv[1:] == ["--version"]:
    print("mbx 1.29.2")
else:
    print("ready", flush=True)
    signal.pause()
""",
        )
        process: subprocess.Popen[str] = subprocess.Popen(
            ["bash", str(ADAPTER), "check"], cwd=self.work,
            env={**self.environment, "CORRAL_BUILD_CACHE": "mbx"},
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            assert process.stdout is not None
            readable, _, _ = select.select([process.stdout.fileno()], [], [], 10)
            self.assertEqual(readable, [process.stdout.fileno()])
            self.assertEqual(process.stdout.readline(), "ready\n")
            process.terminate()
            _, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, -15, stderr)
        finally:
            if process.poll() is None:
                process.kill()
                _ = process.communicate(timeout=10)

    def test_existing_managed_target_is_rejected_but_ordinary_link_is_allowed(self) -> None:
        regular = self.root / "ordinary"
        regular.mkdir()
        self.target.symlink_to(regular, target_is_directory=True)
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 19, result.stderr)
        self.target.unlink()

        view = self.root / "managed" / "v1" / ("a" * 64)
        view.mkdir(parents=True)
        _ = view.with_suffix(".json").write_text("{}")
        self.target.symlink_to(view, target_is_directory=True)
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.target.unlink()
        result = self.run_adapter("check", CORRAL_BUILD_CACHE="mbx", FIXTURE_BUILD=str(view))
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    _ = unittest.main()
