#!/usr/bin/env python3
"""Run T-2071 offline; stdout is a deterministic pass/fail JSON receipt."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from threading import Event
import traceback

from evidence import EvidenceHost, EvidencePeer
from session import Refused, Session, digest, wire


HERE = Path(__file__).resolve()
BUNDLE = HERE.parents[3] / "docs" / "contracts" / "lite-analyst-v1"
BUNDLE_ID = "9a0376354d43efe1b72ffef0be9ee333cedf95cd50890a096f9c891c0b4607a5"
FAULTS = ("before_blob", "after_blob", "before_row_commit", "after_row_commit")


def equal(observed, expected):
    if observed != expected:
        raise AssertionError(f"observed {observed!r}, expected {expected!r}")
    return observed


def denied(code, fn, *args, **kw):
    try:
        fn(*args, **kw)
    except Refused as exc:
        return equal(str(exc), code)
    raise AssertionError(f"expected {code}, operation succeeded: {fn.__name__}")


def fixtures():
    sums = (BUNDLE / "SHA256SUMS").read_bytes()
    equal(digest(sums), BUNDLE_ID)
    for line in sums.decode().splitlines():
        sha, name = line.split("  ")
        equal(digest((BUNDLE / name).read_bytes()), sha)
    v = json.loads((BUNDLE / "vectors.json").read_bytes())
    policy = json.loads((BUNDLE / "tool-policy.json").read_bytes())
    equal(digest(wire(policy)), v["policy"]["body"]["tool_policy_sha256"])
    return v, policy


def start(root, v):
    session = Session(root, v)
    link = v["link_id"]
    initial = session.start(link, v["start"]["params"])
    equal(initial, v["start"]["result"])
    binding = dict(link=link, run_id=initial["run_id"], generation=initial["generation"])
    session.bind_launch(link, binding["run_id"])
    return session, binding


def start_case(root, v):
    session, binding = start(root, v)
    # Discard the initial response, reopen storage, and retry after the deadline.
    session = Session(root, v)
    session.now = v["request"]["body"]["deadline_ms"] + 1
    link, run = binding["link"], binding["run_id"]
    recovered = equal(session.start(link, v["start"]["params"]), v["snapshots"]["before_finish"])
    conflict = denied("conflict", session.start, link, v["start"]["conflict_params"])
    denied("stopped", session.bind_launch, link, run)
    with session.transaction() as db:
        runs, slots, claims = db.execute("SELECT count(*),sum(slot),sum(launch='bound') FROM runs").fetchone()
    equal((runs, slots, claims), (1, 1, 1))
    return dict(snapshot=recovered, changed_request=conflict, runs=runs, slots=slots,
                launch_claims=claims, non_repo_scratch=True)


def drive_evidence(root, v, policy):
    from turns import complete_fixture_turn, finish_fixture_turn

    session, binding = start(root, v)
    peer = EvidencePeer(root, binding, v)
    host = EvidenceHost(session, peer, binding, policy)
    provider, actions = complete_fixture_turn(root, v)
    calls = v["invocations"]
    equal(len(actions), 3)
    for (tool, args), call in zip(actions, calls[:3], strict=True):
        equal((tool, args), (call["signature"]["body"]["tool"], call["signature"]["body"]["args"]))
        equal(host.model_call(call["invocation_id"], tool, args), call["reply"])
    terminal = calls[3]
    model_poll = denied("invalid", host.model_call, "model_poll", "read_result", {"result": host.pending})
    before = provider.send_count()
    equal(host.poll("poll_wait"), calls[2]["reply"])
    peer.ready = True
    equal(host.poll(terminal["invocation_id"], lose_reply=True), None)
    equal(len(host.accepted), 4)
    denied("invalid", session.finish, binding, v["finish_report_args"])
    before_replay = peer.disclosed
    equal(host.poll(terminal["invocation_id"]), terminal["reply"])
    replay_bytes = equal(peer.disclosed - before_replay, len(wire(terminal["reply"])))
    changed = dict(terminal["signature"]["body"], args={"result": "wrong"})
    denied("conflict", peer.call, binding, terminal["invocation_id"], digest(wire(changed)), changed)
    after = provider.send_count()
    equal((before, after, peer.admissions, peer.polls, peer.releases), (1, 1, 1, 2, 6))
    args = finish_fixture_turn(root, v, host.accepted)
    total_sends = equal(provider.send_count(), 2)
    equal(args, v["finish_report_args"])
    trace = dict(provider_sends=total_sends, polls_paid_sends=after-before, model_poll=model_poll,
                 peer_polls=peer.polls, compute_admissions=peer.admissions, releases=peer.releases,
                 replay_bytes=replay_bytes, host_accepted=len(host.accepted))
    return session, binding, peer, host.accepted, args, trace


def record_report(root, v, policy):
    session, binding, peer, replies, args, trace = drive_evidence(root, v, policy)
    link, run = binding["link"], binding["run_id"]
    denied("invalid", session.finish, binding, dict(args, path="not-a-tool-argument"))
    denied("invalid", session.finish, binding,
           dict(args, citations=[dict(args["citations"][0], handle="cite_missing")]))
    denied("invalid", session.finish, binding,
           dict(text="é1", citations=[dict(start_byte=1, end_byte=3, handle="cite_feb_net")]))
    denied("stale_generation", session.finish, dict(binding, generation=0), args)
    equal(session.read(link, run), v["snapshots"]["before_finish"])
    equal(list(session.blobs.iterdir()), [])

    equal(session.finish(binding, args), v["finish_report_result"])
    equal(session.read(link, run), v["snapshots"]["after_finish"])
    with session.transaction() as db:
        row = session.owned(db, link, run)
        ledger = json.loads(row["sealed_ledger"])
        equal(ledger, session.ledger(db, row))
    invocations = v["invocations"]
    source = [*invocations[:3], dict(invocations[3], invocation_id="poll_wait"), invocations[3]]
    equal(ledger, [dict(invocation=entry["invocation_id"], tool=entry["signature"]["body"]["tool"],
                        args_sha=entry["signature"]["sha256"], response_sha=digest(wire(reply)),
                        citation=reply.get("citation"))
                   for entry, reply in zip(source, replies, strict=True)])

    fetch, ack = v["fetch"]["params"], v["ack"]["params"]
    denied("not_found", session.read, "link_B", run)
    denied("not_found", session.cancel, "link_B", run, binding["generation"])
    denied("not_found", session.fetch, "link_B", **fetch)
    denied("not_found", session.ack, "link_B", **ack)
    equal(session.read(link, run), v["snapshots"]["after_finish"])
    result = equal(session.fetch(link, **fetch), v["fetch"]["result"])
    equal(session.fetch(link, **fetch), result)
    count, raw = peer.accept_delivery(result)
    equal(count, 1)
    equal(peer.accept_delivery(result), (1, raw))
    equal(raw, v["report"]["canonical_utf8"].encode("utf-8"))
    denied("conflict", session.ack, link, **dict(ack, bytes=0))
    for _ in range(2):
        equal(session.ack(link, **ack), v["ack"]["result"])
        equal(session.read(link, run), v["snapshots"]["after_ack"])
    equal(session.fetch(link, **fetch), result)
    equal(len(list(session.blobs.iterdir())), 1)

    terminal = invocations[3]
    before = (peer.releases, peer.disclosed, dict(peer.calls))
    peer.revoked = True
    stale = denied("stale_scope", peer.call, binding, terminal["invocation_id"],
                   terminal["signature"]["sha256"], terminal["signature"]["body"])
    equal((peer.releases, peer.disclosed, peer.calls), before)
    with session.transaction() as db:
        equal(session.ledger(db, session.owned(db, link, run)), ledger)
    trace.update(offer=result["offer"], invocation_ledger=ledger, durable_copies=count,
                 ack_seq_delta=1, cross_link_denials=4, rejected_finish_inputs=5,
                 revoked_cached_reply=stale)
    return trace, raw


def race_case(root, v, policy, finish_wins):
    session, b, _, _, args, _ = drive_evidence(root, v, policy)
    at_boundary, release = Event(), Event()
    boundary = "after_row_commit" if finish_wins else "after_blob"

    def barrier(phase):
        if phase == boundary:
            at_boundary.set()
            equal(release.wait(10), True)

    with ThreadPoolExecutor(max_workers=1) as pool:
        worker = pool.submit(session.finish, b, args, barrier)
        try:
            equal(at_boundary.wait(10), True)
            equal(worker.done(), False)
            stop = session.cancel(b["link"], b["run_id"], b["generation"])
            equal(stop["state"], "finished" if finish_wins else "stop_requested")
            with session.transaction() as db:
                equal(session.owned(db, b["link"], b["run_id"])["slot"], 1)
        finally:
            release.set()
        if finish_wins:
            equal(worker.result(10), v["finish_report_result"])
            equal(stop["offer"], v["finish_report_result"]["offer"])
        else:
            denied("stopped", worker.result, 10)
            denied("not_found", session.fetch, b["link"], **v["fetch"]["params"])
            session.confirmed_stop(b["link"], b["run_id"])
            equal(session.read(b["link"], b["run_id"])["state"], "cancelled")
    equal(len(list(session.blobs.iterdir())), 1)
    final = session.read(b["link"], b["run_id"])
    return dict(boundary=boundary, cancel_return=stop["state"], final_state=final["state"],
                published_offers=int(final["offer"] is not None), blobs=1)


def crash_child(root, v, phase):
    if not (root / "session.sqlite").is_file() or not (root / "blobs").is_dir():
        raise ValueError("fault child requires an initialized synthetic fixture")
    args = json.load(sys.stdin)
    b = v["bind"]["params"]
    binding = dict(link=v["link_id"], run_id=b["run_id"], generation=b["generation"])

    def fault(point):
        if point == phase:
            os._exit(81 + FAULTS.index(point))

    Session(root, v).finish(binding, args, fault)
    raise AssertionError("fault not reached")


def crash_case(root, v, policy, phase):
    session, b, _, _, args, _ = drive_evidence(root, v, policy)
    with session.transaction() as db:
        expected_ledger = session.ledger(db, session.owned(db, b["link"], b["run_id"]))
    process = subprocess.run([sys.executable, "-EsB", str(HERE), "--crash-child", phase],
                             cwd=root, env={}, input=wire(args), timeout=10, check=False)
    code = equal(process.returncode, 81 + FAULTS.index(phase))
    session = Session(root, v)
    committed = phase == "after_row_commit"
    snapshots = v["snapshots"]
    expected = snapshots["after_finish" if committed else "before_finish"]
    equal(session.read(b["link"], b["run_id"]), expected)
    with session.transaction() as db:
        ledger = session.owned(db, b["link"], b["run_id"])["sealed_ledger"]
        equal(None if ledger is None else json.loads(ledger), expected_ledger if committed else None)
    blobs = equal(len(list(session.blobs.iterdir())), int(phase != "before_blob"))
    if committed:
        equal(session.fetch(b["link"], **v["fetch"]["params"]), v["fetch"]["result"])
        session.now = v["request"]["body"]["deadline_ms"] + 1
        equal(session.finish(b, args), v["finish_report_result"])
        denied("conflict", session.finish, b, dict(args, text=args["text"] + " More."))
        equal(session.cancel(b["link"], b["run_id"], b["generation"]), expected)
        equal(len(list(session.blobs.iterdir())), 1)
    else:
        denied("not_found", session.fetch, b["link"], **v["fetch"]["params"])
    return dict(exit_code=code, recovered_state=expected["state"], blobs=blobs,
                offer_visible=committed, sealed_ledger_visible=committed)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--report-out", type=Path, help="write exact fetched bytes to a new file")
    ap.add_argument("--crash-child", choices=FAULTS, help=argparse.SUPPRESS)
    opts = ap.parse_args()
    if opts.crash_child:
        crash_child(Path.cwd(), fixtures()[0], opts.crash_child)
        return 1
    receipt = dict(task="T-2071", baseline="T-2069", bundle="sha256:" + BUNDLE_ID,
                   baseline_commit="efba191b76e65fbfd04becf5e39707eb9a5a48a3",
                   status="fail", cases={})
    case = "verify_bundle"
    try:
        v, policy = fixtures()
        from turns import turn_cases

        # Explicit system scratch, not the caller's repo or TMPDIR override.
        with tempfile.TemporaryDirectory(prefix="t2071-", dir="/tmp") as work:
            def folder(name):
                root = Path(work).resolve() / name
                root.mkdir(mode=0o700)
                return root

            cases = receipt["cases"]
            case = "lost_start"
            cases[case] = start_case(folder(case), v)
            case = "turn_cases"
            cases[case] = turn_cases(folder(case), v)
            case = "report_delivery"
            cases[case], raw = record_report(folder(case), v, policy)
            for case, wins in (("cancel_wins", False), ("finish_wins", True)):
                cases[case] = race_case(folder(case), v, policy, wins)
            for phase in FAULTS:
                case = "crash_" + phase
                cases[case] = crash_case(folder(case), v, policy, phase)
        if opts.report_out:
            case = "report_output"
            with opts.report_out.open("xb") as dest:
                dest.write(raw)
        receipt["status"] = "pass"
    except Exception as exc:
        receipt["failed_case"] = case
        receipt["error"] = type(exc).__name__
        traceback.print_exc()
    print(json.dumps(receipt, ensure_ascii=False, indent=2, sort_keys=True))
    return int(receipt["status"] != "pass")


if __name__ == "__main__":
    sys.exit(main())
