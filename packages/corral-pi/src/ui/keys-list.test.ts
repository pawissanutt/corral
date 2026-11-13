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
  expect(text[1]).toMatch(/^│ Keys +1–12 \/ \d+ +esc +close │$/);
  expect(text[2]).toMatch(/^│─+│$/);
  expect(text.at(-1)!.startsWith("╰")).toBeTrue();
  expect(lines.find(line => plain(line).includes("Views"))).toContain(PANEL_HEAD);
  for (const line of lines) expect(line).toContain(PANEL_RAISED);
  expect(text.some(line => line.startsWith("│▌"))).toBeTrue();
});

test("the list stays within its rows and width; a drawn selection is always a runnable row", () => {
  for (const width of [40, 78]) for (const capacity of [4, 9]) {
    const results: (string | undefined)[] = [];
    const list = new KeysList(keyGroups(mode), data => { results.push(data); }, () => {}, capacity);
    for (let step = 0; step < 60; step++) {
      const lines = list.render(width);
      expect(lines.length).toBeLessThanOrEqual(capacity + 4);
      for (const line of lines) expect(visibleWidth(line)).toBe(width);
      if (lines.some(line => plain(line).startsWith("│▌"))) {
        list.handleInput("\r");
        expect(typeof results.at(-1)).toBe("string");
      }
      list.handleInput("\x1b[B");
    }
  }
});

// 68 lines: runnable groups between reference-only groups, as in a task-bound session.
const fixture = ([["Views", 6, true], ["Agents", 15, false], ["Changes", 3, true], ["Task", 20, false], ["Anywhere", 10, false], ["Queue", 8, true]] as const)
  .map(([title, count, runs]) => ({ title, rows: Array.from({ length: count }, (_, index) => ({ key: "k", label: `${title} ${index + 1}`,
    ...(runs ? { data: `run ${title} ${index + 1}` } : {}) })) }));
const titles = fixture.flatMap(group => [group.title, ...group.rows.map(row => row.label)]);
const label = (line: string) => plain(line).slice(1, -1).replace(/^▌/, "").trim().replace(/^k\s+/, "");

test("F1 at 24 rows brings every one of 68 lines on screen with ↓ or PgDn, and Enter runs the selected row", () => {
  expect(titles.length).toBe(68);
  for (const key of ["\x1b[B", "\x1b[6~"]) {
    const results: (string | undefined)[] = [];
    const list = new KeysList(fixture, data => { results.push(data); }, () => {}, 16);
    const seen = new Set<string>();
    let lines = list.render(78);
    for (let press = 0; press < 80; press++) {
      for (const line of lines.slice(3, -1)) seen.add(label(line));
      list.handleInput(key); lines = list.render(78);
    }
    expect(titles.filter(title => !seen.has(title))).toEqual([]);
    expect(plain(lines[1]!)).toContain("53–68 / 68");
    const bar = lines.find(line => plain(line).startsWith("│▌"));
    expect(bar === undefined).toBeFalse();
    list.handleInput("\r");
    expect(results).toEqual([`run ${label(bar ?? "")}`]);
  }
  const wheel = new KeysList(fixture, () => {}, () => {}, 16);
  wheel.handleMouse({ type: "wheel", button: "none", x: 4, y: 6, screenX: 4, screenY: 6, width: 78, height: 20,
    shift: false, alt: false, ctrl: false, wheelDelta: 5 });
  expect(plain(wheel.render(78)[1]!)).toContain("6–21 / 68");
});
