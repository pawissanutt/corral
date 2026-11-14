import { expect, test } from "bun:test";
import { CURSOR_MARKER, stripTerminalSequences, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { PopupList, type PopupRow } from "./popup-list.ts";

const PAGE_DOWN = "\x1b[6~", DOWN = "\x1b[B";

test("at 46 columns an entry taller than the list pages through to its last row before the selection moves; a disabled entry is never chosen nor offered Enter", () => {
  const rows: PopupRow<string>[] = [
    { value: "a", match: "alpha", lines: width => wrapTextWithAnsi(`A-1 ${"long recorded title ".repeat(24)}TAIL`, width) },
    { value: "b", match: "beta", disabled: "current · this pane", lines: () => ["B-2 beta"] },
    { value: "c", match: "gamma", lines: () => ["C-3 gamma"] },
  ];
  let chosen: string | undefined | null = null;
  const list = new PopupList<string>({ title: "Browse history", placeholder: "search", verb: "resume", rows: () => rows, capacity: () => 6,
    done: value => { chosen = value; }, changed: () => {} });
  const hintRow = () => { const lines = list.render(46); return { text: stripTerminalSequences(lines.at(-2)!), y: lines.length - 2 }; };
  list.focused = true;
  const frame = list.render(46);
  expect(frame.join("").includes(CURSOR_MARKER)).toBeTrue();
  for (const line of frame) expect(visibleWidth(line)).toBe(46);
  const seen: string[] = [stripTerminalSequences(frame.join("\n"))];
  for (let page = 0; page < 20 && list.selected === "a"; page++) {
    list.handleInput(PAGE_DOWN);
    if (list.selected === "a") seen.push(stripTerminalSequences(list.render(46).join("\n")));
  }
  expect(seen[0]).not.toContain("TAIL");
  expect(seen.at(-1)).toContain("TAIL");
  expect(list.selected).toBe("b");
  expect(stripTerminalSequences(list.render(46).join("\n"))).toContain("current · this pane");
  expect(hintRow().text).not.toContain("resume");
  list.handleInput("\r");
  expect(chosen).toBeNull();
  list.handleInput(DOWN);
  const live = hintRow();
  expect(live.text).toContain("resume");
  list.handleMouse({ type: "click", button: "left", x: live.text.indexOf("⏎"), y: live.y, screenX: 0, screenY: 0,
    width: 46, height: live.y + 2, shift: false, alt: false, ctrl: false, clickCount: 1 });
  expect(chosen).toBe("c");
});
