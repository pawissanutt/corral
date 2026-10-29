#!/usr/bin/env python3
"""Verdict-note compatibility checks; run directly with Python."""

import collections
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "measure-procedure-friction.py"
SPEC = importlib.util.spec_from_file_location("procedure_friction", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
mpf = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mpf)

NOTES = [text + "context\n" * 2500 for text in [
    "# Independent review\n\n## Findings\n\n### Missing gate evidence\n\nThe gate evidence is missing.\n",
    "# Independent review\n\n## Findings\n\n### Owner decision required\n\nChoose the integration policy.\n",
    "# Independent review\n\n## Findings\n\n### [P2] Incorrect result\n\nRepair src/result.rs:42.\n",
]]


def blob_ref(text):
    return "blob:sha256:" + hashlib.sha256(text.encode()).hexdigest()


def events(externalized):
    return [{
        "seq": index + 1, "kind": "verdict", "task": "T-900",
        "ts": f"2026-10-20T12:0{index}:00Z",
        "detail": {"run": "R1" if index != 1 else "R2", "verdict": "escalate",
                   "notes": blob_ref(text) if externalized else text},
    } for index, text in enumerate(NOTES)]


class VerdictNotesTests(unittest.TestCase):
    def test_blob_notes_keep_run_metrics_and_aggregate_categories(self):
        # Given the same detailed verdicts inline and externalized.
        def metrics(externalized, reader):
            event_rows = collections.defaultdict(list)
            run = {
                "task": "T-900", "run": "R1", "start": mpf.ts("2026-10-20T11:00:00Z"),
                "worktree": "/nonexistent", "project": "corral", "usage": {},
                "ev": event_rows,
            }
            for event in events(externalized):
                event_rows["verdict"].append({**event, "_t": mpf.ts(event["ts"])})
            result = mpf.run_metrics(run, None, {}, reader)
            return result, mpf.agg([{**run, "m": result}])

        stored = {blob_ref(text): text for text in NOTES}
        reader = mock.Mock(side_effect=stored.__getitem__)
        # When both representations pass through the real per-run classifier.
        inline = metrics(False, reader)
        reader.assert_not_called()
        externalized = metrics(True, reader)
        # Then every result and aggregate category stays equal, not unknown.
        self.assertEqual(externalized, inline)
        self.assertEqual([item["kind"] for item in externalized[0]["escalations"]],
                         ["procedural_evidence", "owner_decision", "code_defect"])
        self.assertEqual(reader.call_args_list, [mock.call(blob_ref(text)) for text in NOTES])

    def test_report_resolves_notes_from_full_show_without_mixing_runs(self):
        # Given full CLI history containing older review children and another run.
        runs = [{
            "id": run, "started": f"2026-10-20T1{index}:00:00Z", "worktree": "/nonexistent",
            "review_runs": [{"review": {"notes": text}} for text in
                            ([NOTES[0], NOTES[2]] if run == "R1" else [NOTES[1]])],
        } for index, run in enumerate(["R1", "R2"])]
        task = {"id": "T-900", "title": "Fixture", "runs": runs, "kind": "test", "size": "m"}

        def report(externalized):
            def cli(argv, **kwargs):
                command = argv[1:3]
                if command == ["list", "--all"]:
                    output = json.dumps([{"id": task["id"]}])
                elif command == ["show", task["id"]]:
                    output = json.dumps(task)
                elif command == ["export", "events"]:
                    output = "\n".join(json.dumps(event) for event in events(externalized))
                elif command == ["stats", "--gates"]:
                    output = "{}"
                elif argv[1:] == ["--help"]:
                    output = "Commands:\n  done\n  verdict\nOptions:\n"
                else:
                    self.fail(f"Unsupported readback: {argv}")
                self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
                return subprocess.CompletedProcess(argv, 0, stdout=output)

            with tempfile.TemporaryDirectory(prefix="corral-friction-notes-") as root:
                output = Path(root) / "result.json"
                with mock.patch.object(mpf.subprocess, "run", side_effect=cli), \
                     mock.patch.object(mpf, "scan_transcripts", return_value=None), \
                     mock.patch("sys.argv", [str(SCRIPT), "--corral", "candidate",
                                            "--root", root, "--json", str(output)]), \
                     contextlib.redirect_stdout(io.StringIO()):
                    mpf.main()
                return json.loads(output.read_text())

        # When the complete measurement pipeline consumes each representation.
        inline = report(False)
        externalized = report(True)
        # Then categories, grouping, and original event/run attribution agree.
        for key in ["totals", "by_profile", "by_harness", "by_kind", "by_size",
                    "failure_modes", "escalations", "escalation_kinds"]:
            self.assertEqual(externalized[key], inline[key], key)
        self.assertEqual(externalized["escalation_kinds"],
                         {"procedural_evidence": 1, "owner_decision": 1, "code_defect": 1})
        self.assertEqual([(item["run"], item["kind"]) for item in externalized["escalations"]],
                         [("R1", "procedural_evidence"), ("R1", "code_defect"),
                          ("R2", "owner_decision")])


if __name__ == "__main__":
    unittest.main()
