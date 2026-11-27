"""Fixture controller running inside the disposable outer Bubblewrap namespace."""
import base64
import secrets
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path

from approval import Approval, fresh
from child import ARGS, Daemon, exec_daemon
from environment import (DAEMON_ENV, PACKAGE, SCRATCH, Teardown, Unsupported, assert_no_workers,
                         observe_payloads, preflight, seed_canaries, wait_namespace_empty)
from protocol import CANARY, FAULT_SCENARIOS, PORT, PROFILE_SHA, SCENARIOS, Failure, Fixture, hex_digest, ident, keys, load, need, sha, wire
from responder import Conversation, ScriptedHTTP

FIXTURE = Path("/fixture/meta")
STATE = Path("/state")


def interrupt(_sig, _frame):
    raise Failure("fixture_interrupted")


def fail_closed(_):
    raise Teardown("provider_thread_not_joined")


def delivery(peer, approval, script, snapshot):
    need(keys(snapshot, "run_id", "generation", "seq", "state", "reason", "offer", "acked")
         and snapshot["run_id"] == approval.run and snapshot["generation"] == 1
         and snapshot["state"] == "finished" and snapshot["reason"] is None
         and snapshot["acked"] is False, "finished_snapshot")
    offer = snapshot["offer"]
    need(keys(offer, "corral_instance", "run_id", "generation", "report_id", "sha256", "bytes")
         and offer["corral_instance"] == peer.enrollment["corral_instance"]
         and offer["run_id"] == approval.run and offer["generation"] == 1
         and ident(offer["report_id"]) and hex_digest(offer["sha256"])
         and type(offer["bytes"]) is int and 0 < offer["bytes"] <= 65536, "report_offer")
    fetch = {key: offer[key] for key in ("run_id", "generation", "report_id")}
    peer.ask("fetch_report", dict(fetch, report_id=fresh()), error="not_found")
    got = peer.ask("fetch_report", fetch)
    need(keys(got, "offer", "report_b64") and got["offer"] == offer
         and type(got["report_b64"]) is str, "report_fetch")
    raw = base64.b64decode(got["report_b64"], validate=True)
    need(base64.b64encode(raw).decode("ascii") == got["report_b64"]
         and len(raw) == offer["bytes"] and sha(raw) == offer["sha256"]
         and raw == wire(dict(script.report, v=1, kind="investigation_report")), "sealed_report_bytes")
    # Simulate the owning peer verifying custody, not a first-party Lite import.
    ack = dict(fetch, sha256=offer["sha256"], bytes=len(raw))
    peer.ask("ack_report", dict(ack, sha256=sha(raw + b"!")), error="conflict")
    need(peer.ask("read_investigation", {"run_id": approval.run}) == snapshot, "rejected_ack_changed_snapshot")
    receipt = {"offer": offer, "acked": True}
    need(peer.ask("ack_report", ack) == receipt and peer.ask("ack_report", ack) == receipt, "report_ack_receipt")
    acked = dict(snapshot, acked=True, seq=snapshot["seq"] + 1)
    need(peer.ask("read_investigation", {"run_id": approval.run}) == acked
         and peer.ask("cancel_investigation", {"run_id": approval.run, "generation": 1}) == acked
         and peer.ask("fetch_report", fetch) == got, "report_ack_or_cancel_changed_retained_report")
    return offer, raw


def durable(root, approval, conversation, terminal, receipts, report):
    db_path = root / "lite-analyst.sqlite3"
    with sqlite3.connect(db_path.as_uri() + "?mode=ro", uri=True) as db:
        runs = db.execute("SELECT run_id, generation, state, reason, launch_phase FROM runs").fetchall()
        ledger = db.execute("SELECT step, consumed, input_digest, outcome, budget_charge, receipt_observed "
                            "FROM turn_ledger ORDER BY step").fetchall()
        epochs = db.execute("SELECT count(*) FROM epochs").fetchone()[0]
        reports = db.execute("SELECT run_id,generation,report_id,sha256,bytes,acked,removed FROM reports").fetchall()
    expected_reports = [] if report is None else [(
        approval.run, 1, report[0]["report_id"], report[0]["sha256"], report[0]["bytes"], 1, 0)]
    need(reports == expected_reports, "durable_report_offer_or_ack")
    need(runs == [(approval.run, 1, *terminal, "cleaned")], "durable_run_not_cleaned")
    need(epochs == (2 if approval.scenario in ("reconnect", "crash") else 1), "durable_epochs")
    settles = approval.settles[:len(conversation.digests)]
    want = [(i, 1, body_digest, settle["body"]["outcome"], settle["body"]["charged_microusd"], receipts)
            for i, (body_digest, settle) in enumerate(zip(conversation.digests, settles, strict=True), 1)]
    need(ledger == want, "durable_turn_accounting")
    need({p.name for p in root.iterdir()} <= {db_path.name, "lite-analyst.lock", "daemon.log", "reconnect.log", "reports"},
         "unexpected_persistent_artifact")
    directory = root / "reports"
    paths = list(directory.iterdir()) if directory.exists() else []
    if report is not None:
        offer, raw_report = report
        path = directory / offer["report_id"]
        need(paths == [path] and path.lstat().st_mode == (stat.S_IFREG | 0o400)
             and path.read_bytes() == raw_report, "report_original_not_retained")
    else:
        need(not paths, "unpublished_report_artifact")
    raw = db_path.read_bytes()
    need(CANARY.encode() not in raw and approval.question.encode() not in raw
         and conversation.fixture.prompt.encode() not in raw
         and conversation.fixture.samples["read"]["text"].encode() not in raw
         and (conversation.report is None or conversation.report["text"].encode() not in raw),
         "unexpected_persisted_content")
    return sum(row[4] for row in ledger)


def terminal(peer, approval, expected_turns):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if approval.scenario == "reconnect" or len(approval.settles) < expected_turns:
            # Drive the reverse calls without queuing status at Corral's send
            # boundary (which refuses any unprocessed control frame). Read as
            # soon as the final settlement barrier has been acknowledged.
            got = peer.pump(0.1)
            need(got is None, "unsolicited_response")
            if peer.dropped_settlement:
                return None
        else:
            result = peer.ask("read_investigation", {"run_id": approval.run})
            if result["state"] in ("failed", "cancelled", "finished"):
                return result
            peer.pump(0.05)
    raise Failure("run_terminal_timeout")


def scenario(root, name, fixture, manifest_sha, lock_sha):
    # Reset private durable state between lanes; reconnect reuses all named pins/store.
    approval = Approval(fixture, name)
    posted = threading.Event()
    release_provider = threading.Event()

    def observe(step, digest):
        if step == 1:
            observe_payloads()
        # This runs at actual wire receipt, not after the investigation exits.
        # The exact body must already have a matching permit durably consumed.
        db_uri = (root / "lite-analyst.sqlite3").as_uri() + "?mode=ro"
        with sqlite3.connect(db_uri, uri=True, timeout=1) as db:
            row = db.execute("SELECT consumed,input_digest,permit_id,outcome FROM turn_ledger WHERE step=?",
                             (step,)).fetchone()
        need(step <= len(approval.reserves), "provider_sent_without_permit")
        permit = approval.reserves[step - 1][1]
        need(row == (1, digest, permit["permit_id"], None), "provider_sent_before_consumption")
        if name == "crash":
            posted.set()
            need(release_provider.wait(12), "crash_barrier_not_released")

    script = Conversation(fixture, name, approval.question, observe=observe)
    bearer = secrets.token_hex(32).encode()
    enrollment = {"v": 1, "type": "enrollment", "role": "corral", "link_id": fresh(),
                  "lite_instance": fresh(), "corral_instance": fresh(), "channel_epoch": fresh(),
                  "prior_channel_epoch": None, "profile_sha256": PROFILE_SHA,
                  "corral_manifest_sha256": manifest_sha, "joint_lock_sha256": lock_sha}
    server = ScriptedHTTP(script, bearer, fail_closed).serve()
    daemon = None
    report = None
    try:
        daemon = Daemon(root, enrollment, bearer, fixture, approval, script, False)
        daemon.boot()
        first = daemon.peer.ask("start_investigation", approval.start)
        need(first["state"] == "admitted" and first["generation"] == 1 and first["offer"] is None,
             "admitted_snapshot")
        approval.run = first["run_id"]
        if name == "crash":
            deadline = time.monotonic() + 15
            while not posted.is_set():
                if server.error is not None:
                    raise server.error
                need(time.monotonic() < deadline and daemon.peer.pump(0.1) is None,
                     "crash_post_not_observed")
            # The responder holds the single accepted POST; consume won, no
            # response or settlement has occurred, and both children are live.
            daemon.process.kill()
            daemon.close(expected=-signal.SIGKILL)
            release_provider.set()
            snapshot = None
        else:
            snapshot = terminal(daemon.peer, approval, script.available_turns)
        need(daemon.peer.handshakes == {"evidence": 3, "host_poll": 0 if name == "registry" else 3},
             "broker_negotiation_not_exercised")
        if name in ("reconnect", "crash"):
            if name == "reconnect":
                # Observe Work EOF as the sole stop first. Closing the driver
                # lifetime before the daemon exits can race and be graceful.
                daemon.process.wait(timeout=5)
                daemon.close(expected=1)
            enrollment = dict(enrollment, prior_channel_epoch=enrollment["channel_epoch"], channel_epoch=fresh())
            daemon = Daemon(root, enrollment, bearer, fixture, approval, script, True)
            daemon.boot()
            # Work pump accepts the sealed receipt replay or the new
            # conservative settlement of the crashed consumed step.
            snapshot = daemon.peer.ask("start_investigation", approval.start)
            need(snapshot["run_id"] == approval.run and snapshot["generation"] == 1, "reconnect_start_identity")
            daemon.peer.ask("start_investigation", approval.start_params(question="different request"), error="conflict")
            daemon.peer.ask("start_investigation", approval.start_params(request_id=fresh()), error="stopped")
            need(len(approval.settles) == (2 if name == "reconnect" else 1), "missing_reconciled_settlement")
            final_state = (snapshot["state"], snapshot["reason"])
            need(final_state in (("cancelled", "stopped"), ("failed", "unavailable")), "reconnect_not_fenced")
        elif name in ("happy", "pending"):
            final_state = ("finished", None)
        else:
            final_state = ("failed", "unavailable")
        need((snapshot["state"], snapshot["reason"]) == final_state, "run_terminal_state")
        if final_state[0] != "finished":
            need(snapshot["offer"] is None and snapshot["acked"] is False, "failure_published_offer")
        assert_no_workers(daemon.process.pid)
        need(len(script.digests) == len(approval.reserves) == script.available_turns, "provider_turn_count")
        expected_calls = {"happy": 3, "pending": 4, "repeated_id": 1}.get(name, 0)
        need(approval.binds == approval.attachments == 1 and len(approval.broker_calls) == expected_calls,
             "broker_call_count")
        need(script.derived == (name in ("happy", "pending")), "finish_was_not_derived")
        if script.derived:
            need({c["handle"] for c in script.report["citations"]} == {approval.field_cite, approval.metric_cite},
                 "finish_used_wrong_citations")
            report = delivery(daemon.peer, approval, script, snapshot)
        # Keep sockets open until orderly daemon exit, unlike the negative reconnect barrier.
        daemon.close(expected=0)
        daemon = None
    finally:
        release_provider.set()
        try:
            if daemon is not None:
                daemon.close()
        finally:
            server.close()
    # Rebinding the exact port proves the original listener is closed. No extra HTTP probe.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as conn:
        conn.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        conn.bind(("127.0.0.1", PORT))
    need(not (SCRATCH / "owner-command-ran").exists(), "owner_mcp_executed")
    cost = durable(root, approval, script, final_state, 1, report)
    result = {"status": "pass", "provider_sends": len(script.digests), "charged_microusd": cost,
              "broker_deliveries": len(approval.broker_calls),
              "settlement_replays": int(name == "reconnect"), "crash_recovery": int(name == "crash"),
              "workers_reaped": True,
              "payload_namespace_observation": "pass" if script.digests else "not_run",
              "synthetic_captured_only": True, "terminal": final_state[1] or final_state[0]}
    if report is not None:
        result["corral_report"] = {"sha256": report[0]["sha256"], "bytes": report[0]["bytes"],
                                   "owning_peer_ack": True, "original_retained": True}
    return result


def main():
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupt)
    result = {"status": "fail", "lanes": {}}
    active = "isolation"
    try:
        need(sys.argv[1:] == ["--inside"], "controller_interface")
        wait_namespace_empty()
        result["landlock_abi"] = preflight()
        seed_canaries()
        with (STATE / "disabled.log").open("xb") as log:
            disabled = subprocess.run(ARGS, cwd=SCRATCH, env=DAEMON_ENV, stdin=subprocess.DEVNULL,
                                      stdout=log, stderr=log, timeout=5, check=False)
        log = STATE / "disabled.log"
        need(disabled.returncode == 1 and log.stat().st_size <= 4096
             and log.read_bytes() == b"Error: public Lite fixture requires explicit per-launch opt-in\n"
             and list(STATE.iterdir()) == [log], "disabled_opt_in_gate")
        log.unlink()
        fixture = Fixture(FIXTURE / "february.json", PACKAGE / "assets/synthetic-prompt.txt",
                          PACKAGE / "assets/tool-policy.json")
        pin = sha((FIXTURE / "build-manifest.json").read_bytes())
        plan = load((FIXTURE / "qa-lock.json").read_bytes())
        lock = sha((FIXTURE / "qa-lock.json").read_bytes())
        need(plan["manifest_sha256"] == pin, "manifest_lock")
        lane = scenario
        allowed = SCENARIOS
        if plan.get("variant") == "corral-fault-v1":
            from fault import scenario as fault_scenario
            lane = fault_scenario
            allowed = FAULT_SCENARIOS
        need(bool(plan["scenarios"]) and set(plan["scenarios"]) <= set(allowed), "scenario_variant_mismatch")
        for active in plan["scenarios"]:
            # C2 requires exactly /state. Reset it only after confirmed teardown.
            result["lanes"][active] = lane(STATE, active, fixture, pin, lock)
            for path in STATE.iterdir():
                if path.name == "reports":
                    # Explicit disposable fixture cleanup, not product deletion through ack.
                    for report_path in path.iterdir():
                        report_path.unlink()
                    path.rmdir()
                else:
                    path.unlink()
        result["status"] = "pass"
        code = 0
    except Unsupported as exc:
        result.update(error=str(exc), case=active)
        code = 69
    except Teardown as exc:
        result.update(error=str(exc), case=active)
        code = 74
    except Failure as exc:
        result.update(error=str(exc), case=active)
        code = 70
    except Exception as exc:
        result.update(error=type(exc).__name__, case=active)
        code = 70
    try:
        wait_namespace_empty()
    except Teardown as exc:
        result.update(status="fail", error=str(exc), case="teardown")
        code = 74
    print(wire(result).decode(), flush=True)
    return code


if __name__ == "__main__":
    if sys.argv[1:2] == ["--exec-daemon"]:
        exec_daemon(sys.argv[2:])
    else:
        sys.exit(main())
