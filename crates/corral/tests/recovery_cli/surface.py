"""Actual PTY keyboard and SGR mouse journeys; raw terminal streams are retained."""
import re

from fixtures import BIN, save
from terminal import Terminal


class Board:
    def __init__(self, fixture, mouse):
        self.f, self.mouse = fixture, mouse
        self.t = Terminal([BIN, "--root", str(fixture.root), "board"], fixture.root,
                          fixture.ev / ("mouse" if mouse else "keyboard"), framed=True)
        t = self.t
        t.contains("Skip tour")
        t.click("Skip tour") if mouse else t.key("esc")
        t.wait(lambda: "Skip tour" not in t.screen.text(), "welcome dismissed")
        t.contains(fixture.id)
        if mouse:
            t.click(fixture.id + " P2")
            t.click(fixture.id + " P2")
            t.contains("Overview")
            t.click("plan / children")
        else:
            t.key("/")
            t.text(fixture.id)
            t.contains(f"Recovery {fixture.root.name}")
            t.key("enter")
            t.key("8")
        t.contains("Run plan review")
        t.capture("plan-80x24")

    def focus(self, label):
        target = self.controls.index(label)
        count = (target - self.controls.index(self.focused)) % len(self.controls)
        if count:
            self.t.send(b"\t" * count, f"tab to {label}")
        marker = r"▸\s+" + (r"\[ \] " if label.endswith("policy") or label.endswith("binding") else "")
        self.t.wait(lambda: re.search(marker + re.escape(label), self.t.screen.text()), f"focus {label}")
        self.focused = label

    def find_mouse(self, text):
        t = self.t
        if text in t.screen.text():
            return
        body = lambda: tuple(t.screen.text().splitlines()[8:21])
        def step(down):
            before = body()
            t.wheel(down)
            t.wait(lambda: body() != before, "wheel changed recovery body")
        # First reach the known top, then observe each actual body movement. Never locate a
        # transient click target while a burst of scroll events is still queued.
        for _ in range(40):
            if "Target " + self.f.id in t.screen.text():
                break
            step(False)
        for _ in range(40):
            if text in t.screen.text():
                return
            step(True)
        raise AssertionError(f"mouse control unavailable: {text}")

    def click(self, text):
        self.find_mouse(text)
        self.t.wait(lambda: "Saving/syncing" not in self.t.screen.text().splitlines()[-1], "mouse target model idle")
        self.find_mouse(text)
        self.t.click(text)

    def field(self, label, value):
        if self.mouse:
            self.click(label.ljust(10))
        else:
            self.focus(label)
        typed = ""
        for char in value:
            self.t.text(char)
            typed += char
            tail = typed[-16:]
            visible = lambda: any(label.ljust(10) in line for line in self.t.screen.text().splitlines())
            echoed = lambda: any(label in line and tail in line for line in self.t.screen.text().splitlines())
            self.t.wait(lambda: echoed() or (self.mouse and not visible()), f"echo or reflow {label}")
            if self.mouse and not visible():
                # Header/footer reflow can reduce the body after a background update. A mouse
                # user scrolls the same focused field back into view; retain the exact echo check.
                self.find_mouse(label.ljust(10))
            self.t.wait(echoed, f"echo {label}")

    def open_recovery(self):
        if self.mouse:
            label = "Recover…" if "Recover…" in self.t.screen.text() else "recover…"
            self.t.click(label)
        else:
            self.t.key("F")

    def recover(self, accept=False):
        t = self.t
        self.open_recovery()
        t.contains(f"Recover {self.f.id}")
        t.contains("[Continue…]")
        self.controls = ["Action", *self.f.codes, "notes", "reason", "cancel", "continue"] if accept else ["Action", "independent-plan-review", "notes", "reason", "cancel", "continue"]
        self.focused = "cancel"
        t.capture("accept-open" if accept else "review-open")
        if not accept:
            # Both paths first demonstrate cancel is nonmutating.
            before = self.f.task()
            if self.mouse:
                t.click("[Cancel]")
            else:
                t.key("esc")
            t.wait(lambda: f"Recover {self.f.id}" not in t.screen.text(), "cancelled sheet closed")
            after = self.f.task()
            save(self.f.ev / "cancel-before.json", before)
            save(self.f.ev / "cancel-after.json", after)
            assert after == before, "task changed across cancel; inspect retained before/after and events"
            self.open_recovery()
            t.contains("[Continue…]")
            if self.mouse:
                self.click("use-reviewer")
                t.contains("[use-reviewer]")
                self.click("gpt-astra")
                t.wait(lambda: re.search(r"▸\s+reviewer", t.screen.text()), "reviewer mouse focus")
            else:
                self.focus("Action")
                t.key("left")  # accept-plan -> use-reviewer
                t.contains("[use-reviewer]")
                self.controls = ["Action", "stage", "reviewer", *self.f.codes, "reason", "cancel", "continue"]
                self.focused = "Action"
                self.focus("reviewer")
                # gpt-astra is the first alphabetic configured fixture profile.
                t.contains("[gpt-astra]")
        for code in self.f.codes:
            if self.mouse:
                self.click("[ ] " + code)
            else:
                self.focus(code)
                t.key("space")
            t.contains("[x] " + code)
        if accept:
            self.field("notes", "Accept the exact inert fixture plan after its genuine callback.")
        self.field("reason", "One exact disposable fixture exception")
        t.capture("accept-fields-80x24" if accept else "review-fields-80x24")
        if self.mouse:
            t.click("[Continue…]")
        else:
            # Reason -> Cancel -> Continue. Enter in the text field itself never applies.
            before = self.f.history()
            t.key("enter")
            assert self.f.history() == before
            t.key("tab")
            t.key("enter")
        t.contains("Confirm owner recovery")
        t.capture("accept-confirm-80x24" if accept else "review-confirm-80x24")
        self.wide("Confirm owner recovery")
        t.capture("accept-confirm-120x40" if accept else "review-confirm-120x40")
        before = len(self.f.history())
        def applied():
            records = self.f.history()
            return len(records) == before + 1 and records[-1]["outcomes"][-1]["state"] == "applied"
        for confirmation in range(2):
            if self.mouse:
                t.click("[Confirm recovery]")
            else:
                # Confirmation defaults to Cancel; Tab explicitly selects Confirm.
                t.key("tab")
                t.key("enter")
            t.wait(lambda: applied() or "[Continue…]" in t.screen.text(), "applied or stale-refreshed recovery")
            if applied():
                break
            assert "changed" in t.screen.text() and "proposal" in t.screen.text(), t.screen.text()
            assert len(self.f.history()) == before, "stale confirmation persisted an operation"
            t.capture("stale-confirmation")
            # Real fixture-owner interaction with the refreshed proposal, not a replay of the
            # old confirmation. The reason stays, but Continue and Confirm are explicit again.
            if self.mouse:
                t.click("[Continue…]")
            else:
                t.key("tab")
                t.key("enter")
            t.contains("Confirm owner recovery")
            t.capture("fresh-confirmation")
        assert applied(), "recovery did not apply after fresh explicit confirmation"
        t.wait(lambda: f"Recover {self.f.id}" not in t.screen.text(), "applied sheet closed")
        t.capture("accept-applied" if accept else "review-applied")
        t.resize(80, 24)
        t.wait(lambda: "Recover…" in t.screen.text() or "recover…" in t.screen.text(), "recovery entry remains reachable")

    def ordinary(self):
        t, f = self.t, self.f
        t.click("Run plan review") if self.mouse else t.key("R")
        f.callback("PR1")
        t.contains("Approve plan")
        t.capture("ordinary-callback-80x24")
        t.click("Approve plan") if self.mouse else t.key("a")
        t.contains("approve plan P1")
        self.wide("approve plan P1")
        t.capture("ordinary-owner-120x40")
        if self.mouse:
            t.click("[Approve]")
        else:
            t.key("enter")
        f.watch.until(lambda: f.task()["plans"][-1].get("approval") is not None, "ordinary owner acceptance")
        t.pump(0.2)
        t.capture("ordinary-applied")

    def wide(self, label):
        self.t.resize(120, 40)
        self.t.wait(lambda: any(label in line and line.index(label) > 5
                               for line in self.t.screen.text().splitlines()), "fresh wide popup layout")

    def close(self):
        self.t.close()


def surface(f, mouse, recovered):
    board = Board(f, mouse)
    try:
        if recovered:
            board.recover()
            f.callback("PR1")
            board.t.wait(lambda: "Recover…" in board.t.screen.text() or "recover…" in board.t.screen.text(), "acceptance recovery entry")
            board.recover(accept=True)
        else:
            board.ordinary()
        f.finish(recovered)
    finally:
        board.close()
