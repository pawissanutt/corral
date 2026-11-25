"""Offline synthetic turn probe; no Pi, real peer, or provider transport.

The three journals do not share a transaction. A short SQLite transaction
serializes each local decision, and the provider records every call (no
idempotency filter). A reopen creates new peer objects and a new connection
lease. The fixed synthetic clock advances wall and elapsed time together.
"""

import base64
from contextlib import contextmanager
import hashlib
import json
import sqlite3
import unittest


CHECK = unittest.TestCase()
DAY_MS = 86_400_000


def _encode(body):
    # The fixed fixture uses integer JSON and ASCII object keys.
    return json.dumps(body, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def _digest(wire):
    return hashlib.sha256(wire).hexdigest()


def _pinned(wire, signature):
    CHECK.assertEqual(wire, signature["canonical_utf8"].encode())
    CHECK.assertEqual(_digest(wire), signature["sha256"])


class Refused(Exception):
    pass


class ReplyLost(Exception):
    pass


def _require(test, code):
    if not test:
        raise Refused(code)


@contextmanager
def _transaction(path):
    db = sqlite3.connect(path)
    db.row_factory = sqlite3.Row
    try:
        with db:
            db.execute("BEGIN IMMEDIATE")
            yield db
    finally:
        db.close()


class FakeProvider:
    def __init__(self, root, vectors):
        self.path = root / "fake-provider.sqlite"
        self.vectors = vectors
        with _transaction(self.path) as db:
            db.execute("CREATE TABLE IF NOT EXISTS sends (ordinal INTEGER PRIMARY KEY, input_digest TEXT)")

    def send(self, wire, lose_reply=False):
        # Deliberately commit before any injected loss; no uniqueness constraint
        # on the input digest to mask a second actual provider call.
        with _transaction(self.path) as db:
            db.execute("INSERT INTO sends (input_digest) VALUES (?)", (_digest(wire),))
        if lose_reply:
            raise ReplyLost("provider")
        v = self.vectors
        if wire == base64.b64decode(v["model_request_b64"], validate=True):
            calls = [inv["signature"]["body"] for inv in v["invocations"][:3]]
            return 760, [(call["tool"], call["args"]) for call in calls]
        messages = json.loads(wire)["messages"]
        replies = [json.loads(msg["content"]) for msg in messages if msg["role"] == "tool"]
        CHECK.assertIn(v["invocations"][3]["reply"], replies)
        return 410, [("finish_report", v["finish_report_args"])]

    def send_count(self):
        with _transaction(self.path) as db:
            return db.execute("SELECT COUNT(*) FROM sends").fetchone()[0]

    def digests(self):
        with _transaction(self.path) as db:
            return [row[0] for row in db.execute("SELECT input_digest FROM sends ORDER BY ordinal")]


class _Lite:
    """One pinned foreground approval; each issued row debits one turn."""

    def __init__(self, root, binding, deadline, limits):
        self.path = root / "fake-lite.sqlite"
        self.binding = binding
        self.deadline = deadline
        self.limits = limits
        with _transaction(self.path) as db:
            db.execute("CREATE TABLE IF NOT EXISTS binding (run TEXT PRIMARY KEY, gen INTEGER)")
            db.execute("INSERT OR IGNORE INTO binding VALUES (?, ?)", (binding["run_id"], binding["generation"]))
            db.execute("""CREATE TABLE IF NOT EXISTS permits (
                step INTEGER PRIMARY KEY, gen INTEGER, reserve BLOB, digest TEXT, upper INTEGER,
                permit TEXT, expires INTEGER, seal BLOB, charged INTEGER)""")

    def _generation(self, db, run):
        row = db.execute("SELECT gen FROM binding WHERE run=?", (run,)).fetchone()
        _require(row is not None, "not_found")
        return row[0]

    def replace_after_teardown(self, run):
        with _transaction(self.path) as db:
            _require(not db.execute("""SELECT 1 FROM permits WHERE seal IS NULL OR
                json_extract(CAST(seal AS TEXT), '$.outcome') != 'not_sent'""").fetchone(), "unresolved")
            db.execute("UPDATE binding SET gen=gen+1 WHERE run=?", (run,))

    def reserve(self, wire, digest, now):
        body = json.loads(wire)
        step, gen, run = body["step"], body["generation"], body["run_id"]
        with _transaction(self.path) as db:
            _require(wire == _encode(body) and digest == _digest(wire), "invalid")
            _require(gen == self._generation(db, run), "stale_generation")
            _require(all(body[k] == self.binding[k] for k in ("request_digest", "policy_ref")), "invalid")
            row = db.execute("SELECT * FROM permits WHERE step=?", (step,)).fetchone()
            if row:
                _require(row["reserve"] == wire and row["digest"] == digest, "conflict")
                _require(now < row["expires"], "expired")
                _require(row["seal"] is None and row["charged"] is None, "resolved")
                return {"permit_id": row["permit"], "expires_ms": row["expires"], "reserve_digest": digest}
            _require(now < self.deadline, "expired")
            prior = db.execute("SELECT * FROM permits ORDER BY step DESC LIMIT 1").fetchone()
            _require(not prior or (prior["seal"] is not None and
                                    json.loads(prior["seal"])["outcome"] != "indeterminate"), "unresolved")
            _require(step == (prior["step"] + 1 if prior else 1), "invalid")
            debit, reserved = db.execute("SELECT COUNT(*), COALESCE(SUM(COALESCE(charged, upper)), 0) FROM permits").fetchone()
            upper = body["max_cost_microusd"]
            _require(debit < self.limits["turns"] and reserved + upper <= self.limits["run_cost_microusd"],
                     "lite_budget_exceeded")
            permit = f"permit_{run.removeprefix('run_')}{step}"
            expires = min(now + 5000, self.deadline)
            db.execute("INSERT INTO permits VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
                       (step, gen, wire, digest, upper, permit, expires))
            return {"permit_id": permit, "expires_ms": expires, "reserve_digest": digest}

    def settle(self, wire, digest):
        body = json.loads(wire)
        with _transaction(self.path) as db:
            _require(wire == _encode(body) and digest == _digest(wire), "invalid")
            self._generation(db, body["run_id"])  # Enrolled run; settlement may name an old generation.
            row = db.execute("SELECT * FROM permits WHERE step=?", (body["step"],)).fetchone()
            _require(row and (row["gen"], row["permit"], row["digest"]) ==
                     (body["generation"], body["permit_id"], body["reserve_digest"]), "conflict")
            charge, outcome = body["charged_microusd"], body["outcome"]
            _require((outcome == "not_sent" and charge == 0) or
                     (outcome == "completed" and 0 <= charge <= row["upper"]) or
                     (outcome == "indeterminate" and charge == row["upper"]), "invalid")
            if row["seal"] is not None:
                _require(row["seal"] == wire, "conflict")
            else:
                # A late authenticated payload is audit only after worst-case default.
                db.execute("UPDATE permits SET seal=?, charged=COALESCE(charged, ?) WHERE step=?",
                           (wire, charge, body["step"]))
            return {"settle_digest": digest, "recorded": True}

    def default(self, now):
        if now >= self.deadline + 300_000:
            with _transaction(self.path) as db:
                db.execute("UPDATE permits SET charged=upper WHERE charged IS NULL")

    def totals(self):
        with _transaction(self.path) as db:
            return dict(db.execute("""SELECT COUNT(*) AS debits, COUNT(seal) AS settlements,
                COALESCE(SUM(CASE WHEN charged IS NULL THEN upper ELSE 0 END), 0) AS held,
                COALESCE(SUM(charged), 0) AS charged FROM permits""").fetchone())


class _Corral:
    def __init__(self, root, costs):
        self.path = root / "fake-corral-spend.sqlite"
        self.costs = costs
        with _transaction(self.path) as db:
            db.execute("""CREATE TABLE IF NOT EXISTS runs (
                run TEXT PRIMARY KEY, gen INTEGER, epoch INTEGER, live INTEGER, fenced INTEGER)""")
            db.execute("""CREATE TABLE IF NOT EXISTS attempts (
                run TEXT, step INTEGER, gen INTEGER, epoch INTEGER, reserve BLOB, digest TEXT,
                input_digest TEXT, upper INTEGER, day INTEGER, state TEXT, permit TEXT, expires INTEGER,
                seal BLOB, charged INTEGER, ack INTEGER DEFAULT 0, PRIMARY KEY (run, step))""")

    def connect(self, run):
        with _transaction(self.path) as db:
            db.execute("INSERT OR IGNORE INTO runs VALUES (?, 1, 0, 0, 0)", (run,))
            db.execute("UPDATE runs SET epoch=epoch+1, live=1 WHERE run=?", (run,))

    def disconnect(self, run):
        with _transaction(self.path) as db:
            db.execute("UPDATE runs SET live=0 WHERE run=?", (run,))

    def replace_after_teardown(self, run):
        with _transaction(self.path) as db:
            _require(not db.execute("""SELECT 1 FROM attempts WHERE run=? AND (state != 'settled' OR ack=0 OR
                json_extract(CAST(seal AS TEXT), '$.outcome') != 'not_sent')""", (run,)).fetchone(),
                "unresolved")
            db.execute("UPDATE runs SET gen=gen+1, epoch=epoch+1 WHERE run=?", (run,))

    @staticmethod
    def _active(db, run, gen):
        current = db.execute("SELECT * FROM runs WHERE run=?", (run,)).fetchone()
        _require(current is not None, "not_found")
        _require(current["gen"] == gen, "stale_generation")
        _require(current["live"], "disconnected")
        _require(not current["fenced"], "stopped")
        return current["epoch"]

    @staticmethod
    def _row(db, run, step):
        return db.execute("SELECT * FROM attempts WHERE run=? AND step=?", (run, step)).fetchone()

    def hold(self, wire, now):
        body = json.loads(wire)
        run, step, gen, upper = (body[k] for k in ("run_id", "step", "generation", "max_cost_microusd"))
        day = now // DAY_MS
        with _transaction(self.path) as db:
            epoch = self._active(db, run, gen)
            row = self._row(db, run, step)
            if row:
                _require(row["reserve"] == wire, "conflict")
                _require(row["state"] == "held", "not_held")
                _require(row["epoch"] == epoch, "lease_changed")
                return
            prior = db.execute("SELECT * FROM attempts WHERE run=? ORDER BY step DESC LIMIT 1", (run,)).fetchone()
            _require(not prior or (prior["state"] == "settled" and prior["ack"]), "unresolved")
            _require(step == (prior["step"] + 1 if prior else 1), "invalid")
            spent_run = db.execute("SELECT COALESCE(SUM(COALESCE(charged, upper)), 0) FROM attempts WHERE run=?",
                                   (run,)).fetchone()[0]
            spent_day = db.execute("""SELECT COALESCE(SUM(COALESCE(charged, upper)), 0)
                FROM attempts WHERE day=? OR charged IS NULL""", (day,)).fetchone()[0]
            _require(spent_run + upper <= self.costs["max_run_microusd"], "run_budget_exceeded")
            _require(spent_day + upper <= self.costs["max_utc_day_microusd"], "day_budget_exceeded")
            db.execute("""INSERT INTO attempts (run, step, gen, epoch, reserve, digest, input_digest, upper, day, state)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'held')""",
                       (run, step, gen, epoch, wire, _digest(wire), body["input_digest"], upper, day))

    def attach(self, body, permit):
        run, step = body["run_id"], body["step"]
        with _transaction(self.path) as db:
            epoch = self._active(db, run, body["generation"])
            row = self._row(db, run, step)
            _require(row and row["state"] == "held" and row["epoch"] == epoch, "not_held")
            _require(row["digest"] == permit["reserve_digest"] and
                     (row["permit"] is None or
                      (row["permit"], row["expires"]) == (permit["permit_id"], permit["expires_ms"])), "conflict")
            db.execute("UPDATE attempts SET permit=?, expires=? WHERE run=? AND step=?",
                       (permit["permit_id"], permit["expires_ms"], run, step))

    def deny(self, run, step):
        # A definitive Lite budget refusal is not a lost (possibly committed) reply.
        with _transaction(self.path) as db:
            result = db.execute("""UPDATE attempts SET state='denied', charged=0
                WHERE run=? AND step=? AND state='held' AND permit IS NULL""", (run, step))
            _require(result.rowcount == 1, "not_held")
            db.execute("UPDATE runs SET fenced=1 WHERE run=?", (run,))

    def consume(self, body, model_wire, now):
        run, step = body["run_id"], body["step"]
        with _transaction(self.path) as db:
            epoch = self._active(db, run, body["generation"])
            row = self._row(db, run, step)
            _require(row and row["state"] == "held" and row["permit"], "not_held")
            _require(row["epoch"] == epoch, "lease_changed")
            _require(now < row["expires"], "expired")
            _require(row["reserve"] == _encode(body) and row["input_digest"] == _digest(model_wire), "conflict")
            result = db.execute("""UPDATE attempts SET state='consumed'
                WHERE run=? AND step=? AND state='held'""", (run, step))
            _require(result.rowcount == 1, "not_held")
        # The caller can attempt one send only after the transaction commits.

    def seal(self, run, step, usage):
        with _transaction(self.path) as db:
            row = self._row(db, run, step)
            _require(row and row["permit"], "unknown_permit")
            if row["seal"] is not None:
                return row["seal"]
            if row["state"] == "held":
                outcome, charge = "not_sent", 0
            else:
                _require(row["state"] == "consumed", "invalid")
                if usage is None:
                    outcome, charge = "indeterminate", row["upper"]
                    db.execute("UPDATE runs SET fenced=1 WHERE run=?", (run,))
                else:
                    _require(0 <= usage <= row["upper"], "overbound")
                    outcome, charge = "completed", usage
            wire = _encode({"v": 1, "kind": "settle_turn", "run_id": run, "generation": row["gen"],
                            "step": step, "permit_id": row["permit"], "reserve_digest": row["digest"],
                            "outcome": outcome, "charged_microusd": charge})
            db.execute("UPDATE attempts SET seal=?, state='settled' WHERE run=? AND step=?", (wire, run, step))
            return wire

    def ack(self, run, step, receipt):
        with _transaction(self.path) as db:
            row = self._row(db, run, step)
            _require(receipt == {"settle_digest": _digest(row["seal"]), "recorded": True}, "conflict")
            charge = json.loads(row["seal"])["charged_microusd"]
            db.execute("UPDATE attempts SET ack=1, charged=COALESCE(charged, ?) WHERE run=? AND step=?",
                       (charge, run, step))

    def default(self, run, now, deadline):
        if now >= deadline + 300_000:
            with _transaction(self.path) as db:
                db.execute("UPDATE runs SET fenced=1 WHERE run=?", (run,))
                db.execute("UPDATE attempts SET state='defaulted', charged=upper WHERE run=? AND charged IS NULL", (run,))

    def totals(self):
        with _transaction(self.path) as db:
            return dict(db.execute("""SELECT COUNT(*) AS attempts, COUNT(permit) AS known_permits,
                COUNT(seal) AS settlements, COALESCE(SUM(ack), 0) AS acks,
                COALESCE(SUM(CASE WHEN charged IS NULL THEN upper ELSE 0 END), 0) AS held,
                COALESCE(SUM(charged), 0) AS charged FROM attempts""").fetchone())


class _Round:
    def __init__(self, root, vectors, now=None, costs=None, limits=None):
        self.v = vectors
        self.binding = vectors["bind"]["params"]
        self.run = self.binding["run_id"]
        self.deadline = vectors["request"]["body"]["deadline_ms"]
        self.now = vectors["clock_ms"] if now is None else now
        self.costs = costs if costs is not None else vectors["link_cost"]
        self.limits = limits if limits is not None else vectors["request"]["body"]["limits"]
        self.root = root
        self.model = base64.b64decode(vectors["model_request_b64"], validate=True)
        self.reopen()

    def reopen(self):
        self.local = _Corral(self.root, self.costs)
        self.lite = _Lite(self.root, self.binding, self.deadline, self.limits)
        self.provider = FakeProvider(self.root, self.v)
        self.local.connect(self.run)
        self.usage = {}  # A restart cannot reconstruct missing trusted model usage.

    def body(self, step=1, generation=1, run=None, model=None, upper=None):
        return {"v": 1, "kind": "reserve_turn", "run_id": run or self.run, "generation": generation,
                "step": step, "request_digest": self.binding["request_digest"],
                "policy_ref": self.binding["policy_ref"],
                "input_digest": _digest(self.model if model is None else model),
                "max_cost_microusd": 1000 if upper is None else upper}

    def reserve(self, body, lose_reply=False):
        wire = _encode(body)
        self.local.hold(wire, self.now)
        try:
            permit = self.lite.reserve(wire, _digest(wire), self.now)
        except Refused as error:
            if str(error) == "lite_budget_exceeded":
                self.local.deny(body["run_id"], body["step"])
            raise
        if lose_reply:
            raise ReplyLost("reserve")
        self.local.attach(body, permit)
        return permit

    def dispatch(self, body, model=None, lose_reply=False):
        model = self.model if model is None else model
        self.local.consume(body, model, self.now)
        usage, output = self.provider.send(model, lose_reply)
        self.usage[body["step"]] = usage
        return output

    def settle(self, step=1, pinned=None, lose_reply=False):
        wire = self.local.seal(self.run, step, self.usage.get(step))
        if pinned is not None:
            _pinned(wire, self.v["settlement"][pinned])
        receipt = self.lite.settle(wire, _digest(wire))
        if lose_reply:
            raise ReplyLost("settlement")
        self.local.ack(self.run, step, receipt)
        return receipt

    def observed(self):
        return {"corral": self.local.totals(), "lite": self.lite.totals(), "provider_sends": self.provider.send_count()}


def _first_turn(turn):
    body = turn.body()
    v = turn.v
    _pinned(_encode(body), v["reserve"]["signature"])
    CHECK.assertEqual(turn.reserve(body), v["reserve"]["result"])
    output = turn.dispatch(body)
    CHECK.assertEqual([name for name, _ in output], ["search_materials", "read_evidence", "calculate_metric"])
    CHECK.assertEqual(turn.provider.digests(), [v["reserve"]["signature"]["body"]["input_digest"]])
    receipt = turn.settle(pinned="completed")
    CHECK.assertEqual(receipt, {"settle_digest": v["settlement"]["completed"]["sha256"], "recorded": True})
    _check_charges(turn, actual=760, sends=1)
    return output


def complete_fixture_turn(root, vectors):
    """Run the pinned first paid turn, returning the journal and scripted evidence calls."""
    turn = _Round(root, vectors)
    return turn.provider, _first_turn(turn)


def finish_fixture_turn(root, vectors, accepted_replies):
    """Run a new paid step with the actual structured evidence in its finalized body."""
    turn = _Round(root, vectors)
    request = json.loads(turn.model)
    request["messages"].extend({"role": "tool", "content": _encode(reply).decode()} for reply in accepted_replies)
    model = _encode(request)
    body = turn.body(step=2, model=model)
    before = turn.provider.digests()
    turn.reserve(body)
    output = turn.dispatch(body, model=model)
    turn.settle(step=2)
    CHECK.assertEqual(turn.provider.digests(), before + [_digest(model)])
    CHECK.assertEqual([name for name, _ in output], ["finish_report"])
    return output[0][1]


def _check_charges(turn, actual, sends):
    CHECK.assertEqual(turn.observed(), {
        "corral": {"attempts": 1, "known_permits": 1, "settlements": 1, "acks": 1, "held": 0, "charged": actual},
        "lite": {"debits": 1, "settlements": 1, "held": 0, "charged": actual},
        "provider_sends": sends,
    })


def turn_cases(root, vectors):
    """Deterministic, independent turn cases; return measurements, not expectations."""
    results = {}

    def case(name, **kwargs):
        path = root / name
        path.mkdir()
        return _Round(path, vectors, **kwargs)

    v = vectors
    cost = v["link_cost"]
    turn = case("link_cost", costs={**cost, "max_run_microusd": 1759})
    _first_turn(turn)
    with CHECK.assertRaisesRegex(Refused, "^run_budget_exceeded$") as run_denial:
        turn.reserve(turn.body(step=2))
    _check_charges(turn, actual=760, sends=1)
    run_cap = turn.observed()
    # Independent subcase: pre-permit local holds in two runs, then a third run.
    # Concurrency slot admission is outside this spend probe.
    midnight = v["clock_ms"] // DAY_MS * DAY_MS
    turn = case("day_cost", now=midnight - 1)
    for run in ("run_A", "run_B"):
        if run != turn.run:
            turn.local.connect(run)
        turn.local.hold(_encode(turn.body(run=run, upper=2100)), turn.now)
    turn.local.connect("run_C")
    candidate = _encode(turn.body(run="run_C"))
    with CHECK.assertRaisesRegex(Refused, "^day_budget_exceeded$") as day_denial:
        turn.local.hold(candidate, turn.now)
    turn.now = midnight
    turn.reopen()
    turn.local.connect("run_C")
    with CHECK.assertRaisesRegex(Refused, "^day_budget_exceeded$") as rollover_denial:
        turn.local.hold(candidate, turn.now)
    CHECK.assertEqual(turn.local.totals(), {"attempts": 2, "known_permits": 0, "settlements": 0,
                                           "acks": 0, "held": 4200, "charged": 0})
    CHECK.assertEqual(turn.provider.send_count(), 0)
    results["link_cost"] = {"run_denial": str(run_denial.exception), "run_cap": run_cap,
                            "day_denial": str(day_denial.exception), "rollover_denial": str(rollover_denial.exception),
                            "day_cap": turn.observed()}

    limits = v["request"]["body"]["limits"]
    turn = case("lite_denial", limits={**limits, "run_cost_microusd": 999})
    with CHECK.assertRaisesRegex(Refused, "^lite_budget_exceeded$") as lite_denial:
        turn.reserve(turn.body())
    CHECK.assertEqual(turn.observed(), {"corral": {"attempts": 1, "known_permits": 0, "settlements": 0,
                                                   "acks": 0, "held": 0, "charged": 0},
                                       "lite": {"debits": 0, "settlements": 0, "held": 0, "charged": 0},
                                       "provider_sends": 0})
    results["lite_denial"] = {"denial": str(lite_denial.exception), **turn.observed()}

    turn = case("lost_reserve")
    body = turn.body()
    _pinned(_encode(body), v["reserve"]["signature"])
    with CHECK.assertRaisesRegex(ReplyLost, "^reserve$"):
        turn.reserve(body, lose_reply=True)
    CHECK.assertEqual(turn.local.totals()["known_permits"], 0)
    turn.now += 1  # A replay must not create a fresh TTL.
    permit = turn.reserve(body)
    CHECK.assertEqual(permit, v["reserve"]["result"])
    changed = _encode({**body, "input_digest": _digest(turn.model + b" ")})
    with CHECK.assertRaisesRegex(Refused, "^conflict$") as changed_denial:
        turn.lite.reserve(changed, _digest(changed), turn.now)
    CHECK.assertEqual(turn.observed(), {"corral": {"attempts": 1, "known_permits": 1, "settlements": 0,
                                                   "acks": 0, "held": 1000, "charged": 0},
                                       "lite": {"debits": 1, "settlements": 0, "held": 1000, "charged": 0},
                                       "provider_sends": 0})
    results["lost_reserve"] = {"permit": permit, "changed_denial": str(changed_denial.exception), **turn.observed()}

    turn = case("reserve_reply_never_recovered")
    with CHECK.assertRaisesRegex(ReplyLost, "^reserve$"):
        turn.reserve(turn.body(), lose_reply=True)
    turn.now = v["reserve"]["result"]["expires_ms"]
    with CHECK.assertRaisesRegex(Refused, "^expired$"):
        turn.reserve(turn.body())
    with CHECK.assertRaisesRegex(Refused, "^unresolved$") as unresolved:
        turn.reserve(turn.body(step=2))
    turn.now = turn.deadline + 299_999
    turn.local.default(turn.run, turn.now, turn.deadline)
    turn.lite.default(turn.now)
    CHECK.assertEqual((turn.local.totals()["held"], turn.lite.totals()["held"]), (1000, 1000))
    turn.now += 1
    turn.local.default(turn.run, turn.now, turn.deadline)
    turn.lite.default(turn.now)
    CHECK.assertEqual(turn.observed(), {"corral": {"attempts": 1, "known_permits": 0, "settlements": 0,
                                                   "acks": 0, "held": 0, "charged": 1000},
                                       "lite": {"debits": 1, "settlements": 0, "held": 0, "charged": 1000},
                                       "provider_sends": 0})
    results["reserve_reply_never_recovered"] = {"next_denial": str(unresolved.exception), **turn.observed()}

    for name, code in (("stale_generation", "stale_generation"), ("permit_fenced", "expired"),
                       ("permit_reconnect", "lease_changed"), ("changed_model_body", "conflict")):
        turn = case(name)
        body = turn.body()
        permit = turn.reserve(body)
        model = turn.model
        if name == "stale_generation":
            turn.settle(pinned="not_sent")
            turn.local.replace_after_teardown(turn.run)
            turn.lite.replace_after_teardown(turn.run)
        elif name == "permit_fenced":
            turn.now = permit["expires_ms"]
        elif name == "permit_reconnect":
            turn.local.disconnect(turn.run)
            with CHECK.assertRaisesRegex(Refused, "^disconnected$"):
                turn.dispatch(body)
            turn.reopen()
            # Lite can return the old receipt for bookkeeping, not a renewed lease.
            wire = _encode(body)
            CHECK.assertEqual(turn.lite.reserve(wire, _digest(wire), turn.now), permit)
        else:
            model += b" "  # Semantically equal JSON cannot substitute for the reserved bytes.
        with CHECK.assertRaisesRegex(Refused, f"^{code}$") as fence:
            turn.dispatch(body, model)
        if name != "changed_model_body":
            with CHECK.assertRaisesRegex(Refused, f"^{code}$"):
                turn.reserve(body)
        CHECK.assertEqual(turn.provider.send_count(), 0)
        results[name] = {"send_denial": str(fence.exception), **turn.observed()}

    turn = case("ambiguous_provider")
    body = turn.body()
    _pinned(_encode(body), v["reserve"]["signature"])
    CHECK.assertEqual(turn.reserve(body), v["reserve"]["result"])
    with CHECK.assertRaisesRegex(ReplyLost, "^provider$"):
        turn.dispatch(body, lose_reply=True)
    CHECK.assertEqual(turn.provider.send_count(), 1)
    after_loss = turn.observed()
    turn.reopen()
    CHECK.assertEqual(turn.observed(), after_loss)
    with CHECK.assertRaisesRegex(Refused, "^not_held$") as replay:
        turn.reserve(body)
    with CHECK.assertRaisesRegex(Refused, "^not_held$"):
        turn.dispatch(body)
    with CHECK.assertRaisesRegex(Refused, "^unresolved$"):
        turn.reserve(turn.body(step=2))
    with CHECK.assertRaisesRegex(ReplyLost, "^settlement$"):
        turn.settle(pinned="indeterminate", lose_reply=True)
    CHECK.assertEqual(turn.local.totals()["acks"], 0)
    CHECK.assertEqual(turn.local.totals()["held"], 1000)
    CHECK.assertEqual(turn.lite.totals()["settlements"], 1)
    turn.reopen()
    receipt = turn.settle(pinned="indeterminate")
    CHECK.assertEqual(turn.settle(pinned="indeterminate"), receipt)
    changed_settlement = v["settlement"]["completed"]
    with CHECK.assertRaisesRegex(Refused, "^conflict$"):
        turn.lite.settle(changed_settlement["canonical_utf8"].encode(), changed_settlement["sha256"])
    with CHECK.assertRaisesRegex(Refused, "^stopped$") as terminal:
        turn.reserve(turn.body(step=2))
    _check_charges(turn, actual=1000, sends=1)
    CHECK.assertEqual(turn.provider.digests(), [body["input_digest"]])
    results["ambiguous_provider"] = {"replay_denial": str(replay.exception), "next_denial": str(terminal.exception),
                                     "settlement": receipt, **turn.observed()}

    turn = case("completed_turn")
    output = _first_turn(turn)
    results["completed_turn"] = {"tools": [name for name, _ in output], **turn.observed()}
    return results
