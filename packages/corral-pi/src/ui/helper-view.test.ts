import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { Text, TuiAltScreen, getKeybindings, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame, type LayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { helperRows } from "./helper-view.ts";
import { HelperChat } from "./helper-chat.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";
beforeAll(() => initTheme("dark", false));
import { NativeWorkbench } from "./workbench.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";

function helper(request: string, prompt: string): HelperInspection {
  return { request, prompt, kind: "explore", model: "fixture/model", generation: 1, phase: "running", createdAt: 1,
    messages: [{ id: "a", kind: "assistant", text: `Visible response for ${prompt}` }], truncated: false };
}
const base: PanelSnapshot = { bound: true, model: "fixture", effort: "off", streaming: true, context: undefined,
  tokens: 0, tools: [], contextFiles: [], skills: [], jobs: [], auxiliary: null, steering: 0, followUps: 0, ledger: null, ledgerStale: false };

test("helper view distinguishes unavailable, empty, stale, queued and uncertain observations", () => {
  const text = (value: Parameters<typeof helperRows>[0], selected?: string, bound = true) => helperRows(value, selected, bound).map(row => row.text).join("\n");
  expect(text(null)).toContain("unavailable"); expect(text([])).toContain("No helper calls yet");
  expect(text([{ ...helper("r", "Question"), phase: "queued", messages: [] }], "r")).toContain("No provider response yet");
  const uncertain = text([{ ...helper("r", "Question"), phase: "uncertain", error: "request interrupted", truncated: true }], "r", false);
  expect(uncertain).toContain("Interrupted / outcome uncertain"); expect(uncertain).toContain("not task completion");
  expect(uncertain).toContain("observations may be stale"); expect(uncertain).toContain("retention limits");
  expect(text([helper("new", "New question")], "evicted")).toContain("no longer retained");
});

function click(frame: LayoutFrame, needle: string): void {
  const y = frame.lines.findIndex(line => stripTerminalSequences(line).includes(needle));
  expect(y).toBeGreaterThanOrEqual(0);
  const box = getLayoutBoxesAt(frame, 2, y).find(box => box.component.handleMouse !== undefined);
  if (!box?.component.handleMouse) throw new Error("No component mouse route");
  expect(box.component.handleMouse({ type: "click", button: "left", x: 2 - box.rect.x, y: y - box.rect.y + (box.lineOffset ?? 0),
    screenX: 2, screenY: y, width: box.rect.width, height: box.rect.height, shift: false, alt: false, ctrl: false, clickCount: 1 })?.handled).toBeTrue();
}

test("keyboard and pointer select stable helper identities without changing draft, parent session or calling a provider", () => {
  let snapshot: PanelSnapshot = { ...base, helpers: [helper("first", "First question"), helper("second", "Second question")] };
  const identity = { store: "s", role: "assistant" as const, project: "", assistant: "A-22", generation: 1 };
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => snapshot, changed() {}, canInteract: () => true });
  const viewport = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
    editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: new Text("REAL_FOOTER", 0, 0), mainView: transcript => wb.mount(transcript) });
  const frame = (width = 80) => renderLayoutFrame(viewport.root, width, 42, () => {});
  const text = () => stripTerminalSequences(frame().lines.join("\n"));
  try {
    wb.setView("activity"); click(frame(), "Helper conversations");
    expect(text()).toContain("Visible response for First question"); expect(text()).toContain("REAL_EDITOR_DRAFT");
    expect(wb.handleInput("j")).toBeTrue(); expect(text()).toContain("Visible response for Second question");
    snapshot = { ...snapshot, helpers: [helper("new", "Newest question"), ...(snapshot.helpers ?? [])] };
    expect(text()).toContain("Visible response for Second question");
    click(frame(), "Research first"); expect(text()).toContain("Visible response for First question");
    for (const width of [52, 80, 140]) {
      const lines = frame(width).lines;
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(stripTerminalSequences(lines.join("\n"))).toContain("REAL_EDITOR_DRAFT");
    }
    expect(wb.handleInput("h")).toBeTrue(); expect(text()).toContain("No background jobs");
    wb.handleInput("\x1b[20~"); expect(text()).toContain("PARENT_TRANSCRIPT");
    expect(identity.generation).toBe(1);
  } finally { wb.dispose(); }
});

test("helper tool expansion and hints use Pi's configured shortcut", () => {
  const keys = getKeybindings(), previous = keys.getUserBindings();
  const wb = new NativeWorkbench({ cwd: "/tmp", identity: { store: "s", project: "", role: "assistant", assistant: "A-test", generation: 1 },
    snapshot: () => null, changed() {}, canInteract: () => true });
  try {
    keys.setUserBindings({ ...previous, "app.tools.expand": "alt+o" });
    wb.openHelpers(); expect(wb.handleInput("\x1bo")).toBe(true);
    expect(helperRows([], undefined, true).map(row => row.text).join("\n").toLowerCase()).toContain("alt+o");
  } finally { keys.setUserBindings(previous); wb.dispose(); }
});

test("native transcript retains returned result while selector metadata retains observed errors", () => {
  const entry = { ...helper("r", "Question"), phase: "completed" as const, result: "\n".repeat(150) + "FINAL_RESULT_VISIBLE",
    error: "OBSERVED_ERROR_VISIBLE", inspectionError: "INSPECTOR_WARNING_VISIBLE",
    messages: Array.from({ length: 64 }, (_, index) => ({ id: String(index), kind: "assistant" as const, text: "line\n".repeat(300) })) };
  const metadata = helperRows([entry], "r", true).map(row => row.text).join("\n");
  const chat = new HelperChat(() => undefined, "/tmp");
  const transcript = stripTerminalSequences(chat.renderHelper(entry, 80).join("\n"));
  expect(transcript).toContain("FINAL_RESULT_VISIBLE");
  expect(metadata).toContain("OBSERVED_ERROR_VISIBLE"); expect(metadata).toContain("INSPECTOR_WARNING_VISIBLE");
});

test("native helper rendering sanitizes data before rendering, preserving only renderer-produced controls", () => {
  const entry = { ...helper("r", "Question"), messages: [{ id: "tool", kind: "tool" as const, tool: "read", phase: "completed" as const,
    text: "\x1b]52;c;EVIL\x07\x1b[2J" + "line\n".repeat(5000) }] };
  const tui = new TuiAltScreen(new RecordingTerminal());
  const chat = new HelperChat(() => tui, "/tmp");
  const rendered = chat.renderHelper(entry, 80).join("\n");
  expect(rendered).not.toContain("\x1b]52"); expect(rendered).not.toContain("\x1b[2J");
  expect(stripTerminalSequences(rendered)).not.toContain("EVIL");
});
