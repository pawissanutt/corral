"""Synthetic Lite contract peer for Corral component QA only, not a Lite host."""
import array
import os
import select
import socket
import time

from approval import now_ms
from protocol import (FRAME, POLICY_REF, Failure, digest, hex_digest, ident, keys, load, need,
                      recv_frame, send_frame, wire)


class LitePeer:
    def __init__(self, work, bootstrap, enrollment, fixture, approval, script, reconnect=False):
        self.work = work
        self.bootstrap = bootstrap
        self.work.settimeout(3)
        self.bootstrap.settimeout(3)
        self.enrollment = enrollment
        self.fixture = fixture
        self.approval = approval
        self.script = script
        self.reconnect = reconnect
        self.channels = {}
        self.handshakes = {"evidence": 0, "host_poll": 0}
        self.sequence = 0
        self.dropped_settlement = False

    def close(self):
        self.work.close()
        self.bootstrap.close()
        for sock in self.channels.values():
            sock.close()

    def hello(self):
        self.work.settimeout(15)
        send_frame(self.work, dict(self.enrollment, type="link_ready", role="lite"))
        need(recv_frame(self.work) == dict(self.enrollment, type="link_ready"), "work_link_ready")
        hello = self.ask("hello", {"versions": [1], "profiles": ["lite-analyst-v1"]})
        need(keys(hello, "v", "profile", "corral_instance", "policies") and hello["v"] == 1
             and hello["profile"] == "lite-analyst-v1"
             and hello["corral_instance"] == self.enrollment["corral_instance"]
             and type(hello["policies"]) is list and len(hello["policies"]) == 1, "work_hello")
        policy = hello["policies"][0]
        need(keys(policy, "manifest", "policy_ref") and policy["policy_ref"] == POLICY_REF
             and digest(policy["manifest"]) == POLICY_REF, "work_hello_policy")
        self.work.settimeout(3)

    def ask(self, op, args, error=None):
        self.sequence += 1
        call_id = f"qa_peer_{self.sequence}"
        send_frame(self.work, {"v": 1, "type": "request", "id": call_id, "op": op, "params": args})
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            result = self.pump(min(0.2, max(0, deadline - time.monotonic())))
            if result is None:
                continue
            field = "error" if error else "result"
            need(keys(result, "v", "type", "id", field) and result["v"] == 1
                 and result["type"] == "response" and result["id"] == call_id, "work_response")
            need(not error or result[field] == error, "work_expected_refusal")
            return result[field]
        raise Failure("work_response_timeout")

    def pump(self, timeout):
        readers = [self.work, self.bootstrap, *self.channels.values()]
        ready = select.select(readers, [], [], timeout)[0]
        if self.work in ready:
            message = recv_frame(self.work)
            if type(message) is dict and message.get("type") == "response":
                return message
            self.reverse_call(message)
        if self.bootstrap in ready:
            self.attach()
        for name, sock in list(self.channels.items()):
            if sock in ready:
                self.broker(name, sock)
        return None

    def reverse_call(self, msg):
        need(keys(msg, "v", "type", "id", "op", "params") and msg["v"] == 1
             and msg["type"] == "request" and ident(msg["id"]), "work_reverse_envelope")
        approval = self.approval
        op, p = msg["op"], msg["params"]
        if op == "bind_session":
            need(not self.reconnect and p == approval.binding() and approval.run is not None
                 and approval.binds == 0, "work_bind")
            approval.binds += 1
            result = {"bound": True}
        elif op == "reserve_turn":
            need(not self.reconnect and approval.binds == 1, "work_reserve_outside_binding")
            need(keys(p, "body", "reserve_digest"), "work_reserve_envelope")
            b = p["body"]
            step = len(approval.reserves) + 1
            need(step <= self.script.available_turns and len(approval.settles) == step - 1,
                 "work_extra_or_unsettled_reserve")
            need(keys(b, "v", "kind", "run_id", "generation", "step", "request_digest", "policy_ref",
                      "input_digest", "max_cost_microusd") and hex_digest(b["input_digest"])
                 and b == {"v": 1, "kind": op, "run_id": approval.run, "generation": 1, "step": step,
                           "request_digest": approval.start["request_digest"], "policy_ref": POLICY_REF,
                           "input_digest": b["input_digest"], "max_cost_microusd": 1000}
                 and p["reserve_digest"] == digest(b), "work_reserve")
            result = {"permit_id": f"qa_permit_{step}", "expires_ms": now_ms() + 4000,
                      "reserve_digest": p["reserve_digest"]}
            approval.reserves.append((p, result))
        elif op == "settle_turn":
            need(keys(p, "body", "settle_digest"), "work_settle_envelope")
            b = p["body"]
            step = b.get("step") if isinstance(b, dict) else None
            need(type(step) is int and 1 <= step <= len(approval.reserves), "work_settle_step")
            reserve, permit = approval.reserves[step - 1]
            indeterminate = approval.scenario in ("forbidden", "reconnect", "crash") or (
                approval.scenario == "repeated_id" and step == 2
            )
            outcome = "indeterminate" if indeterminate else "completed"
            need(b == {"v": 1, "kind": op, "run_id": approval.run, "generation": 1, "step": step,
                       "permit_id": permit["permit_id"], "reserve_digest": permit["reserve_digest"],
                       "outcome": outcome, "charged_microusd": 760 if outcome == "completed" else 1000}
                 and p["settle_digest"] == digest(b), "work_settlement")
            need(len(self.script.digests) == step and
                 self.script.digests[step - 1] == reserve["body"]["input_digest"], "work_final_body_digest")
            if self.reconnect:
                previous = [] if approval.scenario == "crash" else [p]
                need(approval.settles == previous, "work_settlement_replay_mismatch")
                approval.settles.append(p)
            else:
                need(approval.scenario != "crash" and len(approval.settles) == step - 1,
                     "work_duplicate_or_precrash_settlement")
                approval.settles.append(p)
                if approval.scenario == "reconnect":
                    # Drop the ack at an observable barrier, after the real POST and durable seal.
                    self.dropped_settlement = True
                    self.close()
                    return
            result = {"recorded": True, "settle_digest": p["settle_digest"]}
        elif op == "fence_session":
            need(p == {"run_id": approval.run, "generation": 1, "reason": "stopped"}, "work_fence_notice")
            result = {"fenced": True}
        else:
            raise Failure("work_unknown_reverse_op")
        send_frame(self.work, {"v": 1, "type": "response", "id": msg["id"], "result": result})

    def attach(self):
        fds = array.array("i")
        raw, control, flags, _ = self.bootstrap.recvmsg(4096, socket.CMSG_SPACE(4 * fds.itemsize),
                                                        socket.MSG_CMSG_CLOEXEC)
        try:
            for level, ctype, data in control:
                need(level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS, "broker_control")
                fds.frombytes(data[:len(data) // fds.itemsize * fds.itemsize])
            need(raw and not (flags & ~socket.MSG_CMSG_CLOEXEC) and len(control) == 1 and len(fds) == 2 and
                 not self.reconnect and not self.channels and self.approval.binds == 1, "broker_attach_fds")
            want = dict(self.approval.binding(), v=1, type="broker_attach",
                        link_id=self.enrollment["link_id"], channel_epoch=self.enrollment["channel_epoch"])
            need(load(raw) == want, "broker_attach_binding")
            stats = [os.fstat(fd) for fd in fds]
            need((stats[0].st_dev, stats[0].st_ino) != (stats[1].st_dev, stats[1].st_ino),
                 "broker_alias_fd")
            for name in ("evidence", "host_poll"):
                conn = socket.socket(fileno=fds.pop(0))
                self.channels[name] = conn
                need(conn.family == socket.AF_UNIX and conn.type == socket.SOCK_SEQPACKET
                     and conn.getpeername() == "", "broker_data_socket")
                conn.settimeout(3)
            self.approval.attachments += 1
            self.bootstrap.sendall(wire(dict(want, type="broker_attached")))
        finally:
            for fd in fds:
                os.close(fd)

    def broker(self, name, sock):
        raw, ancillary, flags, _ = sock.recvmsg(FRAME)
        if not raw and not ancillary and not flags:
            sock.close()
            self.channels.pop(name)
            return
        need(raw and not ancillary and not flags, "broker_packet")
        request = load(raw)
        count = self.handshakes[name]
        if count != 3:
            self.negotiate(name, sock, count, request)
            self.handshakes[name] += 1
            return
        expected = self.approval.binding()
        need(keys(request, "run_id", "generation", "invocation_id", "args_digest", "request")
             and request["run_id"] == expected["run_id"] and request["generation"] == expected["generation"]
             and ident(request["invocation_id"]), "broker_invocation")
        parsed = request["request"]
        need(keys(parsed, "jsonrpc", "id", "method", "params") and parsed["jsonrpc"] == "2.0"
             and parsed["method"] == "tools/call" and keys(parsed["params"], "name", "arguments"),
             "broker_rpc")
        tool, args = parsed["params"]["name"], parsed["params"]["arguments"]
        need(request["args_digest"] == digest({"v": 1, "kind": "mcp_args", "tool": tool, "args": args}),
             "broker_args_digest")
        reply = self.effect(name, tool, args, request["invocation_id"])
        payload = {"jsonrpc": "2.0", "id": parsed["id"],
                   "result": {"content": [], "structuredContent": reply, "isError": False}}
        sock.sendall(wire({"run_id": expected["run_id"], "generation": expected["generation"],
                           "invocation_id": request["invocation_id"], "response": payload}))

    def negotiate(self, name, sock, count, request):
        incoming = self.fixture.packet["startup"][count]["request"]
        need(name == "evidence" or self.handshakes["evidence"] == 3, "broker_startup_fd_order")
        need(type(request) is dict and request.get("jsonrpc") == "2.0"
             and request.get("method") == incoming["method"], "broker_startup_order")
        if count == 1:
            need(request == incoming, "broker_initialized")
            return
        if count == 0:
            p = request.get("params")
            need(keys(request, "jsonrpc", "id", "method", "params")
                 and keys(p, "protocolVersion", "clientInfo", "capabilities")
                 and p["protocolVersion"] == "2025-06-18" and p["capabilities"] == {}
                 and keys(p["clientInfo"], "name", "version"), "broker_initialize")
            result = self.fixture.sample_init
        else:
            need(keys(request, "jsonrpc", "id", "method", "params") and request["params"] == {},
                 "broker_list_params")
            descriptors = list(self.fixture.descriptors[name])
            if name == "evidence" and self.approval.scenario == "registry":
                descriptors.append(dict(descriptors[0], name="mitimind_read_note"))
            result = {"tools": descriptors}
        sock.sendall(wire({"jsonrpc": "2.0", "id": request["id"], "result": result}))

    def effect(self, channel, tool, args, invocation):
        a = self.approval
        samples = self.fixture.samples
        expected = {
            "happy": ["search_materials", "read_evidence", "calculate_metric"],
            "pending": ["search_materials", "read_evidence", "calculate_metric", "read_result"],
            "repeated_id": ["search_materials"],
        }.get(a.scenario, [])
        index = len(a.broker_calls)
        need(index < len(expected) and tool == expected[index]
             and channel == ("host_poll" if tool == "read_result" else "evidence"), "broker_forbidden_call")
        need(invocation not in {prior[1] for prior in a.broker_calls}, "broker_repeated_invocation")
        a.broker_calls.append((tool, invocation))
        if tool == "search_materials":
            need(args == self.fixture.packet["main"][0]["request"]["params"]["arguments"], "broker_search_args")
            return {"status": "ok", "hits": [dict(samples["search"]["hits"][0], source=a.source)]}
        if tool == "read_evidence":
            need(args == {"source": a.source, "start_byte": 0, "max_bytes": 256}, "broker_live_read_args")
            return dict(samples["read"], source=a.source, citation=a.field_cite)
        if tool == "calculate_metric":
            need(args == {"metric": a.metric, "inputs": [a.source]}, "broker_live_metric_args")
            if a.scenario == "pending":
                a.pending_at = time.monotonic()
                return dict(samples["pending"], result=a.result)
        if tool == "read_result":
            need(args == {"result": a.result} and a.pending_at is not None
                 and time.monotonic() - a.pending_at >= 1, "broker_host_poll_before_min_interval")
        return dict(samples["metric"], result=a.result, citation=a.metric_cite)
