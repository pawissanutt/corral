"""Test the synthetic QA script and inherited broker packet helper, without a daemon."""
import array
import os
import socket
import unittest
from pathlib import Path

from approval import Approval, fresh
from peer import LitePeer
from protocol import CANARY, Failure, Fixture, load, wire
from responder import Conversation

REPO = Path(__file__).resolve().parents[3]
PACKET = Path(os.environ.get("CORRAL_QA_PACKET", REPO / ".evidence/lite-fixtures/docs/protocol/analyst-february-close-v1.json"))
BUNDLE = REPO / "docs/contracts/lite-analyst-v1"


class ScriptTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = Fixture(PACKET, BUNDLE / "synthetic-prompt.txt", BUNDLE / "tool-policy.json")

    def build(self, name):
        approval = Approval(self.fixture, name)
        conversation = Conversation(self.fixture, name, approval.question)
        body = {"model": "fixture-v1", "stream": True, "tools": self.fixture.model_tools,
                "messages": self.fixture.prompt_messages(approval.question)}
        return approval, conversation, body

    def append_response(self, body, script, *payloads):
        issued = script.calls[-1]
        self.assertEqual(len(issued), len(payloads))
        body["messages"].append({"role": "assistant", "content": None, "tool_calls": issued})
        body["messages"].extend({"role": "tool", "tool_call_id": call["id"], "content": wire(payload).decode()}
                                for call, payload in zip(issued, payloads, strict=True))

    def test_actual_handles_and_derived_utf8_spans(self):
        a, script, body = self.build("happy")
        self.assertIn(b"data: [DONE]\n\n", script.next(wire(body)))
        self.append_response(body, script, {"status": "ok", "hits": [
            dict(self.fixture.samples["search"]["hits"][0], source=a.source)]})
        script.next(wire(body))
        calls = [c["function"] for c in script.calls[-1]]
        self.assertEqual(load(calls[0]["arguments"].encode())["source"], a.source)
        self.assertEqual(load(calls[1]["arguments"].encode()), {"inputs": [a.source], "metric": a.metric})
        read = dict(self.fixture.samples["read"], source=a.source, citation=a.field_cite)
        result = dict(self.fixture.samples["metric"], citation=a.metric_cite)
        self.append_response(body, script, read, result)
        script.next(wire(body))
        self.assertTrue(script.derived)
        issued = script.calls[-1][0]["function"]
        self.assertEqual(issued["name"], "finish_report")
        got = load(issued["arguments"].encode())
        for value in ("5,100", "2,800", "2,300", "not cash on hand"):
            self.assertIn(value, got["text"])
        self.assertEqual({c["handle"] for c in got["citations"]}, {a.field_cite, a.metric_cite})
        self.assertEqual({c["end_byte"] for c in got["citations"]}, {len(got["text"].encode())})
        with self.assertRaisesRegex(Failure, "provider_extra_turn"):
            script.next(wire(body))

    def test_exact_prompt_and_canary(self):
        _, script, body = self.build("happy")
        body["messages"][0]["content"] += CANARY
        with self.assertRaisesRegex(Failure, "provider_owner_canary"):
            script.next(wire(body))
        body["messages"][0]["content"] = "changed prompt"
        with self.assertRaisesRegex(Failure, "provider_system_and_question"):
            script.next(wire(body))

    def test_prior_call_id_mismatch_and_pending_not_final(self):
        a, script, body = self.build("pending")
        script.next(wire(body))
        self.append_response(body, script, {"status": "ok", "hits": [
            dict(self.fixture.samples["search"]["hits"][0], source=a.source)]})
        message = body["messages"][-1]
        original = message["tool_call_id"]
        message["tool_call_id"] = "unexpected"
        with self.assertRaisesRegex(Failure, "provider_tool_correlation"):
            script.next(wire(body))
        message["tool_call_id"] = original
        script.next(wire(body))
        self.append_response(body, script, dict(self.fixture.samples["read"], source=a.source),
                             self.fixture.samples["pending"])
        with self.assertRaisesRegex(Failure, "provider_terminal_metric_reply"):
            script.next(wire(body))

    def test_attach_real_unix_packet_and_role_fds(self):
        owners = []

        def pair(kind):
            endpoints = socket.socketpair(socket.AF_UNIX, kind)
            owners.extend(endpoints)
            return endpoints

        work, _ = pair(socket.SOCK_STREAM)
        bootstrap, sending = pair(socket.SOCK_SEQPACKET)
        receiving_fds = [pair(socket.SOCK_SEQPACKET)[0].fileno() for _ in range(2)]
        a, script, _ = self.build("happy")
        a.binds = 1
        a.run = fresh()
        enrollment = {"link_id": fresh(), "channel_epoch": fresh()}
        peer = LitePeer(work, bootstrap, enrollment, self.fixture, a, script)
        try:
            record = dict(enrollment, **a.binding(), v=1, type="broker_attach")
            sending.sendmsg([wire(record)], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", receiving_fds))])
            peer.attach()
            self.assertEqual(load(sending.recv(4096)), dict(record, type="broker_attached"))
            self.assertEqual(a.attachments, 1)
            self.assertEqual(list(peer.channels), ["evidence", "host_poll"])
            self.assertTrue(all(not s.get_inheritable() for s in peer.channels.values()))
        finally:
            peer.close()
            for s in owners:
                s.close()


if __name__ == "__main__":
    unittest.main()
