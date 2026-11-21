import { beforeAll, expect, test } from "bun:test";
import { ToolExecutionComponent, TranscriptContainer, UserMessageComponent, getMarkdownTheme, initTheme } from "@earendil-works/pi-coding-agent";
import { Box, Markdown, Text, stripTerminalSequences, type Component, type SelectionRow, type TUI } from "@earendil-works/pi-tui";

import { diffLines, type ChangesModel } from "./changes-view.ts";
import { diffView } from "./tool-lines-files.ts";
import { resultText, rowRenderers, textLines, type Presenter, type RowDeps } from "./tool-row.ts";

beforeAll(() => initTheme("dark", false));

const deps: RowDeps = { ticker: () => undefined };
const tui = { requestRender() {} } as unknown as TUI;

/** What the renderer copies when every row is selected: rows of one line joined, lines on their own rows. */
function copied(rows: readonly SelectionRow[] | undefined): string {
  let out = "", last: number | undefined;
  for (const row of rows ?? []) {
    if (last !== undefined && last !== row.logicalLine) out += "\n";
    last = row.logicalLine;
    out += row.text;
  }
  return out;
}
/** Each row's text is on screen where the row says it starts. */
function expectAligned(lines: readonly string[], rows: readonly SelectionRow[] | undefined): void {
  expect(rows?.length).toBeGreaterThan(0);
  for (const row of rows ?? []) expect(stripTerminalSequences(lines[row.y] ?? "").slice(row.left).startsWith(row.text.trimEnd())).toBe(true);
}
const rowOf = (lines: readonly string[], text: string) => lines.findIndex(line => stripTerminalSequences(line).includes(text));

test("a Markdown paragraph joins its wrapped rows; a code block leaves out its label and indent; a list keeps its markers", () => {
  const text = "alpha beta gamma delta epsilon zeta eta theta\n\n```ts\nconst x = 1;\n  return x;\n```\n\n- first item that wraps around the row\n- second";
  const markdown: Component = new Markdown(text, 1, 0, getMarkdownTheme());
  const lines = markdown.render(24);

  const prose = markdown.selectionRows?.(24, rowOf(lines, "alpha"));
  expect(new Set(prose?.map(row => row.logicalLine)).size).toBe(1);
  expect(prose?.length).toBeGreaterThan(1);
  expect(copied(prose)).toBe("alpha beta gamma delta epsilon zeta eta theta");
  expectAligned(lines, prose);

  const code = markdown.selectionRows?.(24, rowOf(lines, "const x"));
  expect(copied(code)).toBe("const x = 1;\n  return x;");
  expect(code?.some(row => row.y === rowOf(lines, "ts"))).toBe(false);
  expect(code?.[0]?.left).toBeGreaterThan(1);
  expectAligned(lines, code);

  const list = markdown.selectionRows?.(24, rowOf(lines, "first"));
  expect(copied(list)).toBe("- first item that wraps around the row\n- second");
  expectAligned(lines, list);
});

test("Text, Box and a user bubble pass their rows through with the padding left out", () => {
  const box = new Box(2, 1);
  box.addChild(new Text("one two three four five six", 0, 0));
  const boxed: Component = box, lines = boxed.render(16);
  const rows = boxed.selectionRows?.(16, 1);
  expect(copied(rows)).toBe("one two three four five six");
  expect(rows?.[0]).toMatchObject({ y: 1, left: 2 });
  expectAligned(lines, rows);

  const bubble: Component = new UserMessageComponent("hello there, a message long enough to wrap");
  const drawn = bubble.render(20), said = bubble.selectionRows?.(20, rowOf(drawn, "hello"));
  expect(copied(said)).toBe("hello there, a message long enough to wrap");
  expectAligned(drawn, said);
});

test("a tool row's expanded output is one block through Pi's transcript, without the row line or the indent", () => {
  const present: Presenter = call => ({ state: "ok", verb: "run", target: "thing", expanded: () => textLines(resultText(call.result), "text") });
  const row = new ToolExecutionComponent("run", "call-1", {}, { outputPad: 1 }, rowRenderers(present, deps, "run"), tui, "/work");
  row.setArgsComplete(); row.markExecutionStarted();
  row.updateResult({ content: [{ type: "text", text: "first line that is long enough to wrap at this width\n\nthird" }], isError: false });
  row.setExpanded(true);
  const transcript: Component = new TranscriptContainer();
  (transcript as TranscriptContainer).addChild(row);
  const lines = transcript.render(30), head = rowOf(lines, "run thing");

  expect(transcript.selectionRows?.(30, head)?.some(selected => selected.y === head) ?? false).toBe(false);
  const rows = transcript.selectionRows?.(30, rowOf(lines, "first"));
  expect(copied(rows)).toBe("first line that is long enough to wrap at this width\n\nthird");
  expect(rows?.[0]?.left).toBe(3);
  expectAligned(lines, rows);
});

test("a collapsed body copies what shows: no `…` and no hidden count", () => {
  const present: Presenter = () => ({ state: "failed", verb: "run", body: [{ text: "x".repeat(60) }], hiddenHint: "12 lines" });
  const row = new ToolExecutionComponent("run", "call-2", {}, { outputPad: 0 }, rowRenderers(present, deps, "run"), tui, "/work");
  row.setArgsComplete(); row.markExecutionStarted();
  row.updateResult({ content: [{ type: "text", text: "boom" }], isError: true });
  const transcript: Component = new TranscriptContainer();
  (transcript as TranscriptContainer).addChild(row);
  const lines = transcript.render(40), rows = transcript.selectionRows?.(40, rowOf(lines, "xxx"));
  expect(rows?.length).toBe(1);
  expect(rows?.[0]?.text).toMatch(/^x+$/);
  expect(stripTerminalSequences(lines[rows?.[0]?.y ?? 0] ?? "")).toContain("… 12 lines");
  expectAligned(lines, rows);
});

test("an edit's diff and the Changes diff copy the code without line numbers or signs", () => {
  const view = diffView(" 1 keep\n-2 old line\n+2 new line", "a.ts"), lines = view.render(40);
  expect(copied(view.selectionRows?.(40, 0))).toBe("keep\nold line\nnew line");
  expectAligned(lines, view.selectionRows?.(40, 0));

  const patch = ["diff --git a/a.rs b/a.rs", "--- a/a.rs", "+++ b/a.rs", "@@ -10,3 +10,4 @@ fn first() {", " keep", "-old", "+new", "+more", " tail",
    "@@ -40,2 +41,2 @@", "-gone", "+came", ""].join("\n");
  const model: ChangesModel = { comparison: "since session", custom: false, range: "a → worktree", state: { text: "worktree", tone: "muted" },
    scope: "repo", updated: "now", query: "", notes: [], hunk: 0, selected: "a.rs", files: [{ path: "a.rs", letter: "M", added: 3, removed: 2 }],
    total: 1, authors: [], filter: "all", diff: { kind: "patch", path: "a.rs", status: "M", text: patch } };
  const frame = diffLines(model, 60);
  expect(frame.hunkText.map(rows => copied(rows))).toEqual(["keep\nold\nnew\nmore\ntail", "gone\ncame"]);
  for (const rows of frame.hunkText) expectAligned(frame.lines, rows);
});
