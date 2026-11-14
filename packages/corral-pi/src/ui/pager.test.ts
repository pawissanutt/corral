import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { PagerView } from "./pager.ts";

const PAGE_DOWN = "\x1b[6~", END = "\x1b[F";
const wheel = (delta: number): TuiMouseEvent => ({ type: "wheel", button: "none", x: 4, y: 4, screenX: 4, screenY: 4,
  width: 90, height: 19, shift: false, alt: false, ctrl: false, wheelDelta: delta });
const click = (x: number, y: number): TuiMouseEvent => ({ type: "click", button: "left", x, y, screenX: x, screenY: y,
  width: 90, height: 19, shift: false, alt: false, ctrl: false, clickCount: 1 });

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

test("d shows the exact records uncut at 46 columns, each line set keeps its own place across resizes, and a clicked action cap returns its key", () => {
  const record = `{"summary":"${"word  ".repeat(30)}end"}`;
  const readable = Array.from({ length: 60 }, (_, index) => ({ text: `fact ${index + 1}`, heading: index % 10 === 0 }));
  const raw = [...Array.from({ length: 59 }, (_, index) => `raw ${index + 1}`), record];
  let rows = 24, closed: string | undefined | null = null;
  const pager = new PagerView({ title: "T-950 / R2", lines: readable, raw, actions: [{ key: "t", label: "transcript" }, { key: "d", label: "shadowed" }] },
    () => rows, () => {}, key => { closed = key; });
  const text = (width: number) => pager.render(width).map(line => stripTerminalSequences(line));
  text(90); pager.handleInput(PAGE_DOWN);
  expect(text(90)[0]).toContain("17–33 / 60");
  pager.handleInput("d");
  expect(closed).toBeNull();
  expect(text(90)[0]).toContain(" 1–17 / 62 ");
  pager.handleInput(END);
  const narrow = text(46);
  expect(narrow.slice(1, -1).map(line => line.slice(2, -2)).join("")).toContain(record);
  pager.handleInput("d");
  rows = 14;
  expect(text(46)[0]).toContain("17–25 / 60");
  rows = 24;
  expect(text(90)[0]).toContain("17–33 / 60");
  const bottom = text(90).at(-1)!;
  expect(pager.handleMouse(click(bottom.indexOf(" t ") + 1, 18))?.handled).toBeTrue();
  expect(closed).toBe("t");
});
