"""Deterministic test-only POST/SSE responder for the C3 component harness."""
import hmac
import re
import socketserver
import threading

from protocol import CANARY, FAULT_BROKER, FAULT_SANDBOX, FAULT_START, HTTP_BODY, PORT, URL_PATH, Failure, ident, keys, load, need, sha, wire

METRIC_LINE = "\nApproved metric selector: "


def calls_for(step, *specs):
    return [{"id": f"qa_turn_{step}_call_{index}", "type": "function",
             "function": {"name": name, "arguments": wire(args).decode("utf-8")}}
            for index, (name, args) in enumerate(specs)]


class Conversation:
    def __init__(self, fixture, scenario, question, observe=None):
        self.fixture = fixture
        self.scenario = scenario
        self.question = question
        self.observe = observe
        self.available_turns = {"happy": 3, "pending": 3, "forbidden": 1, "repeated_id": 2,
                                "registry": 0, "reconnect": 1, "crash": 1,
                                FAULT_START: 0, FAULT_BROKER: 1, FAULT_SANDBOX: 0}[scenario]
        self.calls = []
        self.digests = []
        self.derived = False
        self.report = None  # Memory-only: inspected in this process, never emitted.

    def next(self, raw):
        need(len(self.digests) < self.available_turns, "provider_extra_turn")
        need(CANARY.encode() not in raw, "provider_owner_canary")
        body = load(raw)
        need(keys(body, "model", "stream", "tools", "messages") and body["model"] == "fixture-v1"
             and body["stream"] is True and body["tools"] == self.fixture.model_tools,
             "provider_pinned_route_or_tools")
        messages = body["messages"]
        need(type(messages) is list and messages[:2] == self.fixture.prompt_messages(self.question),
             "provider_system_and_question")
        tool_results = []
        offset = 2
        for issued in self.calls:
            assistant = {"role": "assistant", "content": None, "tool_calls": issued}
            need(messages[offset:offset + 1] == [assistant], "provider_changed_prior_calls")
            offset += 1
            for call in issued:
                need(offset < len(messages), "provider_missing_tool_result")
                message = messages[offset]
                need(keys(message, "role", "tool_call_id", "content") and message["role"] == "tool"
                     and message["tool_call_id"] == call["id"] and type(message["content"]) is str,
                     "provider_tool_correlation")
                raw_result = message["content"].encode()
                value = load(raw_result)
                need(wire(value) == raw_result, "provider_noncanonical_tool_result")
                tool_results.append(value)
                offset += 1
        need(len(messages) == offset, "provider_hidden_messages")
        step = len(self.digests) + 1
        digest = sha(raw)
        self.digests.append(digest)
        if self.observe is not None:
            self.observe(step, digest)
        if self.scenario == "crash":
            # The controller killed the consumed generation before this socket
            # produced any response. Close without writing (no fake completion).
            return None
        if self.scenario == "reconnect":
            # Complete HTTP exchange, but missing SSE terminal/[DONE]: possible paid send.
            response = make_sse(step, [], truncated=True)
        else:
            issued = self.actions(step, tool_results, messages[1]["content"])
            if self.scenario == "repeated_id" and step == 2:
                # Complete SSE and schema-valid arguments, but this ID was
                # already dispatched in turn 1. None of this batch may run.
                issued[0]["id"] = self.calls[0][0]["id"]
            self.calls.append(issued)
            response = make_sse(step, issued)
        return response

    def actions(self, step, results, question):
        if self.scenario == "forbidden":
            return calls_for(step, ("read_result", {"result": "not_a_model_tool"}))
        if step == 1:
            args = self.fixture.packet["main"][0]["request"]["params"]["arguments"]
            return calls_for(step, ("search_materials", args))
        hit = results[0]
        need(keys(hit, "status", "hits") and hit["status"] == "ok"
             and type(hit["hits"]) is list and len(hit["hits"]) == 1, "provider_search_reply")
        hit = hit["hits"][0]
        need(keys(hit, "source", "label") and ident(hit["source"]) and "February" in hit["label"],
             "provider_search_hit")
        source = hit["source"]
        selector = question.rsplit(METRIC_LINE, 1)[-1]
        need(METRIC_LINE in question and ident(selector), "provider_visible_metric_selector")
        if step == 2:
            return calls_for(step, ("read_evidence", {"source": source, "start_byte": 0, "max_bytes": 256}),
                             ("calculate_metric", {"metric": selector, "inputs": [source]}))
        read, metric = results[1:]
        need(keys(read, "status", "source", "source_version", "start_byte", "text", "citation")
             and read["status"] == "ok" and read["source"] == source and read["start_byte"] == 0
             and ident(read["source_version"]) and ident(read["citation"]), "provider_read_reply")
        need(keys(metric, "status", "value_minor", "unit", "limitations", "result", "operation_version",
                  "cell", "result_kind", "citation") and metric["status"] == "ok"
             and type(metric["value_minor"]) is int and metric["result_kind"] == "captured"
             and all(ident(metric[k]) for k in ("result", "operation_version", "cell", "citation"))
             and metric["unit"] == "report_units" and type(metric["limitations"]) is str,
             "provider_terminal_metric_reply")
        projection = load(read["text"].encode())
        need(keys(projection, "title", "revenue", "expenses") and projection["title"] == hit["label"]
             and type(projection["revenue"]) is int and type(projection["expenses"]) is int,
             "provider_approved_projection")
        revenue, expenses = projection["revenue"], projection["expenses"]
        value = metric["value_minor"]
        expected = self.fixture.packet["fake_broker_only"]["synthetic_captured_result"]
        need(revenue - expenses == value == expected["result_identity"]["rows"][0][0],
             "provider_expected_cell")
        claim = (f"{projection['title']} records {revenue:,} revenue and {expenses:,} expenses; "
                 f"the difference is {value:,} in report units. {metric['limitations']}")
        end = len(claim.encode())
        report = {"text": claim, "citations": [{"start_byte": 0, "end_byte": end, "handle": handle}
                                                for handle in sorted((read["citation"], metric["citation"]))]}
        self.report = report
        self.derived = True
        return calls_for(step, ("finish_report", report))


def make_sse(step, calls, truncated=False):
    def event(delta, finish=None):
        return b"data: " + wire({
            "id": f"qa_chunk_{step}", "object": "chat.completion.chunk", "created": 1,
            "model": "fixture-v1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }) + b"\n\n"

    start = event({"role": "assistant"})
    if truncated:
        return start
    return (start + b"".join(event({"tool_calls": [dict(call, index=i)]}) for i, call in enumerate(calls))
            + event({}, "tool_calls") + b"data: [DONE]\n\n")


class HTTPHandler(socketserver.BaseRequestHandler):
    def handle(self):
        broker_drop = self.server.conversation.scenario == FAULT_BROKER
        # Connect precedes reserve/consume. The broker-drop controller can hold
        # this socket through the bounded permit deadline, but must see no bytes.
        self.request.settimeout(8 if broker_drop else 2)
        try:
            if broker_drop:
                need(self.request.recv(1) == b"", "broker_drop_provider_byte")
                self.server.empty_closes += 1
                return
            with self.request.makefile("rb") as reader:
                need(reader.readline(8193) == f"POST {URL_PATH} HTTP/1.1\r\n".encode(), "provider_request_line")
                headers = {}
                remaining = 8192
                while True:
                    line = reader.readline(remaining + 1)
                    remaining -= len(line)
                    need(remaining >= 0 and line.endswith(b"\r\n"), "provider_header_size")
                    if line == b"\r\n":
                        break
                    name, sep, value = line[:-2].partition(b":")
                    name = name.lower()
                    need(sep and re.fullmatch(rb"[a-z-]+", name) and name not in headers, "provider_headers")
                    headers[name] = value.strip()
                need(set(headers) == {b"host", b"authorization", b"content-type", b"accept",
                                      b"connection", b"content-length"} and
                     headers[b"host"] == f"127.0.0.1:{PORT}".encode() and
                     hmac.compare_digest(headers[b"authorization"], self.server.authorization) and
                     headers[b"content-type"] == b"application/json" and
                     headers[b"accept"] == b"text/event-stream" and headers[b"connection"] == b"close",
                     "provider_headers_auth_route")
                length = headers[b"content-length"]
                need(re.fullmatch(rb"[1-9][0-9]{0,5}", length) and int(length) <= HTTP_BODY,
                     "provider_content_length")
                body = reader.read(int(length))
                need(len(body) == int(length), "provider_body_size")
            sse = self.server.conversation.next(body)
            if sse is None:
                return
            response = (b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\n"
                        + f"Content-Length: {len(sse)}\r\n\r\n".encode() + sse)
        except Failure as exc:
            self.server.error = exc
            response = b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        except (OSError, TypeError, KeyError, ValueError, AttributeError):
            self.server.error = Failure("provider_invalid_request")
            response = b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        try:
            self.request.sendall(response)
        except OSError:
            self.server.error = Failure("provider_response_write")


class ScriptedHTTP(socketserver.TCPServer):
    allow_reuse_address = True

    def __init__(self, conversation, bearer, stop):
        self.conversation = conversation
        self.authorization = b"Bearer " + bearer
        self.error = None
        self.accepted = 0
        self.empty_closes = 0
        self.stop = stop
        super().__init__(("127.0.0.1", PORT), HTTPHandler)
        self.thread = threading.Thread(target=self.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)

    def get_request(self):
        request = super().get_request()
        self.accepted += 1
        return request

    def handle_error(self, _request, _address):
        self.error = Failure("provider_handler")  # Never echo headers, body, or tracebacks.

    def serve(self):
        self.thread.start()  # Socket bind/listen happened synchronously in __init__.
        return self

    def close(self):
        self.shutdown()
        self.server_close()
        self.thread.join(3)
        if self.thread.is_alive():
            self.stop("provider_teardown")
        if self.error is not None:
            raise self.error
