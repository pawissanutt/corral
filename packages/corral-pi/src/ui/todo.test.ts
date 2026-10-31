import { expect, test } from "bun:test";
import { visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";

import { snapshotProgress } from "./todo.ts";
import { ProgressSchema, TodoWidget } from "./todo-widget.ts";

const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
const progress = ProgressSchema.parse({
  revision: 4, spec_revision: "s1",
  phases: [{ id: "build", label: "Implementation", steps: [
    { id: "read", label: "Read existing UI", status: { state: "done" } },
    { id: "widget", label: "Build expandable widget", status: { state: "in_progress" }, criteria: ["A1"] },
    { id: "test", label: "Test keyboard and mouse", status: { state: "pending" } },
    { id: "extra", label: "Extra check", status: { state: "dropped", reason: "Covered by targeted checks" } },
  ] }],
});
const click: TuiMouseEvent = {
  type: "click", button: "left", x: 1, y: 0, screenX: 1, screenY: 0,
  width: 80, height: 12, shift: false, alt: false, ctrl: false,
};

test("folded ledger shows the active symbol and never counts drops as completed", () => {
  // Given confirmed progress with all four statuses.
  const widget = new TodoWidget(theme, () => 40, () => {});
  widget.update(progress);
  // When rendered in its default folded state.
  const lines = widget.render(80);
  // Then only the summary is shown and the counts remain honest.
  expect(lines).toHaveLength(1);
  expect(lines[0]).toContain("✓ 1/4");
  expect(lines[0]).toContain("– 1");
  expect(lines[0]).toContain("● Build expandable widget");
  expect(lines[0]).not.toContain("Read existing UI");
});

test("click expands full details and updates preserve the user's disclosure", () => {
  // Given a folded widget.
  const widget = new TodoWidget(theme, () => 80, () => {});
  widget.update(progress);
  // When the owner clicks the header and a new receipt arrives.
  expect(widget.handleMouse(click)?.handled).toBeTrue();
  widget.update({ ...progress, revision: 5 });
  // Then status symbols, IDs, criteria and drop reasons are visible.
  const text = widget.render(80).join("\n");
  for (const part of ["▾ Todo", "✓ Read existing UI", "● Build expandable widget", "○ Test keyboard",
    "– Extra check", "[widget]", "A1", "Covered by targeted checks"]) expect(text).toContain(part);
});

test("body clicks and presses do not toggle or change the ledger", () => {
  // Given the folded widget.
  const widget = new TodoWidget(theme, () => 40, () => {});
  widget.update(progress);
  // When terminal press/release events or body clicks arrive.
  widget.handleMouse({ ...click, type: "press" });
  widget.handleMouse({ ...click, type: "release" });
  widget.handleMouse({ ...click, y: 1 });
  // Then only one line still renders.
  expect(widget.render(80)).toHaveLength(1);
});

test("wheel scrolling reaches long-list details within a bounded height", () => {
  // Given an expanded list in a short terminal.
  const widget = new TodoWidget(theme, () => 18, () => {});
  widget.update(progress);
  widget.toggle();
  const first = widget.render(40).join("\n");
  // When the pointer scrolls down.
  widget.handleMouse({ ...click, type: "wheel", y: 3, wheelDelta: 100 });
  // Then the tail and reason are reachable without growing the widget.
  const lines = widget.render(40);
  expect(lines.length).toBeLessThanOrEqual(8);
  expect(lines.join("\n")).not.toBe(first);
  expect(lines.join("\n")).toContain("Covered by targeted checks");
});

test("keyboard toggle and scrolling reach the same long-list tail", () => {
  // Given the same short terminal and ledger.
  const widget = new TodoWidget(theme, () => 18, () => {});
  widget.update(progress);
  // When the command actions expand and scroll.
  widget.toggle();
  widget.render(40);
  widget.scroll(100);
  // Then details are reachable, and folding restores a single line.
  expect(widget.render(40).join("\n")).toContain("Covered by targeted checks");
  widget.toggle();
  expect(widget.render(40)).toHaveLength(1);
});

test("receipt replay cannot overwrite newer progress and failed reads retain it", () => {
  // Given a confirmed revision.
  const widget = new TodoWidget(theme, () => 40, () => {});
  widget.update(progress);
  // When an older receipt arrives and a refresh fails.
  widget.update({ ...progress, revision: 1, phases: [] });
  widget.failed();
  // Then the last confirmed ledger remains visible as stale.
  expect(widget.render(80)[0]).toContain("✓ 1/4");
  expect(widget.render(80)[0]).toContain("stale");
});

test("no ledger is hidden, while an unavailable initial read is explicit", () => {
  // Given an uninitialized widget.
  const widget = new TodoWidget(theme, () => 40, () => {});
  expect(widget.render(80)).toEqual([]);
  // When its initial read fails.
  widget.failed();
  // Then there is an unavailable indicator rather than an empty checklist.
  expect(widget.render(80)[0]).toContain("unavailable");
});

test("narrow rendering wraps wide labels and neutralizes terminal controls", () => {
  // Given long CJK and control-bearing labels.
  const widget = new TodoWidget(theme, () => 80, () => {});
  widget.update({ ...progress, phases: [{ id: "p", label: "Implementation", steps: [{
    id: "wide", label: "確認".repeat(18) + "\u001b[31m\nunsafe", criteria: [],
    status: { state: "in_progress" },
  }] }] });
  widget.toggle();
  // When rendered in a split pane.
  const lines = widget.render(30);
  // Then all visible cells fit and no data can emit terminal escapes.
  for (const line of lines) {
    expect(visibleWidth(line)).toBeLessThanOrEqual(30);
    expect(line).not.toContain("\u001b");
  }
});

test("worker restore selects the bound run, not the latest run", () => {
  // Given historical and current runs with different ledgers.
  const snapshot = { runs: [{ id: "R1", progress }, { id: "R2", progress: null }] };
  // When resuming the earlier bound run.
  const restored = snapshotProgress(snapshot, { store: "s", project: "p", role: "worker",
    task: "T-1", run: "R1", generation: 2 });
  // Then the original ledger is restored.
  expect(restored).toEqual(progress);
});

test("PM restores its own ledger and malformed snapshots are refused", () => {
  // Given the PM container snapshot.
  const identity = { store: "s", project: "p", role: "pm", generation: 2 } as const;
  // When the PM ledger is restored.
  expect(snapshotProgress({ container: { todo: progress } }, identity)).toEqual(progress);
  // Then an invalid status is not accepted as canonical progress.
  expect(() => snapshotProgress({ container: { todo: { ...progress, phases: "broken" } } }, identity)).toThrow();
});
