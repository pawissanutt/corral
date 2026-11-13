import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { PagerView } from "./pager.ts";

const PAGE_DOWN = "\x1b[6~", END = "\x1b[F";
const wheel = (delta: number): TuiMouseEvent => ({ type: "wheel", button: "none", x: 4, y: 4, screenX: 4, screenY: 4,
  width: 90, height: 19, shift: false, alt: false, ctrl: false, wheelDelta: delta });

test("a 300-line pager at 24 rows shows its position after PgDn, End and the wheel, inside its frame", () => {
  const lines = Array.from({ length: 300 }, (_, index) => `${String(index + 1).padStart(3)} │ ${index === 4 ? "x".repeat(400) : "line"}`);
  const pager = new PagerView({ title: "Job j1 · retained output", lines }, () => 24, () => {}, () => {});
  const top = (width: number) => stripTerminalSequences(pager.render(width)[0]!);
  expect(top(90)).toContain("1–17 / 300");
  pager.handleInput(PAGE_DOWN);
  expect(top(90)).toContain("17–33 / 300");
  pager.handleInput(END);
  expect(top(90)).toContain("284–300 / 300");
  expect(pager.handleMouse(wheel(-3))?.handled).toBeTrue();
  expect(top(90)).toContain("281–297 / 300");
  pager.handleInput("\x1b[H");
  for (const width of [90, 41]) {
    const frame = pager.render(width);
    expect(frame.length).toBe(19);
    for (const line of frame) expect(visibleWidth(line)).toBe(width);
    expect(stripTerminalSequences(frame[5]!)).toContain("…");
  }
});
