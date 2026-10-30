#!/usr/bin/env python3
"""Deterministic checks of the operating-launch barrier and JSON publication."""
import importlib.util
import json
import os
from pathlib import Path
import selectors
import socket
import tempfile
import unittest
from unittest.mock import MagicMock, patch

SOURCE = Path(os.environ.get("CORRAL_FIXTURE_SOURCE", str(
    Path(__file__).resolve().parents[1] / "crates/corral-ops/tests/operating_launch/herdr.py"
)))
spec = importlib.util.spec_from_file_location("launch_fixture", SOURCE)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class LaunchFixtureTests(unittest.TestCase):
    def test_main_does_not_apply_an_ipc_timeout_to_controller_work(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "barrier.sock").touch()
            channel = MagicMock()
            channel.__enter__.return_value = channel
            def release(_):
                # Model a peer arriving after the old 10 s IPC window, without
                # burning time or relying on the scheduler to reproduce it.
                channel.settimeout.assert_not_called()
                return b"R"
            channel.recv.side_effect = release
            with patch.object(fixture.sys, "argv", [str(root / "herdr"), "pane", "report-metadata"]), \
                 patch.object(fixture.socket, "socket", return_value=channel), \
                 patch.object(selectors, "DefaultSelector") as factory, \
                 patch("builtins.print"):
                factory.return_value.__enter__.return_value.select.return_value = [(channel, selectors.EVENT_READ)]
                fixture.main()
            channel.recv.assert_called_once_with(1)

    def test_release_is_an_event_with_a_full_controller_budget(self):
        controller, peer = socket.socketpair()
        with controller, peer:
            controller.sendall(b"R")
            # Still use the real selector to observe the event, without waiting
            # 10 seconds or depending on scheduler speed for the regression.
            with patch.object(selectors, "DefaultSelector", wraps=selectors.DefaultSelector) as factory:
                fixture.wait_for_release(peer)
                self.assertTrue(factory.called)
            self.assertIsNone(peer.gettimeout())

    def test_missing_release_is_bounded_and_reports_the_controller(self):
        controller, peer = socket.socketpair()
        with controller, peer:
            with patch.object(selectors, "DefaultSelector") as factory:
                events = factory.return_value.__enter__.return_value
                events.select.return_value = []
                with self.assertRaisesRegex(TimeoutError, "controller did not release"):
                    fixture.wait_for_release(peer)
                events.register.assert_called_once_with(peer, selectors.EVENT_READ)
                events.select.assert_called_once_with(fixture.BARRIER_BUDGET)
                self.assertGreaterEqual(fixture.BARRIER_BUDGET, 120)

    def test_controller_death_is_not_a_release(self):
        controller, peer = socket.socketpair()
        controller.close()
        with peer, self.assertRaisesRegex(AssertionError, "disconnected"):
            fixture.wait_for_release(peer)

    def test_readers_only_see_complete_published_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "panes.json"
            path.write_text('["old"]\n')
            original = Path.write_text
            def interrupted_write(target, text):
                self.assertNotEqual(target, path)
                original(target, text[:1])
                self.assertEqual(json.loads(path.read_text()), ["old"])
                return original(target, text)
            with patch.object(Path, "write_text", interrupted_write):
                fixture.publish(path, ["new"])
            self.assertEqual(json.loads(path.read_text()), ["new"])
            self.assertEqual(list(Path(directory).iterdir()), [path])


if __name__ == "__main__":
    unittest.main()
