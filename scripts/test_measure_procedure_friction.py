#!/usr/bin/env python3
"""Deterministic regression tests for measure-procedure-friction.py; run directly with Python."""

import collections
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from typing import Any

SCRIPT = Path(__file__).resolve().with_name("measure-procedure-friction.py")
SPEC = importlib.util.spec_from_file_location("measure_procedure_friction", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
mpf: Any = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mpf)

T0 = dt.datetime(2026, 10, 20, 12, 0, tzinfo=dt.timezone.utc)


def at(seconds: float) -> dt.datetime:
    return T0 + dt.timedelta(seconds=seconds)


def event(kind: str, seconds: float, **detail: Any) -> dict[str, Any]:
    return {"kind": kind, "task": "T-900", "ts": at(seconds).isoformat(), "_t": at(seconds), "detail": detail}


def submission(seconds: float, head: str | None = None, base: str | None = None) -> dict[str, Any]:
    detail: dict[str, Any] = {"run": "R1"}
    if head is not None:
        detail["identity"] = {"head": head}
        detail["integration_base"] = base
    return event("done", seconds, **detail)


def sendback(seconds: float, text: str) -> dict[str, Any]:
    return event("changes_requested", seconds, text=text)


def make_run(events: list[dict[str, Any]], worktree: str = "/nonexistent/.worktrees/T-900") -> dict[str, Any]:
    run: dict[str, Any] = {
        "task": "T-900", "run": "R1", "start": at(0), "worktree": worktree, "project": "corral",
        "usage": {}, "ev": collections.defaultdict(list), "window_end": None,
    }
    for e in events:
        run["ev"][e["kind"]].append(e)
    return run


class FakeAncestry:
    def __init__(self, table: dict[tuple[str, str], bool]) -> None:
        self.table = table
        self.original = mpf.is_ancestor

    def __enter__(self) -> "FakeAncestry":
        mpf.is_ancestor = lambda repo, base, head: self.table.get((base, head))
        return self

    def __exit__(self, *exc: object) -> None:
        mpf.is_ancestor = self.original


REBASE_ASK = "Rebase onto current main (observed abc1234), resolve conflicts, commit, rerun the gate."


class SendbackAttributionTests(unittest.TestCase):
    def test_each_sendback_uses_the_submission_it_concerns(self) -> None:
        # Given a stale first submission, a current second one, and a rebase send-back after each.
        events = [
            submission(600, "h1", "b1"), sendback(1200, REBASE_ASK),
            submission(1800, "h2", "b2"), sendback(2400, REBASE_ASK),
        ]
        run = make_run(events)
        # When metrics attribute the send-backs.
        with FakeAncestry({("b1", "h1"): False, ("b2", "h2"): True, ("b2", "abc1234"): True}):
            m = mpf.run_metrics(run, None, {}, "")
        first, second = m["sendbacks"]
        # Then the first is a stale submission and the second is main moving afterwards.
        self.assertEqual(first["kind"], "rebase_submitted_stale")
        self.assertTrue(first["procedural"])
        self.assertEqual(second["kind"], "rebase_main_moved_after_submit")
        self.assertFalse(second["procedural"])
        self.assertEqual(second["submission_index"], 1)

    def test_first_submission_without_identity_stays_unknown(self) -> None:
        # Given an unmeasurable first submission followed by a measurable stale one.
        events = [
            submission(600), sendback(1200, REBASE_ASK),
            submission(1800, "h2", "b2"), sendback(2400, REBASE_ASK),
        ]
        run = make_run(events)
        # When metrics attribute the send-backs.
        with FakeAncestry({("b2", "h2"): False}):
            m = mpf.run_metrics(run, None, {}, "")
        # Then the actual first submission is unknown, not the first measurable one.
        self.assertIsNone(m["first_submission_stale_base"])
        self.assertFalse(m["done_before_rebase"])
        self.assertEqual([s["kind"] for s in m["sendbacks"]], ["rebase_unknown", "rebase_submitted_stale"])

    def test_remediation_decides_the_class_not_an_incidental_rebase_mention(self) -> None:
        # Given the T-060 style text that mentions a clean rebase but asks for a code fix.
        text = ("PM: your approved candidate failed its integrated gate after the clean rebase onto main b2c4. "
                "Fix the duplicate serde_json.workspace = true in Cargo.toml introduced by integration.")
        # When classified.
        c = mpf.classify_sendback(sendback(0, text), {}, "")
        # Then it is an integration code defect, not a rebase request, and a real request still is.
        self.assertEqual(c["kind"], "code_defect")
        self.assertFalse(c["procedural"])
        self.assertEqual(mpf.classify_sendback(sendback(0, REBASE_ASK), {}, "")["kind"], "rebase_request")
        advanced = "Review result: verdict could not be recorded because main advanced during the review."
        self.assertEqual(mpf.classify_sendback(sendback(0, advanced), {}, "")["kind"], "rebase_request")

    def test_routine_closing_instruction_does_not_turn_a_code_finding_into_a_rebase(self) -> None:
        # Given the T-122 / T-146 shape: a reproduced finding followed by boilerplate `fix, rebase, run, done`.
        t122 = ("Independent re-review requested changes (round 2, last allowed): 1) P2: Update the existing inspector "
                "browser journey to the new qualified inspect response. Full notes: corral show T-122 --json. "
                "Fix all with regressions, rebase, run checks, call done.")
        t146 = ("Independent review R1 requested changes. Blocking P1: normalize TOML key paths before protecting "
                "operating-policy edits. Fix every finding with regressions, rebase, run scripts/gate.sh, then done.")
        # When classified.
        for text in (t122, t146):
            c = mpf.classify_sendback(sendback(0, text), {}, "")
            # Then it is a code defect only; the routine tail is neither a rebase nor procedure time.
            self.assertEqual(c["kind"], "code_defect")
            self.assertEqual(c["components"], ["code"])
            self.assertFalse(c["procedural"])

    def test_code_fix_beyond_the_opening_text_makes_the_sendback_mixed(self) -> None:
        # Given the T-055 shape: a rebase and an evidence ask, with a code change in a later numbered item.
        text = ("Before the PM gates you: T-059 (plan gate) landed on main at 6fe850b after your base 422cc72. "
                "1) Rebase onto 6fe850b; the only conflict is crates/corral/src/main.rs: keep both sides. "
                "2) crates/corral-ops/src/review.rs adds `eprintln!(\"warning: discard {}\")`; report it through "
                "the shared sink, not eprintln!. 3) Full gate in the foreground including your stress run evidence, "
                "then corral done.")
        # When classified and resolved against a stale submission.
        run = make_run([submission(600, "h1", "b1"), sendback(1200, text)])
        with FakeAncestry({("b1", "h1"): False}):
            m = mpf.run_metrics(run, None, {}, "")
        s = m["sendbacks"][0]
        # Then the interval is mixed, not procedure-only, and not counted as procedural time.
        self.assertEqual(s["kind"], "mixed_code_and_procedure")
        self.assertEqual(s["components"], ["code", "evidence", "rebase"])
        self.assertFalse(s["procedural"])
        self.assertEqual(m["sendbacks_procedural"], 0)
        self.assertEqual(m["proc_latency_s"], 0)

    def test_main_not_moved_with_config_change_is_not_queue_drift(self) -> None:
        # Given the T-181 shape: a rebase ask that says main has not moved and a config digest changed.
        text = ("Rebase onto current main (observed ebd0732), resolve conflicts, commit, run the full configured gate, "
                "and obtain a fresh independent review before landing. PM note: the verdict was rejected only for a "
                "changed inputs_digest (config changed at 19:50Z). Main has not moved, so no code change is expected.")
        run = make_run([submission(600, "h1", "b1"), sendback(1200, text)])
        with FakeAncestry({("b1", "h1"): True, ("b1", "ebd0732"): True}):
            m = mpf.run_metrics(run, None, {}, "")
        # Then it is a config/policy re-review, not a rebase after main moved.
        self.assertEqual(m["sendbacks"][0]["kind"], "config_policy_stale")
        self.assertEqual(m["rebase_after_submit_runs"], 0)

    def test_main_advance_needs_evidence_after_a_current_submission(self) -> None:
        # Given a current submission and a rebase ask, with and without a main sha the git history confirms.
        text = "Rebase onto current main (observed ccc3333), resolve conflicts, commit, run the full configured gate."
        vague = "Rebase onto main, resolve conflicts, commit, run the full configured gate."
        events = [submission(600, "h1", "b1"), sendback(1200, text), sendback(1800, vague)]
        # When the cause is resolved.
        with FakeAncestry({("b1", "h1"): True, ("b1", "ccc3333"): True}):
            m = mpf.run_metrics(make_run(events), None, {}, "")
        # Then only the send-back with git-established advance is drift; the other is not established.
        self.assertEqual([s["kind"] for s in m["sendbacks"]],
                         ["rebase_main_moved_after_submit", "rebase_cause_not_established"])
        self.assertEqual(m["sendbacks"][0]["main_advance_basis"], "git")

    def test_rebase_with_evidence_is_its_own_class(self) -> None:
        # Given the T-044 shape: main advanced during review, rebase plus an isolated evidence run, no code.
        text = ("Review result (verdict could not be recorded because main advanced during the review): changes "
                "required, evidence only. 1) Rebase onto current main 943a7d7; resolve conflicts keeping both sides. "
                "2) With the FINAL rebased commit, run in an isolated herdr session and attach the screenshots.")
        run = make_run([submission(600, "h1", "b1"), sendback(1200, text)])
        with FakeAncestry({("b1", "h1"): True}):
            m = mpf.run_metrics(run, None, {}, "")
        s = m["sendbacks"][0]
        self.assertEqual(s["kind"], "rebase_and_evidence")
        self.assertEqual(s["rebase_cause"], "main_moved_after_submit")
        self.assertFalse(s["procedural"])

    def test_first_submission_denominator_counts_only_measurable_first_submissions(self) -> None:
        # Given an unmeasurable first submission and a measurable later one (the T-044 shape).
        run = make_run([submission(600), submission(1800, "h2", "b2")])
        with FakeAncestry({("b2", "h2"): True}):
            m = mpf.run_metrics(run, None, {}, "")
        # Then the run has a measurable submission but no measurable first submission.
        self.assertEqual(m["stale_base_evaluable"], 1)
        self.assertEqual(m["first_submission_evaluable"], 0)
        self.assertEqual(mpf.agg([dict(run, m=m)])["first_submission_evaluable_runs"], 0)


def write_session(root: Path, worktree: str, rows: list[dict[str, Any]]) -> None:
    folder = root / "sessions" / ("--" + worktree.strip("/").replace("/", "-") + "--")
    folder.mkdir(parents=True)
    lines = [{"type": "session_info", "name": "T-901 worker"}]
    lines += rows
    (folder / "session.jsonl").write_text("\n".join(json.dumps(r) for r in lines) + "\n")


def tool_call(call_id: str, seconds: float, code: str) -> dict[str, Any]:
    return {"type": "message", "timestamp": at(seconds).isoformat().replace("+00:00", "Z"),
            "message": {"role": "assistant", "content": [
                {"type": "toolCall", "id": call_id, "name": "eval", "arguments": {"code": code}}]}}


def tool_result(call_id: str, seconds: float, text: str) -> dict[str, Any]:
    return {"type": "message", "timestamp": at(seconds).isoformat().replace("+00:00", "Z"),
            "message": {"role": "toolResult", "toolCallId": call_id, "content": [{"type": "text", "text": text}]}}


class ScanCase(unittest.TestCase):
    def scan(self, rows: list[dict[str, Any]]) -> dict[str, Any]:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        worktree = "/work/.worktrees/T-901"
        write_session(Path(temp.name), worktree, rows)
        previous = os.environ.get("OMO_CODING_AGENT_DIR")
        os.environ["OMO_CODING_AGENT_DIR"] = temp.name
        self.addCleanup(lambda: os.environ.pop("OMO_CODING_AGENT_DIR") if previous is None
                        else os.environ.__setitem__("OMO_CODING_AGENT_DIR", previous))
        run = make_run([], worktree)
        run["window_end"] = at(3600)
        regexes = mpf.build_regexes({"done", "artifact"})
        facts = mpf.scan_transcripts(run, "/home/nobody", *regexes)
        self.assertIsNotNone(facts)
        return facts


class TimeGapTests(ScanCase):
    def test_batched_help_counts_every_invocation_but_charges_one_interval(self) -> None:
        # Given one tool call that asks for two help screens, followed by a call 60 s later.
        code = "run('/h/.cargo/bin/corral done --help'); run('/h/.cargo/bin/corral artifact add --help')"
        facts = self.scan([tool_call("c1", 10, code), tool_result("c1", 11, "usage"), tool_call("c2", 70, "noop")])
        # When metrics are computed from the scan.
        m = mpf.run_metrics(make_run([]), facts, {}, "")
        # Then invocations stay at two while the elapsed interval is charged once.
        self.assertEqual(m["help_calls"], 2)
        self.assertEqual(m["help_rounds"], 1)
        self.assertEqual(m["friction_rounds"], 1)
        self.assertEqual(m["friction_gap_s"], 60)

    def test_overlapping_signals_in_one_round_share_the_interval(self) -> None:
        # Given one call with a help request and an invalid invocation, 720 s before the next call.
        code = "run('/h/.cargo/bin/corral done --help'); run('/h/.cargo/bin/corral frobnicate')"
        clap = "error: unrecognized subcommand 'frobnicate'\n\nUsage: corral [OPTIONS] [COMMAND]"
        facts = self.scan([tool_call("c1", 10, code), tool_result("c1", 11, clap), tool_call("c2", 730, "noop")])
        run = make_run([])
        run["m"] = mpf.run_metrics(run, facts, {}, "")
        # When the failure modes allocate the round's time.
        modes = mpf.failure_modes({("T-900", "R1"): run})
        # Then the time is split across the signals, not charged to each in full.
        self.assertEqual(run["m"]["friction_gap_s"], 720)
        self.assertEqual(len([m for m in modes if m["rounds"] == 1]), 2)
        self.assertAlmostEqual(sum(m["time_lost_h"] for m in modes), 0.2)


class InvalidInvocationTests(ScanCase):
    CLAP = "error: unrecognized subcommand '{}'\n\nUsage: corral [OPTIONS] [COMMAND]"

    def invalid(self, code: str, result: str) -> int:
        facts = self.scan([tool_call("c1", 10, code), tool_result("c1", 11, result), tool_call("c2", 70, "noop")])
        return len(facts["invalid"])

    def test_unknown_verb_is_invalid_even_when_absent_from_the_valid_verb_set(self) -> None:
        self.assertEqual(self.invalid("run('/h/.cargo/bin/corral frobnicate')", self.CLAP.format("frobnicate")), 1)

    def test_argv_form_of_the_installed_binary_is_recognised(self) -> None:
        code = "Bun.spawn(['/h/.cargo/bin/corral','completion','--help'], {stdout: 'pipe'})"
        self.assertEqual(self.invalid(code, self.CLAP.format("completion")), 1)

    def test_source_mentions_dev_builds_and_unrelated_errors_are_excluded(self) -> None:
        error = self.CLAP.format("frobnicate")
        # A grep over source, a cargo-built dev binary, and a usage error about another token are not invalid calls.
        self.assertEqual(self.invalid('rg -n "corral frobnicate" crates', error), 0)
        self.assertEqual(self.invalid("cargo run -p corral -- frobnicate # corral frobnicate", error), 0)
        self.assertEqual(self.invalid("run('/h/.cargo/bin/corral done T-1')", self.CLAP.format("other")), 0)
        self.assertEqual(self.invalid("cat crates/corral/src/main.rs", error), 0)
        # A deliberate negative test through a dev-binary alias, labelled `$ corral $a`, is not a worker mistake.
        self.assertEqual(self.invalid('for a in x; do echo "\\\\$ corral $a"; $C $a; done', error), 0)

    def test_error_read_in_a_later_call_correlates_with_the_earlier_invocation(self) -> None:
        # Given a monitored `corral wait --for bogus` call, then a later call reading its failed output.
        started = "tool.monitor({command: '/h/.cargo/bin/corral wait --for bogus_kind --json'})"
        reread = "print(tool.bash_output({bash_id: 'b1'})); run('/h/.cargo/bin/corral wait --help')"
        error = "error: invalid value 'bogus_kind' for '--for <KINDS>'\n\nUsage: corral wait [OPTIONS]"
        rows = [tool_call("c0", 5, started), tool_result("c0", 6, "Monitor started"),
                tool_call("c1", 10, reread), tool_result("c1", 11, error), tool_call("c2", 70, "noop")]
        self.assertEqual(len(self.scan(rows)["invalid"]), 1)


ESCALATE_PROCEDURAL = """# T-902 R1 independent review

Verdict: ESCALATE.

## Finding requiring owner resolution

### The current machine gate does not establish the required herdr isolation

The proposal closeout itself is satisfactory. The remaining concern is the gate path.
"""
ESCALATE_OWNER = """# T-903 R1 independent review

Verdict: ESCALATE.

## Finding E1: owner integration decision still blocks the baseline

Acceptance criteria 4 and 7 are not satisfied.
"""
ESCALATE_CODE = """# T-904 R1 independent review

Verdict: escalate.

## Finding

### [P2] Changing a queued escalation retains the obsolete switch

Locations: `crates/corral-ops/src/routing.rs:575-585`.
"""


class EscalationTests(unittest.TestCase):
    def test_classifier_separates_procedural_owner_and_code_escalations(self) -> None:
        kinds = [mpf.classify_escalation(t)["kind"] for t in (ESCALATE_PROCEDURAL, ESCALATE_OWNER, ESCALATE_CODE)]
        self.assertEqual(kinds, ["procedural_evidence", "owner_decision", "code_defect"])
        self.assertEqual(mpf.classify_escalation("no findings section")["kind"], "unknown")

    def test_semantic_verdicts_are_counted_apart_from_watchdog_events(self) -> None:
        # Given an escalate verdict, a changes verdict, and a mechanical reviewer watchdog event.
        events = [
            event("verdict", 100, verdict="escalate", notes=ESCALATE_PROCEDURAL),
            event("verdict", 200, verdict="changes", notes=ESCALATE_CODE),
            event("review_escalated", 300, reason="the reviewer closed without a verdict"),
            submission(700),
        ]
        run = make_run(events)
        # When metrics and system noise are computed.
        m = mpf.run_metrics(run, None, {}, "")
        noise = mpf.system_noise([e for e in events if e["kind"] == "review_escalated"], None)
        # Then only the escalate verdict is an escalation, with its latency to resubmission.
        self.assertEqual([e["kind"] for e in m["escalations"]], ["procedural_evidence"])
        self.assertEqual(m["escalations"][0]["latency_s"], 600)
        self.assertEqual(sum(noise.values()), 1)
        agg = mpf.agg([dict(run, m=m)])
        self.assertEqual(agg["escalations"], 1)
        self.assertEqual(agg["escalations_procedural_evidence"], 1)


if __name__ == "__main__":
    unittest.main()
