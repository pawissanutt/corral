import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { keyGroups } from "./keymap.ts";
import { KeysList } from "./keys-list.ts";

const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
const mode = { taskBound: true, answer: true, jobs: true, queueControl: true, lifecycle: true };
const click = (y: number): TuiMouseEvent => ({ type: "click", button: "left", x: 4, y, screenX: 4, screenY: y,
  width: 80, height: 12, shift: false, alt: false, ctrl: false });
const F5 = "\x1b[15~", F8 = "\x1b[19~";

test("Enter and a click run the row's key; Esc and F1 close without one", () => {
  const results: (string | undefined)[] = [];
  const list = () => new KeysList(keyGroups(mode), theme, data => { results.push(data); }, () => {}, 12);
  list().handleInput("\r");
  const clicked = list();
  const row = clicked.render(80).findIndex(line => stripTerminalSequences(line).includes("Agents: helper calls"));
  expect(row).toBeGreaterThan(0);
  clicked.handleMouse(click(row));
  list().handleInput("\x1b");
  list().handleInput("\x1bOP");
  expect(results).toEqual([F5, F8, undefined, undefined]);
});

test("the list stays within its rows and width and keeps the selection in view", () => {
  for (const width of [40, 80]) for (const capacity of [4, 9]) {
    const list = new KeysList(keyGroups(mode), theme, () => {}, () => {}, capacity);
    for (let step = 0; step < 60; step++) {
      const lines = list.render(width);
      expect(lines.length).toBeLessThanOrEqual(capacity + 1);
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(lines.some(line => stripTerminalSequences(line).startsWith("▌"))).toBeTrue();
      list.handleInput("\x1b[B");
    }
  }
});
