"""Own the exact daemon subprocess and preconnected launch descriptors."""
import fcntl
import os
import resource
import socket
import subprocess
import time

from environment import DAEMON_ENV, SCRATCH, Teardown, wait_namespace_empty
from peer import LitePeer
from protocol import CANARY, DAEMON, FAULT_SCENARIOS, OPT_IN, need, pipe_record, wire

PYTHON = "/usr/bin/python3"
ARGS = [DAEMON, "lite-analyst-fixture-v1"]


def exec_daemon(fields):
    # A fresh single-threaded process. Using preexec_fn would fork Python while
    # the HTTP responder thread is live. Duplicate before touching fixed slots.
    need(len(fields) in (6, 8), "exec_descriptor_layout")
    fault_case = fields.pop() if len(fields) == 8 else None
    source = [int(s) for s in fields]
    need(len(source) == len(set(source)) and (fault_case is None or fault_case in FAULT_SCENARIOS),
         "exec_descriptor_layout")
    saved = [fcntl.fcntl(fd, fcntl.F_DUPFD_CLOEXEC, 32) for fd in source]
    for target, fd in enumerate(saved, 3):
        os.dup2(fd, target, inheritable=True)
    os.closerange(3 + len(source), max(source + saved) + 1)
    resource.setrlimit(resource.RLIMIT_FSIZE, (8 * 1024 * 1024, 8 * 1024 * 1024))
    fault_args = ["--fault-fd", "9", "--fault-case", fault_case] if fault_case is not None else []
    os.execve(DAEMON, [*ARGS, OPT_IN, *fault_args], dict(DAEMON_ENV, CORRAL_QA_OWNER_CANARY=CANARY))


def record_pipe(record):
    src, dst = os.pipe()
    try:
        need(len(record) <= 4096 and os.write(dst, record) == len(record), "bootstrap_pipe_write")
    finally:
        os.close(dst)
    return src


class Daemon:
    def __init__(self, root, enrollment, bearer, fixture, approval, script, reconnect, *, fault_case=None):
        work, daemon_work = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        boot, daemon_boot = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.ready, ready_write = os.pipe()
        lifetime_read, self.lifetime = os.pipe()
        descriptor_order = [record_pipe(wire(enrollment) + b"\n"), daemon_work.fileno(), daemon_boot.fileno(),
                            ready_write, lifetime_read, record_pipe(bearer + b"\n")]
        self.diag = root / ("reconnect.log" if reconnect else "daemon.log")
        self.enrollment = enrollment
        self.peer = LitePeer(work, boot, enrollment, fixture, approval, script, reconnect)
        self.fault = daemon_fault = None
        try:
            suffix = []
            if fault_case is not None:
                self.fault, daemon_fault = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
                self.fault.settimeout(3)
                descriptor_order.append(daemon_fault.fileno())
                suffix.append(fault_case)
            with self.diag.open("xb") as log, (SCRATCH / "owner-fd").open("rb") as sentinel:
                os.set_inheritable(sentinel.fileno(), True)
                # close_fds/pass_fds must drop this otherwise-inheritable ambient FD.
                self.process = subprocess.Popen([PYTHON, "-EsB", "/qa/inside.py", "--exec-daemon",
                                                 *map(str, descriptor_order), *suffix],
                                                cwd=SCRATCH, env={}, stdin=subprocess.DEVNULL,
                                                stdout=log, stderr=log, pass_fds=descriptor_order)
        except BaseException:
            self.peer.close()
            if self.fault is not None:
                self.fault.close()
            os.close(self.ready)
            os.close(self.lifetime)
            raise
        finally:
            daemon_work.close()
            daemon_boot.close()
            if daemon_fault is not None:
                daemon_fault.close()
            for fd in (descriptor_order[0], ready_write, lifetime_read, descriptor_order[5]):
                os.close(fd)

    def boot(self):
        deadline = time.monotonic() + 15
        self.peer.hello()
        check = dict(self.enrollment, type="ready")
        for extra in ("lite_instance", "corral_instance", "prior_channel_epoch"):
            del check[extra]
        ready, self.ready = self.ready, None
        need(pipe_record(ready, deadline) == check and self.process.poll() is None, "daemon_ready")

    def close(self, expected=None):
        if self.lifetime is not None:
            os.close(self.lifetime)
            self.lifetime = None
        try:
            try:
                status = self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                # Popen owns the unreaped identity; never search by process name/PID.
                self.process.terminate()
                try:
                    status = self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    status = self.process.wait(timeout=2)
                if expected is not None:
                    raise Teardown("daemon_stop_required_signal")
        finally:
            self.peer.close()
            if self.fault is not None:
                self.fault.close()
            if self.ready is not None:
                os.close(self.ready)
                self.ready = None
        wait_namespace_empty()
        if expected is not None:
            need(status == expected, "daemon_exit")
        need(self.diag.stat().st_size <= 4096, "daemon_diagnostics_size")
        # Only this static failure line is allowed, no raw bodies, stderr from
        # Pi/shim/bwrap, credential/paths, canaries or full tracebacks.
        need(self.diag.read_bytes() in (b"", b"Error: public Lite fixture stopped or unavailable\n"),
             "daemon_unsanitized_diagnostics")
