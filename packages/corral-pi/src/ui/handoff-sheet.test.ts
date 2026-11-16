import { expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import { HandoffSheet, implementationActions, planActions, type SheetResult } from "./handoff-sheet.ts";

const DOWN = "\x1b[B";
const sheet = (plan = false, disabled = {}) => {
  const results: SheetResult[] = [];
  const s = new HandoffSheet("Fix cache", "3 commits · 2 files", plan,
    plan ? planActions(disabled) : implementationActions(disabled), r => results.push(r));
  return { s, results };
};
const click = (y: number) => ({ type: "click", button: "left", x: 2, y, shift: false, alt: false, ctrl: false }) as never;
const rowOf = (s: HandoffSheet, label: string) => s.render(80).findIndex(l => l.includes(label));

test("focus starts on context; Enter never auto-submits the recommended action", () => {
  const { s, results } = sheet();
  s.handleInput("\r");
  expect(results).toEqual([]);
  expect(s.context).toBe("task");
});

test("keyboard and pointer reach the same stable option and context", () => {
  const kb = sheet();
  kb.s.handleInput(DOWN); kb.s.handleInput("\r"); // conversation context
  for (let i = 0; i < 3; i++) kb.s.handleInput(DOWN); // -> Take over and finish
  kb.s.handleInput("\r");
  const ptr = sheet();
  ptr.s.handleMouse(click(rowOf(ptr.s, "Conversation context")));
  ptr.s.handleMouse(click(rowOf(ptr.s, "Take over and finish")));
  expect(kb.results).toEqual([{ action: "continue_selected_goal", context: "conversation" }]);
  expect(ptr.results).toEqual(kb.results);
});

test("cancel and escape have no effect; disabled actions explain and never submit", () => {
  const esc = sheet();
  esc.s.handleInput("\x1b");
  expect(esc.results).toEqual([undefined]);
  const plan = sheet(true, { plan_review_only: "No reviewable plan revision yet" });
  plan.s.handleMouse(click(rowOf(plan.s, "Review this plan")));
  expect(plan.results).toEqual([]);
  expect(plan.s.render(80).join("\n")).toContain("No reviewable plan revision yet");
  expect(plan.s.render(80).join("\n")).toContain("Plan · No implementation");
  plan.s.handleMouse(click(rowOf(plan.s, "Cancel")));
  expect(plan.results).toEqual([undefined]);
});

test("renders within 140/80/52 columns", () => {
  for (const width of [140, 80, 52]) for (const line of sheet(true).s.render(width)) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
});

test("short height keeps the focused option visible and clicks map through the scrolled window", () => {
  const results: SheetResult[] = [];
  const s = new HandoffSheet("Fix cache", "Plan r4 · ready · Local review r3 completed · stale (current r4)", true,
    planActions(), r => results.push(r), () => {}, true, {}, () => 11);
  for (let i = 0; i < 6; i++) s.handleInput(DOWN); // -> Cancel, the last row
  let lines = s.render(52);
  expect(lines.length).toBeLessThanOrEqual(11);
  expect(lines.some(l => l.includes("> [Cancel]"))).toBe(true);
  expect(lines[0]).toContain("more");
  s.handleInput("\x1b[A"); s.handleInput("\x1b[A"); // -> Continue planning
  lines = s.render(52);
  const y = lines.findIndex(l => l.includes("[Continue planning]"));
  expect(y).toBeGreaterThan(0);
  expect(lines[y + 1]).toContain("Stops at a reviewed plan");
  s.handleMouse(click(y));
  expect(results).toEqual([{ action: "plan_continue", context: "task" }]);
});
