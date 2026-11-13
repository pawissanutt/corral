import { beforeAll, expect, test } from "bun:test";
import { AssistantMessageComponent, ToolExecutionComponent, UserMessageComponent, getMarkdownTheme, initTheme, withBuiltInRenderers } from "@earendil-works/pi-coding-agent";
import { TuiAltScreen, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { AuxiliaryHistory, type HelperInspection } from "../extensions/subagent/inspection.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";
import { HelperChat, bubble, promptLabel, type HelperChatStyle } from "./helper-chat.ts";
import { userLabel } from "./panels-theme.ts";
import { PANEL_BUBBLE } from "./panels-view.ts";

beforeAll(() => initTheme("dark", false));
const prompt = "Explain **this code**, then read `sample.ts`.";
const response = "## Findings\n\n**Shared styling** and `inline code`.\n\n```ts\nconst answer = 42;\n```\n\n| Item | State |\n| --- | --- |\n| Native chat | ready |";
const result = "const answer = 42;\nexport { answer };";
function fixture(): HelperInspection {
  return { request: "fixture", generation: 1, kind: "explore", model: "fixture/model", createdAt: 0, prompt, phase: "completed", truncated: false,
    messages: [{ id: "assistant", kind: "assistant", text: response, stopReason: "toolUse", hasToolCalls: true },
      { id: "read", kind: "tool", tool: "read", arguments: JSON.stringify({ path: "sample.ts" }), text: result, phase: "completed" }] };
}
function style(outputPad: 0 | 1): HelperChatStyle {
  return { outputPad, markdownTheme: { ...getMarkdownTheme(), codeBlockIndent: "  " }, markdownTransformers: [],
    renderers: name => withBuiltInRenderers(name, undefined) };
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const click = (y: number, height: number): TuiMouseEvent => ({ type: "click", button: "left", x: 2, y, width: 80, height,
  shift: false, alt: false, ctrl: false, screenX: 2, screenY: y });

test("the prompt is the native user bubble with its own label, replies are native Pi rendering, and a tool call is one line until tools expand", () => {
  const tui = new TuiAltScreen(new RecordingTerminal());
  for (const padding of [0, 1] as const) for (const width of [24, 80, 120]) {
    const settings = style(padding), chat = new HelperChat(() => tui, "/tmp", () => settings);
    const nativeAssistant = new AssistantMessageComponent({ stopReason: "toolUse", content: [{ type: "text", text: response },
      { type: "toolCall", id: "read", name: "read", arguments: { path: "sample.ts" } }] }, true, settings.markdownTheme, "", padding, []);
    const nativeTool = new ToolExecutionComponent("read", "read", { path: "sample.ts" }, { showImages: false, outputPad: padding }, settings.renderers("read"), tui, "/tmp");
    nativeTool.setArgsComplete(); nativeTool.markExecutionStarted();
    nativeTool.updateResult({ content: [{ type: "text", text: result }], isError: false });
    const prompted = new UserMessageComponent(prompt, settings.markdownTheme, padding, [promptLabel("brief from parent")]).render(width);
    const actual = chat.renderHelper(fixture(), width, { bubble: { label: "brief from parent", notes: ["Owns: src/"] } });
    expect(actual.slice(0, prompted.length)).toEqual(prompted);
    expect(plain(actual)[0]?.startsWith(" ".repeat(1 + padding) + "brief from parent Explain")).toBe(width >= 80);
    // The notes sit inside the bubble in its geometry, and the reply follows with its own leading blank row.
    expect(plain(actual)[prompted.length]?.trim()).toBe("Owns: src/");
    expect(visibleWidth(actual[prompted.length] ?? "")).toBe(visibleWidth(prompted[0] ?? ""));
    const assistant = nativeAssistant.render(width);
    expect(actual.slice(prompted.length + 1, prompted.length + 1 + assistant.length)).toEqual(assistant);
    expect(plain(actual.slice(prompted.length + 1 + assistant.length))).toEqual(["", " ✓ read sample.ts"]);
    expect(actual.every(line => visibleWidth(line) <= width)).toBe(true);
    // Expanding tools reuses the native card rather than a flattened copy.
    chat.expandTools(true); nativeTool.setExpanded(true);
    expect(chat.renderHelper(fixture(), width).slice(-nativeTool.render(width).length)).toEqual(nativeTool.render(width));
    expect(stripTerminalSequences(chat.renderHelper(fixture(), width).join("\n"))).toContain("const answer = 42;");
  }
});

test("a bubble sits one cell in on the bubble surface, at most 96 cells wide as the transcript's user bubble, with its notes inside", () => {
  const rows = bubble("you → host-slots", "word ".repeat(80), 140, ["Owns: crates/host"]);
  for (const row of rows) {
    expect(row).toContain(PANEL_BUBBLE);
    expect(visibleWidth(row)).toBe(1 + 96);
    expect(stripTerminalSequences(row).startsWith("  ")).toBeTrue();
  }
  expect(stripTerminalSequences(rows[0] ?? "")).toMatch(/^ {2}you → host-slots word/);
  expect(stripTerminalSequences(rows.at(-1) ?? "").trim()).toBe("Owns: crates/host");
  for (const width of [24, 46]) for (const row of bubble("brief from parent", "word ".repeat(40), width)) expect(visibleWidth(row)).toBe(width);
});

test("native components update from streaming observations, hide reasoning and do not duplicate the final result", () => {
  const history = new AuxiliaryHistory(); history.begin("stream", "explore", "fixture/model", 2, "Question");
  history.phase("stream", "running"); history.observe("stream", { type: "assistant_start" });
  history.observe("stream", { type: "assistant_delta", text: "First **chunk" });
  const chat = new HelperChat(() => undefined, "/tmp");
  const first = chat.renderHelper(history.snapshot()[0]!, 80).join("\n");
  expect(stripTerminalSequences(first)).toContain("First **chunk");
  history.observe("stream", { type: "assistant_end", text: "First **chunk completed**", stopReason: "stop", hasToolCalls: false });
  history.finish("stream", "completed", "First **chunk completed**");
  const finished = stripTerminalSequences(chat.renderHelper(history.snapshot()[0]!, 80).join("\n"));
  expect(finished).toContain("First chunk completed");
  expect(finished.split("First chunk completed").length).toBe(2);
  expect(finished).not.toContain("Returned result");
});

test("multiple text blocks and response errors use the native assistant composition without invented model/usage fields", () => {
  const settings = style(1), blocks = ["First paragraph", "```ts\nconst second = true;\n```"], errorMessage = "Fixture request interrupted";
  const entry = { ...fixture(), phase: "uncertain" as const, result: blocks.join(""), messages: [{ id: "assistant", kind: "assistant" as const,
    text: blocks.join("\n"), textBlocks: blocks, errorMessage, stopReason: "error" as const, hasToolCalls: false }] };
  const chat = new HelperChat(() => undefined, "/tmp", () => settings);
  const assistant = new AssistantMessageComponent({ content: blocks.map(text => ({ type: "text", text })), stopReason: "error", errorMessage }, true, settings.markdownTheme, "", 1, []);
  const user = new UserMessageComponent(prompt, settings.markdownTheme, 1, [promptLabel("prompt")]);
  expect(chat.renderHelper(entry, 80)).toEqual([...user.render(80), ...assistant.render(80)]);
});

test("the owner's `you` label never reaches a helper prompt or reply", () => {
  // The host registers `userLabel` behind a wrapper of its own, as here.
  const chat = new HelperChat(() => undefined, "/tmp", () => ({ ...style(1), markdownTransformers: [(markdown, context) => userLabel(markdown, context)] }));
  const text = plain(chat.renderHelper({ ...fixture(), messages: [{ id: "a", kind: "assistant", text: "REPLY", stopReason: "stop" }] }, 80,
    { bubble: { label: "brief from parent" } }));
  expect(text.some(line => /\byou\b/.test(line))).toBeFalse();
  expect(text.some(line => line.trim() === "REPLY")).toBeTrue();
  expect(text[0]?.trimStart().startsWith("brief from parent Explain")).toBeTrue();
});

test("a tool line opens its native card by pointer, stays open across refreshes, and folds back without mutating history", () => {
  const tui = new TuiAltScreen(new RecordingTerminal()), entry = fixture(), before = JSON.stringify(entry);
  const chat = new HelperChat(() => tui, "/tmp", () => style(1));
  const initial = chat.renderHelper(entry, 80);
  expect(stripTerminalSequences(initial.join("\n"))).not.toContain("export { answer }");
  const row = plain(initial).findIndex(line => line.trim().startsWith("✓ read sample.ts"));
  expect(row).toBeGreaterThan(0);
  expect(chat.handleMouse(click(row, initial.length))?.handled).toBe(true);
  const opened = chat.renderHelper(entry, 80);
  expect(stripTerminalSequences(opened.join("\n"))).toContain("export { answer }");
  chat.invalidate();
  const refreshed = chat.renderHelper(entry, 80);
  expect(stripTerminalSequences(refreshed.join("\n"))).toContain("export { answer }");
  const header = plain(refreshed).findIndex(line => line.trim().startsWith("read sample.ts"));
  expect(chat.handleMouse(click(header, refreshed.length))?.handled).toBe(true);
  expect(plain(chat.renderHelper(entry, 80)).some(line => line.trim() === "✓ read sample.ts")).toBeTrue();
  expect(JSON.stringify(entry)).toBe(before);
});

test("incomplete argument JSON uses native generic rendering, never a built-in renderer with corrupt args", () => {
  const tui = new TuiAltScreen(new RecordingTerminal()); let resolved = 0;
  const chat = new HelperChat(() => tui, "/tmp", () => ({ ...style(1), renderers: () => { resolved++; return undefined; } }));
  const entry = fixture();
  const incomplete = { ...entry, messages: [{ id: "t", kind: "tool" as const, tool: "read", phase: "error" as const,
    arguments: '{"path":"truncated', text: "Arguments omitted by retention limit" }] };
  expect(stripTerminalSequences(chat.renderHelper(incomplete, 80).join("\n"))).toContain("✗ read");
  chat.expandTools(true);
  expect(stripTerminalSequences(chat.renderHelper(incomplete, 80).join("\n"))).toContain("truncated");
  expect(resolved).toBe(0);
});

test("data controls are sanitized before Markdown and tool argument rendering; native ANSI remains intact", () => {
  const tui = new TuiAltScreen(new RecordingTerminal());
  const chat = new HelperChat(() => tui, "/tmp", () => style(1));
  const entry = { ...fixture(), prompt: "Question\x1b]52;c;CLIPBOARD\x07", messages: [
    { id: "a", kind: "assistant" as const, text: "**Visible**\x1b[2J\x1b]52;c;SECRET\x07", stopReason: "stop" as const },
    { id: "t", kind: "tool" as const, tool: "read", arguments: JSON.stringify({ path: "safe\x1b[2J.ts" }), text: "result", phase: "completed" as const },
  ] };
  for (const expanded of [false, true]) {
    chat.expandTools(expanded);
    const rendered = chat.renderHelper(entry, 80).join("\n");
    expect(rendered).not.toContain("\x1b]52"); expect(rendered).not.toContain("\x1b[2J");
    expect(rendered).not.toContain("SECRET"); expect(rendered).not.toContain("CLIPBOARD");
    expect(rendered).toContain("\x1b["); expect(stripTerminalSequences(rendered)).toContain("Visible");
    expect(stripTerminalSequences(rendered)).toContain("safe.ts");
  }
});
