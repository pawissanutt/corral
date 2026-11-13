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
