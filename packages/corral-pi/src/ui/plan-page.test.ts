import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { PlanPage, renderPlanPage } from "./plan-page.ts";

test("the Plan page renders and selects across the document, ledger and filed tasks", () => {
  const data = {
    document: { phase: "review", title: "Proposal", revision: 1 },
    steps: { stale: false, progress: { plan: "ledger", revision: 1, spec_revision: "spec", phases: [{ id: "p", label: "Implement",
      steps: [{ id: "s", label: "Write test", status: { state: "pending" }, criteria: [] }] }] } },
    board: { proposed: undefined, unavailable: undefined, tasks: [{ id: "T-1", title: "Filed task", phase: "queued", status: "Ready", depends: [], updatedAt: 0 }] },
  } satisfies Parameters<typeof renderPlanPage>[0];
  for (const [width, short] of [[140, false], [80, false], [80, true], [52, false]] as const) {
    const frame = renderPlanPage(data, undefined, width, { now: 0, focused: true, short });
    for (const line of frame.lines) expect(visibleWidth(line)).toBe(width);
    const text = stripTerminalSequences(frame.lines.join("\n"));
    expect(text).toMatch(/Plan document[\s\S]*◆ Proposal[\s\S]*Steps[\s\S]*○ Write test[\s\S]*Board tasks[\s\S]*T-1/);
  }
  const calls: string[] = [];
  const page = new PlanPage({ data: () => data, now: () => 0, short: () => false, focused: () => true, click: () => true, focus() {},
    document: () => calls.push("review"), ledger: step => calls.push(`ledger:${step}`), board: (action, id) => calls.push(`${action}:${id}`),
  });
  for (const key of ["\r", "j", "\r", "\x1b[B", "\r"]) expect(page.handleKey(key)).toBeTrue();
  expect(calls).toEqual(["review", "ledger:s", "details:T-1"]);
});

test("an assistant's Plan does not render Board tasks", () => {
  const frame = renderPlanPage({ document: undefined, steps: { progress: null, stale: false } }, undefined, 80, { now: 0, short: false, focused: true });
  expect(stripTerminalSequences(frame.lines.join("\n"))).not.toContain("Board tasks");
});
