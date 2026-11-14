import { expect, test } from "bun:test";
import type { BashExecutionState } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";

import type { Ticker } from "./activity.ts";
import { ownerBashPresenter } from "./owner-bash.ts";

const T0 = 1_700_000_000_000;
const SPINNING = /[⠋⠙⠹⠸⠼⠴⠦⠧●]/u;
// The owner's label ends the row line: `you` (or `you · not sent to the agent`), then an optional line count and an
// optional duration; or it is the fallback row. A command or outcome saying `you` mid-line is neither.
const COUNT = String.raw`(?:\s+\d+ (?:more )?lines?)?`;
const DURATION = String.raw`(?:\s+(?:\d+h(?: \d+m)?|\d+m(?: \d+s)?|\d+(?:\.\d)?m?s))?`;
const OWNER = new RegExp(String.raw`(?:(?<!by )\byou(?: · not sent to the agent)?${COUNT}${DURATION}\s*$|^\s*you ran this(?: · not sent to the agent)?\s*$)`, "mu");

function harness() {
  const held = new Set<string>();
  const clock = { now: T0 + 5_000 };
  const ticker: Ticker = { now: () => clock.now, hold: id => { held.add(id); return () => { held.delete(id); }; } };
  const present = ownerBashPresenter({ cwd: "/work", ticker: () => ticker });
  const draw = (state: BashExecutionState, width = 101): string[] => (present(state, width) ?? []).map(line => stripTerminalSequences(line));
  return { held, clock, draw };
}
function block(over: Partial<BashExecutionState> = {}): BashExecutionState {
  return { command: "make", excludeFromContext: false, status: "complete", exitCode: 0, lines: [], expanded: false, truncated: false, ...over };
}

test("every state stays inside the width at 44, 78 and 101 columns, controls and all", () => {
  const states = [
    block({ status: "running", exitCode: undefined, startedAt: T0, lines: ["compiling \u001b[2J crate", "tab\there", "\u0007bell"] }),
    block({ exitCode: 0, lines: ["done"], startedAt: T0, endedAt: T0 + 1_500 }),
    block({ status: "error", exitCode: 2, lines: ["a", "b", "c", "d"], excludeFromContext: true, command: "cargo test -p a-very-long-package-name --features everything" }),
    block({ status: "cancelled", exitCode: undefined, lines: ["partial"] }),
    block({ exitCode: undefined, lines: ["x"] }),
    block({ truncated: true, fullOutputPath: "/tmp/pi-bash-0123456789abcdef0123456789abcdef.log", lines: ["tail"], expanded: true }),
    block({ command: "echo thank you for all the fish in the sea and more of them", lines: ["ok"] }),
  ];
  for (const width of [44, 78, 101]) {
    const { draw } = harness();
    for (const state of states) {
      const lines = draw(state, width);
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(lines.join("\n")).toMatch(OWNER);
    }
  }
});

test("an exit code of 0 and no exit code at all read differently", () => {
  const { draw } = harness();
  const ok = draw(block())[1]!, unknown = draw(block({ exitCode: undefined }))[1]!;
  expect(ok).toContain("✓");
  expect(ok).toContain("exit 0");
  expect(unknown).not.toContain("✓");
  expect(unknown).toContain("no exit code");
  expect(unknown).not.toContain("exit 0");
});

test("`!!` says its output is not sent to the agent even where the row has no room for the label", () => {
  const { draw } = harness();
  const state = block({ excludeFromContext: true, command: "cargo test -p corral-pi --lib some_long_test_name", lines: ["ok"] });
  for (const width of [44, 101]) expect(draw(state, width).join("\n")).toContain("not sent to the agent");
  expect(draw(block({ lines: ["ok"] }), 44).join("\n")).not.toContain("not sent");
});

test("the expanded block holds the whole command, every kept output line and the exact saved path", () => {
  const { draw } = harness();
  const lines = Array.from({ length: 30 }, (_, index) => `line ${index + 1}`);
  const path = "/tmp/pi-bash-abc123.log";
  const text = draw(block({ command: "echo one\necho two", lines, truncated: true, fullOutputPath: path, expanded: true }), 78).join("\n");
  for (const wanted of ["echo one", "echo two", "line 1", "line 30", "output cut", path]) expect(text).toContain(wanted);
  const collapsed = draw(block({ command: "echo one\necho two", lines, truncated: true, fullOutputPath: path }), 78).join("\n");
  expect(collapsed).toContain("output cut · full output saved");
  expect(collapsed).not.toContain("line 1");
});

test("a running block holds the shared clock once and releases it when it settles; replay shows no time", () => {
  const { held, clock, draw } = harness();
  const running = block({ status: "running", exitCode: undefined, startedAt: T0, lines: ["a", "b", "c"] });
  const first = draw(running);
  expect(first[1]!).toMatch(SPINNING);
  expect(first[1]!).toContain("5s");
  const rows = first.map(line => line.trim());
  expect(rows).toContain("c");
  expect(rows).toContain("b");
  expect(rows).not.toContain("a");
  draw({ ...running, lines: ["a", "b", "c", "d"] });
  expect(held.size).toBe(1);
  draw(block({ startedAt: T0, endedAt: T0 + 6_000 }));
  expect(held.size).toBe(0);
  clock.now = T0 + 6_000;
  expect(draw(block())[1]!.trim().endsWith("you")).toBeTrue();
});

test("a hold whose block is no longer drawn is released by the next draw of any block", () => {
  const { held, clock, draw } = harness();
  draw(block({ status: "running", exitCode: undefined, startedAt: T0 }));
  expect(held.size).toBe(1);
  clock.now += 11_000;
  draw(block({ command: "other" }));
  expect(held.size).toBe(0);
});
