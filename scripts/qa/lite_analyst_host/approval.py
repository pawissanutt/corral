"""In-memory synthetic approval and per-run fixture selectors for the fake peer."""
import base64
import secrets
import time

from protocol import POLICY_REF, sha, wire
from responder import METRIC_LINE


def now_ms():
    return time.time_ns() // 1_000_000


def fresh():
    return secrets.token_urlsafe(16)


class Approval:
    def __init__(self, fixture, scenario):
        suffix = secrets.token_hex(6)
        self.source = f"qa_src_{suffix}"
        self.metric = f"qa_metric_{suffix}"
        self.result = f"qa_result_{suffix}"
        self.field_cite = f"qa_fields_{suffix}"
        self.metric_cite = f"qa_cell_{suffix}"
        self.scenario = scenario
        self.question = fixture.question + METRIC_LINE + self.metric
        self.approval = {
            "v": 1, "kind": "start_investigation", "request_id": fresh(), "grant_id": fresh(),
            "grant_digest": sha(secrets.token_bytes(32)), "grant_generation": 1, "policy_ref": POLICY_REF,
            "question": self.question, "deadline_ms": now_ms() + 75_000,
            "limits": {"calls": 8, "response_bytes": 8192, "total_response_bytes": 32768,
                       "turns": 4, "run_cost_microusd": 4000},
        }
        self.start = self.start_params()
        self.run = None
        self.reserves = []
        self.settles = []
        self.broker_calls = []
        self.binds = 0
        self.attachments = 0
        self.pending_at = None

    def start_params(self, **changes):
        raw = wire(dict(self.approval, **changes))
        return {"request_b64": base64.b64encode(raw).decode(), "request_digest": sha(raw)}

    def binding(self):
        result = {k: self.approval[k] for k in ("request_id", "grant_id", "grant_generation", "policy_ref")}
        return dict(result, request_digest=self.start["request_digest"], run_id=self.run, generation=1)
