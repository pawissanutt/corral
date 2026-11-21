import { expect, test } from "bun:test";
import { Container, Editor, HStack, ScrollView, Text, TuiAltScreen, VStack, stripTerminalSequences } from "@earendil-works/pi-tui";
import { RecordingTerminal } from "./support/panel-terminal.ts";

test("a drag out of the wrapped editor copies the logical draft, not the footer", async () => {
  const terminal = new RecordingTerminal();
  terminal.resize(18, 32);
  let copied: string | undefined;
  const tui = new TuiAltScreen(terminal, false, undefined, { copySelection: async text => { copied = text; return true; } });
  const ink = (s: string) => s;
  const editor = new Editor(tui, { borderColor: ink, selectList: { selectedPrefix: ink, selectedText: ink, description: ink, scrollInfo: ink, noMatch: ink } }, { paddingX: 2 });
  const draft = "界 alpha beta gamma delta  \n  next  ";
  editor.setText(draft);
  const dock = new Container();
  dock.addChild(editor);
  tui.setLayoutRoot(new VStack([{ component: dock }, { component: new Text("controls", 0, 0) }]));
  tui.start();
  try {
    await terminal.untilVisible("controls");
    terminal.send("\x1b[<0;3;2M");
    terminal.send("\x1b[<32;16;9M");
    terminal.send("\x1b[<0;16;9m");
    expect(copied).toBe(draft);
    expect(editor.getText()).toBe(draft);
  } finally { tui.stop(); }
});

function scrolledText() {
  const terminal = new RecordingTerminal();
  terminal.resize(32, 8);
  const copies: string[] = [];
  const tui = new TuiAltScreen(terminal, false, undefined, { copySelection: async text => { copies.push(text); return true; } });
  const prose = ["anchor", ...Array.from({ length: 29 }, (_, i) => `word-${i}`), "last-word"].join(" ");
  const document = new Text(`before one\nbefore two\nbefore three\n${prose}`, 2, 0);
  const scroll = new ScrollView(document, { scrollbar: "always" });
  const body = new HStack([{ component: new Text("rail", 0, 0), basis: 5 }, { component: scroll, grow: 1 }]);
  tui.setLayoutRoot(new VStack([{ component: new Text("header", 0, 0) }, { component: body, grow: 1, basis: 0 }, { component: new Text("footer", 0, 0) }]));
  return { terminal, tui, scroll, prose, copies };
}

test("edge drag in a scrolled pane highlights content and copies logical rows across the viewport", async () => {
  const { terminal, tui, scroll, prose, copies } = scrolledText();
  tui.start();
  try {
    await terminal.untilVisible("before one");
    const after = terminal.writes.length;
    scroll.scrollTo(3);
    await terminal.untilVisible("anchor", after);
    expect(scroll.scrollTop).toBe(3);
    const anchorColumn = stripTerminalSequences(tui.getScreenLines()[1]!).indexOf("anchor") + 1;
    expect(anchorColumn).toBeGreaterThan(1);
    const drag = terminal.writes.length;
    terminal.send(`\x1b[<0;${anchorColumn};2M`);
    terminal.send("\x1b[<32;32;8M");
    await terminal.untilVisible("last-word", drag);
    expect(scroll.scrollTop).toBeGreaterThan(3);
    const screen = tui.getScreenLines();
    const endRow = screen.find(line => line.includes("last-word")) ?? "";
    const highlight = /\x1b\[7m(.*?)\x1b\[27m/.exec(endRow);
    expect(stripTerminalSequences(highlight?.[1] ?? "")).toBe("last-word");
    expect(screen[0]).not.toContain("\x1b[7m");
    expect(screen.at(-1)).not.toContain("\x1b[7m");
    terminal.send("\x1b[<0;32;8m");
    expect(copies).toEqual([prose]);
  } finally { tui.stop(); }
});

test("a width change invalidates an in-progress semantic drag before release", async () => {
  const { terminal, tui, copies } = scrolledText();
  tui.start();
  try {
    await terminal.untilVisible("anchor");
    terminal.send("\x1b[<0;8;5M");
    terminal.send("\x1b[<32;20;6M");
    expect(tui.hasActiveSelection()).toBe(true);
    const after = terminal.writes.length;
    terminal.resize(60, 8);
    await terminal.untilVisible("anchor word-0 word-1 word-2 word-3", after);
    expect(tui.hasActiveSelection()).toBe(false);
    expect(tui.getScreenLines().every(line => !line.includes("\x1b[7m"))).toBe(true);
    terminal.send("\x1b[<0;20;6m");
    expect(copies).toEqual([]);
  } finally { tui.stop(); }
});
