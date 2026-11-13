import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { TuiMouseEvent } from "@earendil-works/pi-tui";
import { ChoiceDock, choiceLines, choiceRows } from "./choice-dock.ts";
import type { ChoiceDialog } from "./choice-dock.ts";

const ENTER = "\r", ESC = "\x1b", DOWN = "\x1b[B";
const click = (x: number, y: number): TuiMouseEvent =>
  ({ type: "click", button: "left", x, y, screenX: x, screenY: y, width: 120, height: 12, shift: false, alt: false, ctrl: false });
const screen = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
interface Frame { readonly lines: string[]; readonly hits: ReadonlyMap<number, number> }
const EXIT: ChoiceDialog = {
  key: "exit:1", title: "How should this leave the build session?", short: "How should this leave?",
  meta: "mitiplay policy: gate allowed, review optional",
  options: [
    { label: "Keep on the branch", detail: "the worktree and its branch stay; nothing is filed" },
    { label: "Land after the gate", detail: "runs the gate, then lands", unavailable: "Project policy requires independent review." },
    { label: "Ship through the board", detail: "one task in Review: gate, independent review, land" },
    { label: "Keep iterating", detail: "stay in the session" },
  ],
};
const QUESTION: ChoiceDialog = {
  key: "question:picker-ui:1", title: "picker-ui asks", titleColor: "\x1b[35m", meta: "frontend · claude-opus-5.5 · paused until you answer",
  body: (width, rows) => {
    const words = "The wireframe has no empty state. With zero slots, what should the arrival screen show? 界界界 \x1b[2J".repeat(4);
    const lines: string[] = [];
    for (let at = 0; at < words.length && lines.length < rows; at += width) lines.push(words.slice(at, at + width).replace(/\x1b/g, " "));
    return lines;
  },
  options: [
    { label: "Show a \"New game\" action", detail: "the picker renders one row that creates the first slot" },
    { label: "Hide the picker", detail: "arrival looks as it does today until a slot exists" },
    { label: "Let picker-ui decide", detail: "it picks one and says which in its report" },
    { label: "Write my own answer", detail: "type it in the composer; Enter sends it to picker-ui" },
  ],
};

function harness(dialog: ChoiceDialog) {
  const calls = { picks: [] as number[], refused: [] as string[], dismissed: 0, focus: 0 };
  const dock = new ChoiceDock({
    dialog: () => dialog,
    pick: index => { calls.picks.push(index); },
    refuse: option => { calls.refused.push(option.label); },
    dismiss: () => { calls.dismissed++; },
    click: event => event.type === "click" && event.button === "left",
    focus: () => { calls.focus++; },
    changed: () => {},
  });
  dock.focused = true;
  return { dock, calls };
}

test("digits outside the options do nothing and are not typed into the composer", () => {
  const { dock, calls } = harness(EXIT);
  for (const key of ["5", "9", "0"]) expect(dock.handleKey(key)).toBeTrue();
  expect(calls.picks).toEqual([]);
  expect(calls.refused).toEqual([]);
  dock.handleKey("3");
  expect(calls.picks).toEqual([2]);
});

test("an unavailable option is never picked; the host is asked to raise its reason", () => {
  const { dock, calls } = harness(EXIT);
  dock.handleKey("2");
  dock.handleKey(DOWN); dock.handleKey("k");
  expect(dock.selected).toBe(1);
  dock.handleKey(ENTER);
  expect(calls.picks).toEqual([]);
  expect(calls.refused).toEqual(["Land after the gate", "Land after the gate"]);
});

test("the guard after an automatic open swallows Enter and digits only", () => {
  const { dock, calls } = harness(EXIT);
  dock.guard(Date.now() + 10_000);
  expect(dock.handleKey(ENTER)).toBeTrue();
  expect(dock.handleKey("1")).toBeTrue();
  expect(calls.picks).toEqual([]);
  dock.handleKey(DOWN);
  expect(dock.selected).toBe(1);
  dock.handleKey("j");
  expect(dock.selected).toBe(2);
  dock.handleKey(ESC);
  expect(calls.dismissed).toBe(1);
  dock.guard(0);
  dock.handleKey(ENTER);
  expect(calls.picks).toEqual([2]);
});

test("the selected option stays visible when the rows are fewer than the content", () => {
  const many: ChoiceDialog = { ...EXIT, key: "many", options: [...EXIT.options, { label: "Fifth", detail: "the last option" }] };
  for (const [width, short] of [[60, false], [60, true], [120, false]] as const) {
    for (const maxRows of [3, 4, 5]) {
      for (let selected = 0; selected < many.options.length; selected++) {
        const frame = choiceLines(many, selected, width, maxRows, short);
        expect(frame.lines.length).toBeLessThanOrEqual(maxRows);
        expect(frame.selectedRow).toBeGreaterThan(0);
        expect(frame.hits.get(frame.selectedRow)).toBe(selected);
        expect(screen(frame.lines)[frame.selectedRow]).toContain(many.options[selected]!.label);
      }
    }
  }
  // With no row left for the body, the question joins the title row.
  const tight = screen(choiceLines(QUESTION, 0, 80, 5, true).lines);
  expect(tight[0]).toContain("picker-ui asks");
  expect(tight[0]).toContain("wireframe");
});

test("options take one row from 84 columns, two below, and drop the detail when short", () => {
  const rowsOf = (frame: Frame, option: number) => [...frame.hits.values()].filter(value => value === option).length;
  const wide = choiceLines(EXIT, 0, 100, 30, false), narrow = choiceLines(EXIT, 0, 70, 30, false), short = choiceLines(EXIT, 0, 70, 30, true);
  for (let option = 0; option < EXIT.options.length; option++) {
    expect(rowsOf(wide, option)).toBe(1);
    expect(rowsOf(narrow, option)).toBe(2);
    expect(rowsOf(short, option)).toBe(1);
  }
  const text = (frame: Frame) => screen(frame.lines).join("\n");
  expect(text(wide)).toContain("nothing is filed");
  expect(text(narrow)).toContain("nothing is filed");
  // An unavailable option keeps a marker on its label row when its detail row is dropped.
  const land = screen(short.lines).find(line => line.includes("Land after the gate"))!;
  expect(land).toContain("unavailable");
  // The meta only shows from 96 columns.
  expect(text(wide)).toContain("mitiplay");
  expect(text(narrow)).not.toContain("mitiplay");
});

test("the top and bottom borders are exactly the dialog's width and keep their corners", () => {
  const LONG: ChoiceDialog = { ...EXIT, key: "long", title: "How should this leave the build session? 界界界 · done",
    meta: "qa-repo · outside a corral workspace · 3 agents still running 界界界 and a very long project label" };
  const QA: ChoiceDialog = { ...EXIT, key: "qa", meta: "qa-repo · outside a corral workspace" };
  for (const dialog of [EXIT, LONG, QA, QUESTION]) {
    for (const short of [false, true]) {
      for (let width = 8; width <= 160; width++) {
        const lines = choiceLines(dialog, 0, width, 14, short).lines, rows = screen(lines);
        for (const line of lines) expect(visibleWidth(line)).toBe(width);
        expect(rows[0]!.startsWith("╭─")).toBeTrue();
        expect(rows[0]!.endsWith("─╮")).toBeTrue();
        for (const row of rows.slice(1)) expect(row.endsWith("│") || row.endsWith("╯")).toBeTrue();
      }
    }
  }
  // With the sidebar rail at 140 columns the dock is 105 wide: the whole label fits and the corner closes the row.
  const top = screen(choiceLines(QA, 0, 105, 14, false).lines)[0]!;
  expect(top).toContain("qa-repo · outside a corral workspace");
  // When the label cannot fit whole it is shortened with an ellipsis before it is dropped.
  const clipped = screen(choiceLines(LONG, 0, 120, 14, false).lines)[0]!;
  expect(clipped).toContain("qa-repo");
  expect(clipped).toContain("…");
});

test("clicking an option row picks the same option as its digit", () => {
  for (const width of [60, 120]) {
    for (let option = 0; option < EXIT.options.length; option++) {
      const byKey = harness(EXIT), byClick = harness(EXIT);
      byKey.dock.handleKey(String(option + 1));
      const rows = screen(byClick.dock.render(width)), y = rows.findIndex(row => row.includes(EXIT.options[option]!.label));
      expect(y).toBeGreaterThan(0);
      for (let clicks = 0; clicks < 2 && !byClick.calls.picks.length && !byClick.calls.refused.length; clicks++) {
        byClick.dock.render(width);
        expect(byClick.dock.handleMouse(click(6, y))?.handled).toBeTrue();
      }
      expect(byClick.calls.picks).toEqual(byKey.calls.picks);
      expect(byClick.calls.refused).toEqual(byKey.calls.refused);
      expect(byClick.calls.picks.length + byClick.calls.refused.length).toBe(1);
    }
  }
});

test("every line fits from 30 to 140 columns at any row limit", () => {
  expect(choiceRows(14, 7)).toBe(5);
  expect(choiceRows(46, 9)).toBe(26);
  expect(choiceRows(10, 9)).toBe(0);
  for (const dialog of [EXIT, QUESTION]) {
    for (const maxRows of [2, 5, 8, 14]) {
      for (const short of [false, true]) {
        for (let width = 30; width <= 140; width++) {
          const lines = choiceLines(dialog, 1, width, maxRows, short).lines;
          expect(lines.length).toBeLessThanOrEqual(maxRows);
          for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
          expect(lines.join("")).not.toContain("\x1b[2J");
        }
      }
    }
  }
});
