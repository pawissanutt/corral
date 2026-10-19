"""Local PM terminal receiver; no model, provider, or herdr replacement.

DSR replies fence terminal rendering before IPC acknowledgements. Multiline
policy input is displayed as a composer until Enter, then captured whole. A fence
sent through the same PTY reports all submissions preceding it, so duplicate
checks never infer absence from a timeout.
"""

import json
import os
import re
import select
import socket
import sys
import termios
import tty


def run() -> None:
    """Own the terminal and control socket until the private pane is closed."""
    report_path, control_path, prompt_path = sys.argv[1:]
    old = termios.tcgetattr(0)
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as report, socket.socket(
        socket.AF_UNIX, socket.SOCK_DGRAM
    ) as control:
        report.connect(report_path)
        control.bind(control_path)
        tty.setraw(0)
        captures: list[str] = []
        incoming = b""
        composer = b""
        pasting = False
        blocked = False
        pending = "ready"

        def send(kind: str, text: str = "") -> None:
            report.send(json.dumps({"kind": kind, "text": text, "count": len(captures)}).encode())

        def draw(text: str) -> None:
            os.write(1, ("\x1b[2J\x1b[H> " + text.replace("\n", "\r\n")).encode())

        try:
            os.write(1, b"\x1b[?2004h")
            draw("")
            # The terminal must parse the preceding draw before answering DSR.
            os.write(1, b"\x1b[6n")
            while True:
                readable, _, _ = select.select([0, control], [], [])
                if control in readable:
                    command = control.recv(256).decode()
                    match command:
                        case "block":
                            blocked = True
                            draw("[Pasted text #R3]")
                        case "unblock":
                            blocked = False
                            draw("")
                        case _:
                            raise AssertionError(f"unexpected control command: {command}")
                    pending = command
                    os.write(1, b"\x1b[6n")
                if 0 not in readable:
                    continue
                data = os.read(0, 65536)
                if not data:
                    return
                incoming += data
                while incoming:
                    if incoming.startswith(b"\x1b[") and not pasting:
                        reply = re.match(rb"\x1b\[\d+;\d+R", incoming)
                        if reply:
                            incoming = incoming[reply.end():]
                            if pending == "ready":
                                with open(prompt_path, encoding="utf-8") as prompt:
                                    send("ready", prompt.read())
                            else:
                                send(pending)
                            pending = ""
                            continue
                    if incoming.startswith(b"\x1b[200~"):
                        incoming = incoming[6:]
                        pasting = True
                        continue
                    if incoming.startswith(b"\x1b[201~"):
                        incoming = incoming[6:]
                        pasting = False
                        draw(composer.decode())
                        continue
                    # An escape sequence can be split across reads.
                    if incoming.startswith(b"\x1b") and (
                        b"\x1b[200~".startswith(incoming)
                        or b"\x1b[201~".startswith(incoming)
                        or (not pasting and re.fullmatch(rb"\x1b\[[\d;]*", incoming))
                    ):
                        break
                    byte, incoming = incoming[:1], incoming[1:]
                    if not pasting and byte in (b"\r", b"\n"):
                        # herdr send-text can write literal multiline input
                        # without bracketed-paste framing. The machine block's
                        # trailing newline belongs to that paste, not submission.
                        if composer.startswith(b"[corral-policy]") and not composer.endswith(
                            b"\n```\n"
                        ):
                            composer += b"\n"
                            continue
                        text = composer.decode()
                        composer = b""
                        if text.startswith("R3-FENCE-"):
                            send("fence", text)
                            draw("[Pasted text #R3]" if blocked else "")
                        elif text:
                            assert not blocked, "input submitted while receiver blocked"
                            captures.append(text)
                            os.write(1, b"\r\n> ")
                            send("policy", text)
                    else:
                        composer += byte
                if composer and not pasting:
                    draw(composer.decode())
        finally:
            termios.tcsetattr(0, termios.TCSANOW, old)


run()
