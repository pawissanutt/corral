import { expect, test } from "bun:test";
import { Container, Editor, Text, TuiAltScreen, VStack } from "@earendil-works/pi-tui";
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
