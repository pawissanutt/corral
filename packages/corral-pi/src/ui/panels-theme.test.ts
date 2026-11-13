import { afterAll, expect, test } from "bun:test";
import * as sdk from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
// The theme global the SDK components read; the package does not export its setter.
import { setThemeInstance } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/theme/theme.js";
import { hiddenThinkingLabel, panelTheme, thinkingLead } from "./panels-theme.ts";
import { setPanelAppearance } from "./panels-view.ts";

afterAll(() => { setPanelAppearance("dark"); sdk.initTheme("dark", false); });

/** The SGR codes in front of the first visible character of `row` (OSC 133 zone marks removed). */
const lead = (row: string) => (row.replace(/\x1b\][^\x07]*\x07/g, "").match(/^(?:\s|\x1b\[[0-9;]*m)*/)?.[0] ?? "").replace(/\s/g, "");

test("thinking reads apart from the answer in every appearance", () => {
  sdk.initTheme("dark", false);
  for (const appearance of ["dark", "light", "contrast"] as const) {
    setPanelAppearance(appearance);
    setThemeInstance(panelTheme(sdk.Theme, "truecolor", appearance));
    const message = new sdk.AssistantMessageComponent({ stopReason: "stop", content: [
      { type: "thinking", thinking: "I will read the file." }, { type: "text", text: "The answer." }] },
    false, sdk.getMarkdownTheme(), hiddenThinkingLabel(), 1, [thinkingLead]);
    const rows = message.render(60).filter(row => stripTerminalSequences(row).trim());
    const thinking = rows.find(row => stripTerminalSequences(row).includes("I will read"))!;
    const answer = rows.find(row => stripTerminalSequences(row).includes("The answer"))!;
    expect(stripTerminalSequences(thinking).trim().startsWith("thinking I will read")).toBeTrue();
    expect(lead(thinking)).not.toBe(lead(answer));
  }
});

test("hidden thinking names what it is", () => {
  expect(hiddenThinkingLabel().startsWith("thinking hidden")).toBeTrue();
});

test("narrow answers keep code shaped as code and stack tables (TX-20, TX-23)", () => {
  sdk.initTheme("dark", false);
  setPanelAppearance("dark");
  setThemeInstance(panelTheme(sdk.Theme, "truecolor", "dark"));
  const rows = (text: string) => new sdk.AssistantMessageComponent({ stopReason: "stop", content: [{ type: "text", text }] },
    false, sdk.getMarkdownTheme(), "", 1, []).render(46).map(row => stripTerminalSequences(row).trimEnd()).filter(row => row.trim());
  const code = rows("```ts\nexport function shortName(name: string): string { return name.replace(/^corral_/, \"\"); }\n```");
  expect(code.some(row => row.includes("```"))).toBeFalse();
  expect(code.length).toBeGreaterThan(2);
  // Padding 1 plus the block indent 2: wrapped rows never restart at the left edge.
  expect(code.slice(1).every(row => row.startsWith("   "))).toBeTrue();
  const table = rows("| Cause | Evidence |\n|---|---|\n| one | first |\n| two | second |");
  expect(table.some(row => row.includes("│"))).toBeFalse();
  expect(table.filter(row => row.trimStart().startsWith("Evidence")).length).toBe(2);
  // A table whose body has not arrived yet still shows its headers.
  const empty = rows("| Cause | Evidence |\n|---|---|");
  expect(empty.map(row => row.trim())).toEqual(["Cause", "Evidence"]);
});
