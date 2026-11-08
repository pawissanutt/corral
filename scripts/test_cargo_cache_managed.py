#!/usr/bin/env python3
"""One real-mbx journey: private candidates, workflow leases, then collection.

Opt in with CORRAL_TEST_MBX_BIN=/absolute/path/to/mbx (must be 1.29.2).
All build outputs and collection are confined to one temporary cache.
"""

import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent


class ManagedCacheJourney(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux" and os.environ.get("CORRAL_TEST_MBX_BIN"),
                         "set CORRAL_TEST_MBX_BIN to exercise pinned mbx on Linux")
    def test_candidates_survive_collection_until_workflow_exits(self) -> None:
        with tempfile.TemporaryDirectory(prefix="corral-mbx-managed-") as temporary:
            root = Path(temporary)
            environment = {
                key: value for key, value in os.environ.items()
                if not key.startswith(("MBX_", "CORRAL_MBX_", "CORRAL_BUILD_CACHE", "CARGO_TARGET_DIR"))
            }
            environment.update({
                "CORRAL_BUILD_CACHE": "mbx", "CORRAL_MBX_TARGETS": "managed",
                "CORRAL_MBX_BIN": os.environ["CORRAL_TEST_MBX_BIN"],
                "CORRAL_MBX_CACHE_DIR": str(root / "cache"),
                "XDG_CONFIG_HOME": str(root / "config"),
                "RUSTC_WRAPPER": "", "CARGO_ENCODED_RUSTFLAGS": "",
            })
            checkouts = [root / "a", root / "b"]
            for checkout, value in zip(checkouts, (11, 22), strict=True):
                (checkout / "src").mkdir(parents=True)
                (checkout / "Cargo.toml").write_text(
                    '[package]\nname = "lease-fixture"\nversion = "0.1.0"\nedition = "2024"\n'
                )
                (checkout / "src/main.rs").write_text(f'fn main() {{ println!("{value}"); }}\n')
                (checkout / ".cargo").mkdir()
                (checkout / ".cargo/config.toml").write_text('[build]\nrustc-wrapper = ""\n')
            workflow = root / "workflow.sh"
            workflow.write_text(
                '#!/usr/bin/env bash\nset -euo pipefail\n'
                '[[ $1 == --corral-mbx-protected ]]\nshift\n'
                f'python3 "{SCRIPTS}/cargo_cache_targets.py" --verify-protection\n'
                f'bash "{SCRIPTS}/cargo.sh" build --offline --quiet\n'
                # Synchronize the deliberate gap between build and execution.
                'printf "ready\\n"\nread -r release\n'
                '[[ $release == release ]]\n./target/debug/lease-fixture\n'
            )
            children: list[subprocess.Popen[str]] = []
            errors = []
            try:
                for checkout in checkouts:
                    error_log = tempfile.TemporaryFile(mode="w+")
                    errors.append(error_log)
                    child = subprocess.Popen(
                        [sys.executable, str(SCRIPTS / "cargo_cache.py"),
                         "--protect-script", str(workflow)],
                        cwd=checkout, env=environment, stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE, stderr=error_log,
                        text=True,
                    )
                    children.append(child)
                    self.assertIsNotNone(child.stdout)
                    self.assertTrue(select.select([child.stdout], [], [], 30)[0], "workflow did not become ready")
                    line = child.stdout.readline()
                    error_log.seek(0)
                    self.assertEqual(line, "ready\n", error_log.read())
                views = [(checkout / "target").resolve() for checkout in checkouts]
                self.assertNotEqual(*views)
                self.assertTrue(all((checkout / "target").is_symlink() for checkout in checkouts))
                # Remove the collector's two-second recent-placement grace period;
                # the lease alone must protect these disposable fixture targets.
                for view in views:
                    record_path = view.with_suffix(".json")
                    record = json.loads(record_path.read_text())
                    record["updated_secs"] = 0
                    record_path.write_text(json.dumps(record))
                gc_environment = {
                    **environment, "MBX_CACHE_DIR": str(root / "cache"),
                    "MBX_TARGET_ROOT": str(root / "cache/corral-managed-targets"),
                    "MBX_TARGET_MAX_SIZE": "1B", "MBX_GC_AUTO": "0",
                }
                collect = [environment["CORRAL_MBX_BIN"], "gc"]
                result = subprocess.run(collect, env=gc_environment, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(all(view.is_dir() for view in views), result.stderr)
                for child, error_log, expected in zip(children, errors, ("11\n", "22\n"), strict=True):
                    output, _ = child.communicate("release\n", timeout=30)
                    error_log.seek(0)
                    self.assertEqual(child.returncode, 0, error_log.read())
                    self.assertEqual(output, expected)
                result = subprocess.run(collect, env=gc_environment, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(all(not view.exists() for view in views), result.stderr)
                # A collected target's dangling link must be safe to rebuild,
                # without needing its deleted ownership record to find the lease.
                result = subprocess.run(
                    [sys.executable, str(SCRIPTS / "cargo_cache.py"),
                     "--protect-script", str(workflow)],
                    cwd=checkouts[0], env=environment, input="release\n",
                    capture_output=True, text=True, timeout=30,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "ready\n11\n")
            finally:
                for child in children:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)
                    if child.stdin is not None:
                        child.stdin.close()
                    if child.stdout is not None:
                        child.stdout.close()
                for error_log in errors:
                    error_log.close()


if __name__ == "__main__":
    unittest.main()
