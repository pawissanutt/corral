"""In-process fake Lite evidence/delivery peer for the pinned synthetic case."""

import base64
from contextlib import closing
import json
import sqlite3

from session import Refused, Session, digest, wire


class EvidencePeer:
    def __init__(self, root, binding, vectors):
        self.root = root
        self.binding = binding.copy()
        self.v = vectors
        self.calls = {}
        self.releases = 0
        self.disclosed = 0
        self.admissions = 0
        self.polls = 0
        self.revoked = False
        self.ready = False

    def call(self, binding, inv, args_sha, body):
        if binding != self.binding or self.revoked:
            raise Refused("stale_scope")
        limits = self.v["request"]["body"]["limits"]
        if self.releases >= limits["calls"]:
            raise Refused("budget_exceeded")
        if args_sha != digest(wire(body)):
            raise Refused("invalid")
        tool = body["tool"]
        expected = next((item for item in self.v["invocations"]
                         if item["signature"]["body"]["tool"] == tool), None)
        old = self.calls.get(inv)
        if old and args_sha != old[0]:
            raise Refused("conflict")
        if expected is None or body != expected["signature"]["body"]:
            raise Refused("invalid")
        if old:
            reply = old[1]
        elif tool == "calculate_metric":
            if self.admissions:
                raise Refused("conflict")
            self.admissions += 1
            reply = {"status": "pending", "result": "res_feb"}
        elif tool == "read_result":
            if not self.admissions:
                raise Refused("not_found")
            self.polls += 1
            if self.ready:
                reply = dict(status="ok", value_minor=(5100 - 2800) * 100, unit="USD",
                             limitations="Revenue minus expenses; not cash on hand.",
                             result="res_feb", operation_version="subtract_expenses_v1",
                             cell="feb_net", result_kind="computed", citation="cite_feb_net")
            else:
                reply = {"status": "pending", "result": "res_feb"}
        else:
            reply = expected["reply"]
        size = len(wire(reply))
        if size > limits["response_bytes"] or self.disclosed + size > limits["total_response_bytes"]:
            raise Refused("budget_exceeded")
        self.releases += 1
        self.disclosed += size
        self.calls[inv] = (args_sha, reply)
        return dict(run_id=binding["run_id"], generation=binding["generation"], invocation_id=inv,
                    structuredContent=reply)

    def accept_delivery(self, fetch):
        offer = fetch["offer"]
        raw = base64.b64decode(fetch["report_b64"], validate=True)
        if (offer["run_id"] != self.binding["run_id"]
                or offer["generation"] != self.binding["generation"]
                or offer["corral_instance"] != self.v["hello"]["result"]["corral_instance"]
                or digest(raw) != offer["sha256"] or len(raw) != offer["bytes"]):
            raise Refused("invalid")
        body = json.loads(raw)
        sealed, cited = Session.seal({"text": body["text"], "citations": body["citations"]})
        released = {reply.get("citation") for _, reply in self.calls.values()}
        if sealed != raw or not cited.issubset(released):
            raise Refused("invalid")
        identity = wire([offer[k] for k in ("corral_instance", "run_id", "generation", "report_id")])
        database = self.root / "delivered.sqlite"
        with closing(sqlite3.connect(database)) as db, db:
            db.execute("CREATE TABLE IF NOT EXISTS delivered (identity BLOB PRIMARY KEY, offer BLOB, body BLOB)")
            db.execute("INSERT OR IGNORE INTO delivered VALUES (?,?,?)", (identity, wire(offer), raw))
        # ACK follows readback from a fresh connection, not the input buffer.
        with closing(sqlite3.connect(database)) as db:
            stored_offer, stored_body = db.execute(
                "SELECT offer,body FROM delivered WHERE identity=?", (identity,)).fetchone()
            count, = db.execute("SELECT count(*) FROM delivered").fetchone()
        if (stored_offer, stored_body) != (wire(offer), raw):
            raise Refused("conflict")
        return count, stored_body


class EvidenceHost:
    def __init__(self, session, peer, binding, policy):
        self.session = session
        self.peer = peer
        self.binding = binding
        self.model_tools = {tool["name"] for tool in policy["model_tools"]} - {"finish_report"}
        self.pending = None
        self.accepted = []

    def model_call(self, inv, tool, args):
        if tool not in self.model_tools:
            raise Refused("invalid")
        reply = self._exchange(inv, tool, args)
        if reply["status"] == "pending":
            self.pending = reply["result"]
        return reply

    def poll(self, inv, *, lose_reply=False):
        if self.pending is None:
            raise Refused("not_found")
        reply = self._exchange(inv, "read_result", {"result": self.pending}, lose_reply)
        if reply is not None and reply["status"] != "pending":
            self.pending = None
        return reply

    def _exchange(self, inv, tool, args, lose_reply=False):
        body = dict(v=1, kind="mcp_args", tool=tool, args=args)
        sha = digest(wire(body))
        frame = self.peer.call(self.binding, inv, sha, body)
        if lose_reply:
            return None
        if (frame["run_id"], frame["generation"], frame["invocation_id"]) != (
                self.binding["run_id"], self.binding["generation"], inv):
            raise Refused("invalid")
        reply = frame["structuredContent"]
        self.session.accept_reply(self.binding, inv, tool, sha, reply)
        self.accepted.append(reply)
        return reply
