"""Disposable T-2071 lifecycle/report model; not a Corral API or storage format."""

import base64
import binascii
from contextlib import closing, contextmanager
import hashlib
import json
import os
import re
import sqlite3
import tempfile


class Refused(Exception):
    pass


def wire(value):
    # JCS for this fixture subset: ASCII object keys, safe integers, no floats.
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"),
                      allow_nan=False).encode("utf-8")


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def b64(raw):
    return base64.b64encode(raw).decode("ascii")


class Session:
    """Single fixture run, with lifecycle and final report on the same SQLite row."""

    def __init__(self, root, vectors):
        self.root = root
        self.v = vectors
        self.now = vectors["clock_ms"]
        self.blobs = root / "blobs"
        self.blobs.mkdir(exist_ok=True)
        with self.transaction() as db:
            db.execute("""CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY, link TEXT NOT NULL, request TEXT NOT NULL, digest TEXT NOT NULL,
                deadline INTEGER NOT NULL, generation INTEGER NOT NULL DEFAULT 1,
                seq INTEGER NOT NULL DEFAULT 1, state TEXT NOT NULL DEFAULT 'admitted',
                launch TEXT NOT NULL DEFAULT 'intent', slot INTEGER NOT NULL DEFAULT 1,
                offer TEXT, sealed_ledger TEXT, acked INTEGER NOT NULL DEFAULT 0,
                UNIQUE(link, request))""")
            db.execute("""CREATE TABLE IF NOT EXISTS invocations (
                run TEXT, generation INTEGER, invocation TEXT, tool TEXT, args_sha TEXT,
                response_sha TEXT, citation TEXT, PRIMARY KEY(run,generation,invocation))""")

    @contextmanager
    def transaction(self):
        with closing(sqlite3.connect(self.root / "session.sqlite")) as db, db:
            db.row_factory = sqlite3.Row
            db.execute("PRAGMA synchronous = FULL")
            db.execute("BEGIN IMMEDIATE")
            yield db

    @staticmethod
    def snapshot(row):
        return dict(run_id=row["id"], generation=row["generation"], seq=row["seq"],
                    state=row["state"], reason=None, offer=json.loads(row["offer"] or "null"),
                    acked=bool(row["acked"]))

    @staticmethod
    def owned(db, link, run):
        row = db.execute("SELECT * FROM runs WHERE link=? AND id=?", (link, run)).fetchone()
        if row is None:
            raise Refused("not_found")
        return row

    def start(self, link, params):
        expected = self.v["request"]
        try:
            raw = base64.b64decode(params["request_b64"], validate=True)
            body = json.loads(raw)
            if (set(params) != {"request_b64", "request_digest"}
                    or b64(raw) != params["request_b64"] or digest(raw) != params["request_digest"]
                    or wire(body) != raw or set(body) != set(expected["body"])
                    or not isinstance(body["question"], str)
                    or len(body["question"].encode()) > 4096):
                raise ValueError
        except (ValueError, TypeError, KeyError, binascii.Error) as exc:
            raise Refused("invalid") from exc
        with self.transaction() as db:
            old = db.execute("SELECT * FROM runs WHERE link=? AND request=?",
                             (link, body["request_id"])).fetchone()
            if old:
                if old["digest"] != params["request_digest"]:
                    raise Refused("conflict")
                return self.snapshot(old)
            # Fresh approvals in this fixture are bound to one exact outbox entry.
            if link != self.v["link_id"] or params["request_digest"] != expected["sha256"]:
                raise Refused("unavailable")
            if self.now >= body["deadline_ms"]:
                raise Refused("expired")
            run = self.v["start"]["result"]["run_id"]
            db.execute("INSERT INTO runs(id,link,request,digest,deadline) VALUES(?,?,?,?,?)",
                       (run, link, body["request_id"], params["request_digest"], body["deadline_ms"]))
            return self.snapshot(self.owned(db, link, run))

    def bind_launch(self, link, run):
        # Stand-in for launch submission and peer binding, no real Pi launch.
        if any((p / ".git").exists() for p in (self.root, *self.root.parents)):
            raise Refused("not_scratch")
        with self.transaction() as db:
            row = self.owned(db, link, run)
            if row["launch"] != "intent" or row["state"] != "admitted":
                raise Refused("stopped")
            db.execute("UPDATE runs SET state='working', launch='bound', seq=seq+1 WHERE id=?", (run,))

    def read(self, link, run):
        with self.transaction() as db:
            return self.snapshot(self.owned(db, link, run))

    def cancel(self, link, run_id, generation):
        with self.transaction() as db:
            row = self.owned(db, link, run_id)
            if generation != row["generation"]:
                raise Refused("stale_generation")
            if row["state"] in ("admitted", "working"):
                db.execute("UPDATE runs SET state='stop_requested', seq=seq+1 WHERE id=?", (run_id,))
            return self.snapshot(self.owned(db, link, run_id))

    def confirmed_stop(self, link, run):
        with self.transaction() as db:
            if self.owned(db, link, run)["state"] == "stop_requested":
                db.execute("UPDATE runs SET state='cancelled', slot=0, seq=seq+1 WHERE id=?", (run,))

    def accept_reply(self, binding, inv, tool, args_sha, reply):
        with self.transaction() as db:
            row = self.owned(db, binding["link"], binding["run_id"])
            if (row["generation"] != binding["generation"] or row["state"] != "working"
                    or row["launch"] != "bound" or self.now >= row["deadline"]):
                raise Refused("stopped")
            db.execute("INSERT INTO invocations VALUES(?,?,?,?,?,?,?)",
                       (row["id"], row["generation"], inv, tool, args_sha, digest(wire(reply)),
                        reply.get("citation")))

    @staticmethod
    def ledger(db, row):
        return [dict(item) for item in db.execute("""SELECT invocation,tool,args_sha,response_sha,citation
            FROM invocations WHERE run=? AND generation=? ORDER BY rowid""",
            (row["id"], row["generation"]))]

    @staticmethod
    def seal(args):
        try:
            if set(args) != {"text", "citations"} or not isinstance(args["citations"], list):
                raise ValueError
            text = args["text"].encode("utf-8")
            citations = args["citations"]
            if not text or len(citations) > 64:
                raise ValueError
            keys = []
            for item in citations:
                start, end, handle = item["start_byte"], item["end_byte"], item["handle"]
                if (set(item) != {"start_byte", "end_byte", "handle"} or type(start) is not int
                        or type(end) is not int or not 0 <= start < end <= len(text)
                        or not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", handle)):
                    raise ValueError
                text[start:end].decode("utf-8")  # Both ends must be scalar boundaries.
                keys.append((start, end, handle))
            if len(set(keys)) != len(keys):
                raise ValueError
            raw = wire(dict(v=1, kind="investigation_report", text=args["text"],
                            citations=[dict(start_byte=s, end_byte=e, handle=h)
                                       for s, e, h in sorted(keys)]))
            if len(raw) > 65536:
                raise ValueError
        except (AttributeError, KeyError, TypeError, ValueError) as exc:
            raise Refused("invalid") from exc
        return raw, {h for _, _, h in keys}

    def stage(self, sha, raw):
        dst = self.blobs / sha
        with tempfile.NamedTemporaryFile(dir=self.blobs) as tmp:
            tmp.write(raw)
            tmp.flush()
            os.fsync(tmp.fileno())
            try:
                os.link(tmp.name, dst)  # Publish once, never overwrite existing bytes.
            except FileExistsError:
                if dst.read_bytes() != raw:
                    raise Refused("unavailable") from None
            fd = os.open(self.blobs, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)

    def finish(self, binding, args, fault=lambda phase: None):
        raw, handles = self.seal(args)
        sha = digest(raw)
        link, run, generation = (binding[k] for k in ("link", "run_id", "generation"))

        def check(db):
            row = self.owned(db, link, run)
            if row["generation"] != generation:
                raise Refused("stale_generation")
            if row["offer"] is not None:
                receipt = json.loads(row["offer"])
                if receipt["sha256"] != sha:
                    raise Refused("conflict")
                return receipt, None
            if row["state"] != "working" or row["launch"] != "bound" or self.now >= row["deadline"]:
                raise Refused("stopped")
            ledger = self.ledger(db, row)
            if not handles.issubset({entry["citation"] for entry in ledger}):
                raise Refused("invalid")
            return None, ledger

        with self.transaction() as db:
            old, _ = check(db)
            if old is not None:
                return dict(offer=old)
        fault("before_blob")
        self.stage(sha, raw)
        fault("after_blob")
        with self.transaction() as db:
            old, ledger = check(db)
            if old is not None:
                return dict(offer=old)
            offer = dict(corral_instance=self.v["hello"]["result"]["corral_instance"],
                         run_id=run, generation=generation, report_id="report_A", sha256=sha,
                         bytes=len(raw))
            db.execute("""UPDATE runs SET state='finished', seq=seq+1, offer=?, sealed_ledger=?
                WHERE id=?""", (wire(offer), wire(ledger), run))
            fault("before_row_commit")
        fault("after_row_commit")
        return dict(offer=offer)

    def offer_for(self, db, link, run_id, generation, report_id):
        offer = self.snapshot(self.owned(db, link, run_id))["offer"]
        if not offer or (generation, report_id) != (offer["generation"], offer["report_id"]):
            raise Refused("not_found")
        return offer

    def fetch(self, link, **identity):
        with self.transaction() as db:
            offer = self.offer_for(db, link, **identity)
            try:
                raw = (self.blobs / offer["sha256"]).read_bytes()
            except FileNotFoundError as exc:
                raise Refused("unavailable") from exc
            if len(raw) != offer["bytes"] or digest(raw) != offer["sha256"]:
                raise Refused("unavailable")
            return dict(offer=offer, report_b64=b64(raw))

    def ack(self, link, sha256, bytes, **identity):
        with self.transaction() as db:
            offer = self.offer_for(db, link, **identity)
            if (sha256, bytes) != (offer["sha256"], offer["bytes"]):
                raise Refused("conflict")
            db.execute("UPDATE runs SET acked=1,seq=seq+1 WHERE id=? AND acked=0", (offer["run_id"],))
            return dict(offer=offer, acked=True)
