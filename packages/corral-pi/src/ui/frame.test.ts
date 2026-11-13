import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { card, countChip, keyCap, rule, spinner, titledBox } from "./frame.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_DIALOG, PANEL_KEY, PANEL_LINE, PANEL_ON_ACCENT, PANEL_RAISED, PANEL_SELECTED, PANEL_TEXT } from "./panels-view.ts";

const plain = (line: string) => stripTerminalSequences(line);
const WIDTHS = [185, 140, 105, 92, 80, 61, 52, 46, 12, 4];

test("key caps and count chips pad one cell each side and end on the canvas", () => {
  const cap = keyCap("F9");
  expect(plain(cap)).toBe(" F9 ");
  expect(cap.startsWith(PANEL_KEY + PANEL_BLUE)).toBeTrue();
  expect(cap.endsWith(PANEL_BG + PANEL_TEXT)).toBeTrue();
  const on = keyCap("F9", true);
  expect(on.startsWith(PANEL_ON_ACCENT)).toBeTrue();
  expect(on.endsWith(PANEL_BG + PANEL_TEXT)).toBeTrue();
  expect(keyCap("e", false, PANEL_RAISED).endsWith(PANEL_RAISED + PANEL_TEXT)).toBeTrue();
  const chip = countChip("3");
  expect(plain(chip)).toBe(" 3 ");
  expect(chip.startsWith(PANEL_SELECTED)).toBeTrue();
  expect(chip.endsWith(PANEL_TEXT + PANEL_BG)).toBeTrue();
});

test("the spinner steps through the braille frames and staggers by phase", () => {
  const frames = new Set(Array.from({ length: 16 }, (_, i) => spinner(i * 110)));
  expect(frames.size).toBe(8);
  expect(spinner(0, 1)).toBe(spinner(110));
  expect(spinner(-5)).toBeDefined();
  expect(spinner(Number.NaN)).toBe(spinner(0));
});

test("a card is exactly its width with rounded corners, a raised header and fitted body rows", () => {
  for (const width of WIDTHS) {
    const lines = card("task · 4 slices", ["host-slots   Host slot endpoints " + "x".repeat(200), "short"], width, { right: "40s" });
    expect(lines).toHaveLength(5);
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    expect(plain(lines[0]!).startsWith("╭")).toBeTrue();
    expect(plain(lines[0]!).endsWith("╮")).toBeTrue();
    expect(plain(lines[4]!).startsWith("╰")).toBeTrue();
    expect(plain(lines[1]!).startsWith("│ ")).toBeTrue();
    expect(lines[1]).toContain(PANEL_RAISED);
    expect(plain(lines[2]!).endsWith(" │")).toBeTrue();
  }
  const wide = card("task", ["a"], 40, { right: "40s" });
  expect(plain(wide[1]!).trimEnd().endsWith("40s │")).toBeTrue();
  expect(wide[0]).toContain(PANEL_LINE);
  expect(card("x", [], 3)).toEqual([]);
});

test("an accent card is a dialog; the selected row carries the bar and the selected surface", () => {
  const lines = card("picker-ui asks", ["1  Pin", "   continued", "2  Grid"], 50, { tone: "accent", selected: { from: 0, to: 1 } });
  expect(lines[0]).toContain(PANEL_BLUE);
  expect(lines[1]).toContain(PANEL_DIALOG);
  expect(plain(lines[2]!).slice(0, 2)).toBe("│▌");
  expect(lines[2]).toContain(PANEL_SELECTED);
  expect(plain(lines[3]!).slice(0, 2)).toBe("│ ");
  expect(lines[3]).toContain(PANEL_SELECTED);
  expect(lines[4]).not.toContain(PANEL_SELECTED);
  const single = card("x", ["a", "b"], 20, { selected: 1 });
  expect(plain(single[3]!).slice(0, 2)).toBe("│▌");
  expect(plain(single[2]!).slice(0, 2)).toBe("│ ");
});

test("a titled box sets its title into the top border at every width", () => {
  for (const width of WIDTHS) {
    const lines = titledBox("Message → parent", ["› draft", "gpt-6.1-sol ▾ · high ↻"], width, { tone: "accent" });
    expect(lines).toHaveLength(4);
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    expect(plain(lines[0]!).startsWith("╭")).toBeTrue();
    expect(plain(lines[0]!).endsWith("╮")).toBeTrue();
    expect(plain(lines[3]!).startsWith("╰")).toBeTrue();
  }
  const top = plain(titledBox("Message → parent", [], 40)[0]!);
  expect(top.startsWith("╭─ Message → parent ─")).toBeTrue();
  expect(titledBox("Message", [], 40, { tone: "accent" })[0]).toContain(PANEL_BLUE);
});

test("a rule is the width in the line colour", () => {
  for (const width of WIDTHS) {
    const line = rule(width);
    expect(plain(line)).toBe("─".repeat(width));
    expect(line).toContain(PANEL_LINE);
  }
});
