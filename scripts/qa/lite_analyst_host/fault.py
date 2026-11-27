"""Three real-Corral fault lanes; the owning Lite peer remains synthetic."""
import secrets
import select
import socket
import sqlite3
import time
from contextlib import closing

from approval import Approval, fresh
from child import Daemon
from environment import SCRATCH, Teardown, assert_no_workers
from protocol import FAULT_BROKER, FAULT_SANDBOX, FAULT_START, PORT, PROFILE_SHA, Failure, ident, keys, need
from responder import Conversation, ScriptedHTTP


def inspect(root, approval):
    uri = (root / "lite-analyst.sqlite3").as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True, timeout=1)) as db:
        db.execute("BEGIN")
        runs = db.execute("SELECT run_id,request_id,request_digest,generation,state,reason,launch_phase,launch_id "
                          "FROM runs").fetchall()
        turns = db.execute("SELECT run_id,generation,step,consumed,input_digest,permit_id,reserve_digest,"
                           "outcome,settle_digest,budget_charge,receipt_observed,cutoff_defaulted "
                           "FROM turn_ledger ORDER BY step").fetchall()
        need(db.execute("SELECT count(*) FROM reports").fetchone() == (0,), "fault_report_published")
        need(db.execute("SELECT count(*) FROM epochs").fetchone() == (1,), "fault_epoch_count")
    paths = root / "reports"
    need(not paths.exists() or not list(paths.iterdir()), "fault_report_artifact")
    need(len(runs) == 1 and ident(runs[0][0]) and
         runs[0][1:4] == (approval.approval["request_id"], approval.start["request_digest"], 1), "fault_start_identity")
    return runs[0], turns


def barrier(daemon, name, *, start):
    if not start:
        until = time.monotonic() + 15
        while not select.select([daemon.fault], [], [], 0)[0]:
            need(time.monotonic() < until and daemon.peer.pump(0.05) is None, "fault_barrier_not_reached")
    raw, controls, flags, _ = daemon.fault.recvmsg(128)
    need(raw == name.encode("ascii") + b"\n" and not controls and not flags, "fault_barrier_record")


def pending_turn(approval):
    reserve, permit = approval.reserves[0]
    return (approval.run, 1, 1, 1, reserve["body"]["input_digest"], permit["permit_id"], reserve["reserve_digest"])


def scenario(root, name, fixture, manifest_sha, lock_sha):
    start = name == FAULT_START
    broker = name == FAULT_BROKER
    approval = Approval(fixture, name)
    script = Conversation(fixture, name, approval.question)
    bearer = secrets.token_hex(32).encode()
    enrollment = {"v": 1, "type": "enrollment", "role": "corral", "link_id": fresh(),
                  "lite_instance": fresh(), "corral_instance": fresh(), "channel_epoch": fresh(),
                  "prior_channel_epoch": None, "profile_sha256": PROFILE_SHA,
                  "corral_manifest_sha256": manifest_sha, "joint_lock_sha256": lock_sha}

    def fail_closed(_):
        raise Teardown("provider_thread_not_joined")

    server = ScriptedHTTP(script, bearer, fail_closed).serve()
    daemon = None
    turns_expected = int(broker)
    action = b"continue\n" if name == FAULT_SANDBOX else b"drop\n"
    terminal = ("cancelled", "stopped") if start else ("failed", "unavailable")
    try:
        daemon = Daemon(root, enrollment, bearer, fixture, approval, script, False, fault_case=name)
        daemon.boot()
        peer = daemon.peer
        if start:
            peer.send("start_investigation", approval.start)
        else:
            admitted = peer.ask("start_investigation", approval.start)
            need(admitted["state"] == "admitted" and admitted["generation"] == 1 and admitted["offer"] is None,
                 "fault_start_reply")
            approval.run = admitted["run_id"]
        barrier(daemon, name, start=start)
        before, turns = inspect(root, approval)
        if start:
            approval.run = before[0]
            need(before[4:] == ("admitted", None, "intent", None), "start_commit_barrier_not_at_intent")
            assert_no_workers(daemon.process.pid)
        else:
            phase = ("working", "attached") if broker else ("admitted", "submitted")
            need(before[0] == approval.run and (before[4], before[6]) == phase and before[5] is None
                 and ident(before[7]), "fault_launch_barrier_phase")
            need(peer.handshakes == {"evidence": 3, "host_poll": 3}
                 and set(peer.channels) == {"evidence", "host_poll"}, "fault_broker_routes_not_live")
        need(approval.binds == approval.attachments == int(not start) and not peer.fences and
             len(approval.reserves) == turns_expected and not approval.settles and
             server.accepted <= turns_expected and not script.digests and server.error is None,
             "fault_before_action_effects")
        want = [pending_turn(approval) + (None, None, None, 0, 0)] if broker else []
        need(turns == want, "fault_before_action_ledger")
        need(daemon.fault.send(action) == len(action), "fault_action_record")
        need(daemon.fault.recvmsg(1)[:3] == (b"", [], 0), "fault_channel_not_consumed")
        if start:
            # Do not close Work ourselves. Any response byte, not just a valid
            # frame, means the committed-Start reply was not actually dropped.
            need(peer.work.recvmsg(1)[:3] == (b"", [], 0), "start_work_not_eof")
            daemon.process.wait(timeout=5)
            need(peer.bootstrap.recvmsg(1)[:3] == (b"", [], 0), "start_bootstrap_not_eof")
        else:
            # Only broker EOF in `pump` removes a data endpoint. Don't close
            # the driver copies or stop the daemon to manufacture this proof.
            until = time.monotonic() + 15
            while peer.channels or peer.fences == 0:
                need(time.monotonic() < until and peer.pump(0.1) is None, "fault_broker_or_fence_not_completed")
            snapshot = peer.ask("read_investigation", {"run_id": approval.run})
            need(keys(snapshot, "run_id", "generation", "seq", "state", "reason", "offer", "acked")
                 and snapshot["run_id"] == approval.run and snapshot["generation"] == 1
                 and (snapshot["state"], snapshot["reason"]) == terminal and snapshot["offer"] is None
                 and snapshot["acked"] is False and peer.fences == 1, "fault_terminal_snapshot")
            need(not select.select([peer.bootstrap], [], [], 0)[0], "fault_bootstrap_not_retained")
            assert_no_workers(daemon.process.pid)
        need(not approval.broker_calls and len(approval.reserves) == len(approval.settles) == turns_expected,
             "fault_work_effects")
        daemon.close(expected=1 if start else 0)
        daemon = None
    finally:
        try:
            if daemon is not None:
                daemon.close()
        finally:
            server.close()
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as conn:
        conn.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        conn.bind(("127.0.0.1", PORT))
    need(server.accepted == server.empty_closes == turns_expected and not script.digests and
         not (SCRATCH / "owner-command-ran").exists(), "fault_provider_or_ambient_effect")
    after, turns = inspect(root, approval)
    need(after == (*before[:4], *terminal, "cleaned", before[7]), "fault_not_durably_cleaned")
    want = [pending_turn(approval) + ("indeterminate", approval.settles[0]["settle_digest"], 1000, 1, 0)] if broker else []
    need(turns == want, "fault_after_action_ledger")
    return {"status": "pass", "barrier": name, "action": action[:-1].decode("ascii"),
            "work_eof": start, "broker_data_eof": not start, "workers_reaped": True,
            "provider_accepts": server.accepted, "provider_empty_closes": server.empty_closes,
            "provider_sends": len(script.digests), "broker_deliveries": 0,
            "reserves": len(approval.reserves), "observed_settlements": len(approval.settles),
            "launch_phase": after[6], "terminal": terminal[1], "charged_microusd": 1000 if broker else 0}
