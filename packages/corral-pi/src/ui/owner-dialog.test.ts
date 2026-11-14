import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { TUI, TuiMouseEvent } from "@earendil-works/pi-tui";
import type { PresentedDialog, SelectDialogRequest } from "@earendil-works/pi-coding-agent";
import { CHOICE_GUARD_MS } from "./build/choice-dock.ts";
import { ownerDialogPresentation, ownerListCard } from "./owner-dialog.ts";
import type { OwnerDialogHost, OwnerListItem } from "./owner-dialog.ts";

const ENTER = "\r", ESC = "\x1b", DOWN = "\x1b[B", PAGE_DOWN = "\x1b[6~", CTRL_O = "\x0f";
const tui = { requestRender() {} } as unknown as TUI;
const host = (rows: number): OwnerDialogHost => ({ rows: () => rows, chromeRows: () => Math.floor(rows / 3), changed() {} });
/** Runs `keys` with the clock past the guard of every card opened before it, without waiting. */
function armed(keys: () => void): void {
  const real = Date.now;
  Date.now = () => real() + CHOICE_GUARD_MS;
  try { keys(); } finally { Date.now = real; }
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const click = (x: number, y: number): TuiMouseEvent =>
  ({ type: "click", button: "left", x, y, screenX: x, screenY: y, width: 80, height: 12, shift: false, alt: false, ctrl: false });

function select(request: Partial<SelectDialogRequest> & Pick<SelectDialogRequest, "options">, rows = 24):
  { readonly card: PresentedDialog; readonly answers: (string | undefined)[] } {
  const answers: (string | undefined)[] = [];
  const card = ownerDialogPresentation(host(rows)).select?.(
    { kind: "select", title: "Pick one", tui, toggleToolsExpanded() {}, ...request }, value => { answers.push(value); });
  if (!card) throw new Error("the select hook declined");
  return { card, answers };
}

test("Enter and digits wait out the guard; then a digit answers with the exact offered label, once", () => {
  const options = ["Allow once", "Allow for 1 hour", "Deny"];
  const { card, answers } = select({ options });
  card.handleInput(ENTER); card.handleInput("1");
  expect(answers).toEqual([]);
  armed(() => {
    // A digit that names no option answers nothing.
    for (const key of ["7", "0"]) card.handleInput(key);
    expect(answers).toEqual([]);
    card.handleInput("3"); card.handleInput("1"); card.handleInput(ENTER);
  });
  expect(answers).toEqual(["Deny"]);
});

test("Esc answers undefined at once, even inside the guard; a confirm starts on its first option", () => {
  const early = select({ kind: "confirm", title: "Stop command?", message: "Output is retained.", options: ["Yes", "No"] });
  early.card.handleInput(ESC); early.card.handleInput(ESC); early.card.handleInput(ENTER);
  expect(early.answers).toEqual([undefined]);
  const late = select({ kind: "confirm", title: "Stop command?", message: "Output is retained.", options: ["Yes", "No"] });
  armed(() => { late.card.handleInput(DOWN); late.card.handleInput(ENTER); });
  expect(late.answers).toEqual(["No"]);
});

test("an input answers what was typed, never its placeholder", () => {
  const answers: (string | undefined)[] = [];
  const open = () => ownerDialogPresentation(host(24)).input?.({ title: "Filter changed paths", placeholder: "src/ui", tui }, value => { answers.push(value); });
  const empty = open(), typed = open();
  empty?.handleInput(ENTER);
  expect(answers).toEqual([]);
  armed(() => {
    empty?.handleInput(ENTER);
    for (const key of "app") typed?.handleInput(key);
    typed?.handleInput(ENTER);
  });
  expect(answers).toEqual(["", "app"]);
});

test("a long list filters by typing: digits are text, Enter answers the original label; Esc while filtered answers undefined at once, never a match", () => {
  const options = Array.from({ length: 14 }, (_, i) => `Q-${i + 1} · Ship slice ${i + 1}?`);
  const { card, answers } = select({ options });
  armed(() => {
    card.handleInput("1"); card.handleInput("3");
    expect(answers).toEqual([]);
    card.handleInput(ENTER);
  });
  expect(answers).toEqual([options[12]]);
  const other = select({ options });
  armed(() => {
    other.card.handleInput("1"); other.card.handleInput("3");
    other.card.handleInput(ESC); other.card.handleInput(ENTER);
  });
  expect(other.answers).toEqual([undefined]);
});

test("at 46 to 138 columns and 14 to 28 rows every row fits, the countdown stays in the header, and every option and line is reachable", () => {
  const options = ["Allow once", "Allow for 1 hour", "Allow for this session", "Deny"];
  const text = Array.from({ length: 40 }, (_, i) => `+ preview line ${i + 1}`).join("\n");
  for (const width of [46, 80, 105, 138]) for (const rows of [14, 24, 28]) {
    const { card } = select({ title: `Allow edit to ~/proj/src/app.ts?\n${text}`, options, timeoutMs: 300_000 }, rows);
    card.setCountdown?.(299);
    const seen = new Set<string>();
    const look = () => {
      const lines = card.render(width);
      for (const line of lines) expect(visibleWidth(line)).toBe(width);
      const shown = plain(lines);
      const clocks = shown.filter(line => line.includes("4:59"));
      expect(clocks).toHaveLength(1);
      expect(clocks[0]).toContain("Allow");
      for (const line of shown) seen.add(line.replace(/[│▌]/g, "").trim());
    };
    for (let step = 0; step < options.length; step++) { look(); card.handleInput(DOWN); }
    for (const option of options) expect([...seen].some(line => line.endsWith(option))).toBeTrue();
    card.handleInput(CTRL_O);
    for (let page = 0; page < 40; page++) { look(); card.handleInput(PAGE_DOWN); }
    for (const line of [1, 40]) expect(seen.has(`+ preview line ${line}`)).toBeTrue();
  }
});

test("at 44 columns and 14 rows a selected option longer than the card is readable in full before any answer, then answers its exact label", () => {
  const long = `~/HEADMARK/${"deep/".repeat(40)} → /real/TAILMARK`;
  const { card, answers } = select({ title: "Directory scope", options: [long, "~/proj"] }, 14);
  const seen: string[] = [];
  for (let page = 0; page < 12; page++) { seen.push(...plain(card.render(44))); card.handleInput(PAGE_DOWN); }
  expect(seen.some(line => line.includes("HEADMARK"))).toBeTrue();
  expect(seen.some(line => line.includes("TAILMARK"))).toBeTrue();
  expect(answers).toEqual([]);
  armed(() => { card.handleInput(ENTER); });
  expect(answers).toEqual([long]);
});

test("a click on a hint replays its key; a click on the selected option chooses it only after the guard", () => {
  const { card, answers } = select({ options: ["Allow once", "Deny"] });
  let shown = plain(card.render(80));
  const deny = shown.findIndex(line => line.includes("Deny"));
  card.handleMouse?.(click(6, deny));
  card.handleMouse?.(click(6, deny));
  expect(answers).toEqual([]);
  armed(() => { card.handleMouse?.(click(6, deny)); });
  expect(answers).toEqual(["Deny"]);
  const other = select({ options: ["Allow once", "Deny"] });
  shown = plain(other.card.render(80));
  const hints = shown.length - 1;
  other.card.handleMouse?.(click(shown[hints]!.indexOf("esc") + 1, hints));
  expect(other.answers).toEqual([undefined]);
});

test("the list card never chooses a disabled item and hands its extra keys the selected value", () => {
  const items: OwnerListItem<string>[] = ["alpha", "beta", "gamma"].map(value =>
    ({ value, match: value, lines: () => [value], ...(value === "beta" ? { disabled: "not available" } : {}) }));
  const chosen: (string | undefined)[] = [], keyed: (string | undefined)[] = [];
  const card = ownerListCard<string>({
    title: "Model", placeholder: "search models", items: () => items,
    key: (data, selected) => { if (data !== "\t") return false; keyed.push(selected); return true; },
    done: value => { chosen.push(value); },
  }, host(24));
  card.handleInput(DOWN); card.handleInput("\t");
  expect(keyed).toEqual(["beta"]);
  card.handleInput(ENTER);
  expect(chosen).toEqual([]);
  card.handleInput(DOWN); card.handleInput(ENTER); card.handleInput(ESC);
  expect(chosen).toEqual(["gamma"]);
});
