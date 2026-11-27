"""Bounded synthetic QA wire helpers, never imported by a product component."""
import hashlib
import json
import os
import re
import struct
from pathlib import Path

FRAME = 131072
HTTP_BODY = 262144
PORT = 43173
URL_PATH = "/v1/chat/completions"
PROFILE_SHA = "c145f94492ad6683432aeb293b96be3385c1b0f7f603dfdb6e81f29de069388c"
PACKET_SHA = "12bf636c36606452e362b5c454de12284a77738fb20dfc0e7fcbfe1aab1f46a5"
RAW_POLICY_SHA = "38fae11185d2b48302d026ed788b04f1e5e6340fced8aa08f9d24707f9607de5"
PROMPT_SHA = "103ccacd5a80abbd9f83e4d64441cbb6b07e5d7a8c28c793d15bc16aaded8f02"
POLICY_REF = "b363edfa14cbf9f9f32cf9578d260c99e762e12719df2b0505358f12e94e0104"
CANARY = "ZZ_QA_UNAPPROVED_OWNER_CANARY_ZZ"
DAEMON = "/fixture/corral/bin/corrald-next"
OPT_IN = "--enable-public-fixture-e2e"
SCENARIOS = ("happy", "pending", "forbidden", "repeated_id", "registry", "reconnect", "crash")
FAULT_START = "corral.start.committed"
FAULT_BROKER = "corral.broker.after_admission"
FAULT_SANDBOX = "corral.sandbox.denied"
FAULT_SCENARIOS = (FAULT_START, FAULT_BROKER, FAULT_SANDBOX)


class Failure(Exception):
    """Only static assertion names may be put in a receipt."""


def need(condition, code):
    if not condition:
        raise Failure(code)


def keys(value, *expected):
    return type(value) is dict and value.keys() == set(expected)


def wire(value):
    # JCS over this fixture's known ASCII object keys and safe integer values.
    return json.dumps(value, ensure_ascii=False, allow_nan=False,
                      separators=(",", ":"), sort_keys=True).encode("utf-8")


def load(data):
    def pairs(fields):
        result = {}
        for key, value in fields:
            need(key not in result, "duplicate_key")
            result[key] = value
        return result

    def bad_number(_):
        raise Failure("non_integer_json")

    def integer(raw):
        result = int(raw)
        need(abs(result) <= 9007199254740991, "unsafe_integer")
        return result

    try:
        value = json.loads(data.decode("utf-8"), object_pairs_hook=pairs,
                           parse_float=bad_number, parse_constant=bad_number, parse_int=integer)
        wire(value)  # Reject lone surrogates.
        return value
    except (UnicodeError, ValueError, RecursionError):
        raise Failure("invalid_json") from None


def sha(data):
    return hashlib.sha256(data).hexdigest()


def file_sha(path):
    with path.open("rb") as reader:
        return hashlib.file_digest(reader, "sha256").hexdigest()


def digest(value):
    return sha(wire(value))


def ident(value):
    return type(value) is str and re.fullmatch(r"[A-Za-z0-9_-]{1,64}", value) is not None


def hex_digest(value):
    return type(value) is str and re.fullmatch(r"[a-f0-9]{64}", value) is not None


def get_exact(sock, length):
    data = bytearray()
    while len(data) < length:
        chunk, ancillary, flags, _ = sock.recvmsg(length - len(data))
        need(chunk and not ancillary and not flags, "work_eof_or_ancillary")
        data.extend(chunk)
    return bytes(data)


def recv_frame(sock):
    length, = struct.unpack("!I", get_exact(sock, 4))
    need(0 < length <= FRAME, "work_size")
    return load(get_exact(sock, length))


def send_frame(sock, obj):
    raw = wire(obj)
    need(0 < len(raw) <= FRAME, "work_send_size")
    sock.sendall(struct.pack("!I", len(raw)) + raw)


def pipe_record(fd, deadline):
    import select
    import time

    data = bytearray()
    try:
        while True:
            need(select.select([fd], [], [], max(0, deadline - time.monotonic()))[0],
                 "bootstrap_ready_timeout")
            chunk = os.read(fd, 4097 - len(data))
            if not chunk:
                break
            data.extend(chunk)
            need(len(data) <= 4096, "ready_record_size")
        need(data.count(b"\n") == 1 and data.endswith(b"\n"), "ready_record_framing")
        return load(bytes(data[:-1]))
    finally:
        os.close(fd)


def checked_file(path, expected):
    data = Path(path).read_bytes()
    need(sha(data) == expected, "fixture_pin")
    return data


class Fixture:
    def __init__(self, packet, prompt, policy):
        self.packet = load(checked_file(packet, PACKET_SHA))
        self.prompt = checked_file(prompt, PROMPT_SHA).decode("utf-8")
        self.policy = load(checked_file(policy, RAW_POLICY_SHA))
        self.model_tools = [{"type": "function", "function": {
            "name": t["name"], "description": t["description"], "parameters": t["inputSchema"],
        }} for t in self.policy["model_tools"]]
        self.descriptors = {
            "evidence": self.policy["model_tools"][:3], "host_poll": self.policy["host_only_tools"],
        }
        self.samples = {
            name: sample["reply"]["result"]["structuredContent"]
            for name, sample in zip(("search", "read", "pending", "metric"), self.packet["main"], strict=False)
        }
        self.sample_init = self.packet["startup"][0]["reply"]["result"]
        self.question = self.packet["question"]

    def prompt_messages(self, question):
        return [{"role": "system", "content": self.prompt}, {"role": "user", "content": question}]
