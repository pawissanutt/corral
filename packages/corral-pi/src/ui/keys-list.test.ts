import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { keyGroups } from "./keymap.ts";
import { KeysList } from "./keys-list.ts";
import { PANEL_HEAD, PANEL_RAISED } from "./panels-view.ts";

const mode = { taskBound: true, answer: true, jobs: true, queueControl: true, lifecycle: true };
const click = (x: number, y: number): TuiMouseEvent => ({ type: "click", button: "left", x, y, screenX: x, screenY: y,
  width: 80, height: 12, shift: false, alt: false, ctrl: false });
const F5 = "\x1b[15~", F8 = "\x1b[19~";
const plain = (line: string) => stripTerminalSequences(line);

test("Enter and a click run the row's key; Esc, F1 and a click on `esc close` close without one", () => {
  const results: (string | undefined)[] = [];
  const list = () => new KeysList(keyGroups(mode), data => { results.push(data); }, () => {}, 12);
  list().handleInput("\r");
  const clicked = list();
  const row = clicked.render(80).findIndex(line => plain(line).includes("Agents: helper calls"));
  expect(row).toBeGreaterThan(2);
  clicked.handleMouse(click(4, row));
  list().handleInput("\x1b");
  list().handleInput("\x1bOP");
  const closing = list();
  const title = plain(closing.render(80)[1]!);
  closing.handleMouse(click(title.indexOf("close"), 1));
  expect(results).toEqual([F5, F8, undefined, undefined, undefined]);
});

test("the popup: rounded border, a `Keys` title with `esc close`, a divider, head-coloured groups and key caps", () => {
  const lines = new KeysList(keyGroups(mode), () => {}, () => {}, 12).render(78);
  const text = lines.map(plain);
  expect(text[0]!.startsWith("╭")).toBeTrue(); expect(text[0]!.endsWith("╮")).toBeTrue();
  expect(text[1]).toMatch(/^│ Keys +esc +close │$/);
  expect(text[2]).toMatch(/^│─+│$/);
  expect(text.at(-1)!.startsWith("╰")).toBeTrue();
  expect(lines.find(line => plain(line).includes("Views"))).toContain(PANEL_HEAD);
  for (const line of lines) expect(line).toContain(PANEL_RAISED);
  expect(text.some(line => line.startsWith("│▌"))).toBeTrue();
});

test("the list stays within its rows and width and keeps the selection in view", () => {
  for (const width of [40, 78]) for (const capacity of [4, 9]) {
    const list = new KeysList(keyGroups(mode), () => {}, () => {}, capacity);
    for (let step = 0; step < 60; step++) {
      const lines = list.render(width);
      expect(lines.length).toBeLessThanOrEqual(capacity + 4);
      for (const line of lines) expect(visibleWidth(line)).toBe(width);
      expect(lines.some(line => plain(line).startsWith("│▌"))).toBeTrue();
      list.handleInput("\x1b[B");
    }
  }
});
