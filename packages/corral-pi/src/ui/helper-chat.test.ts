import { beforeAll, expect, test } from "bun:test";
import { AssistantMessageComponent, UserMessageComponent, ToolExecutionComponent, getMarkdownTheme, initTheme, withBuiltInRenderers } from "@earendil-works/pi-coding-agent";
import { TuiAltScreen, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { AuxiliaryHistory, type HelperInspection } from "../extensions/subagent/inspection.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";
import { HelperChat, type HelperChatStyle } from "./helper-chat.ts";

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

test("helper messages are byte-for-byte native Pi user, assistant and read-tool rendering at each width/padding", () => {
  const tui = new TuiAltScreen(new RecordingTerminal());
  for (const padding of [0, 1] as const) for (const width of [24, 80, 120]) {
    const settings = style(padding), chat = new HelperChat(() => tui, "/tmp", () => settings);
    const nativeUser = new UserMessageComponent(prompt, settings.markdownTheme, padding, []);
    const nativeAssistant = new AssistantMessageComponent({ stopReason: "toolUse", content: [{ type: "text", text: response },
      { type: "toolCall", id: "read", name: "read", arguments: { path: "sample.ts" } }] }, true, settings.markdownTheme, "", padding, []);
    const nativeTool = new ToolExecutionComponent("read", "read", { path: "sample.ts" }, { showImages: false, outputPad: padding }, settings.renderers("read"), tui, "/tmp");
    nativeTool.setArgsComplete(); nativeTool.markExecutionStarted();
    nativeTool.updateResult({ content: [{ type: "text", text: result }], isError: false });
    const expected = [...nativeUser.render(width), ...nativeAssistant.render(width), ...nativeTool.render(width)];
    const actual = chat.renderHelper(fixture(), width);
    expect(actual).toEqual(expected);
    expect(actual.every(line => visibleWidth(line) <= width)).toBe(true);
    // Native card expansion is reused, not flattened into custom Text/ActivityRow output.
    chat.expandTools(true); nativeTool.setExpanded(true);
    expect(chat.renderHelper(fixture(), width)).toEqual([...nativeUser.render(width), ...nativeAssistant.render(width), ...nativeTool.render(width)]);
    expect(stripTerminalSequences(chat.renderHelper(fixture(), width).join("\n"))).toContain("const answer = 42;");
  }
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

test("multiple text blocks and response errors use the same native composition without invented model/usage fields", () => {
  const settings = style(1), blocks = ["First paragraph", "```ts\nconst second = true;\n```"], errorMessage = "Fixture request interrupted";
  const entry = { ...fixture(), phase: "uncertain" as const, result: blocks.join(""), messages: [{ id: "assistant", kind: "assistant" as const,
    text: blocks.join("\n"), textBlocks: blocks, errorMessage, stopReason: "error" as const, hasToolCalls: false }] };
  const chat = new HelperChat(() => undefined, "/tmp", () => settings);
  const user = new UserMessageComponent(prompt, settings.markdownTheme, 1, []);
  const assistant = new AssistantMessageComponent({ content: blocks.map(text => ({ type: "text", text })), stopReason: "error", errorMessage }, true, settings.markdownTheme, "", 1, []);
  expect(chat.renderHelper(entry, 80)).toEqual([...user.render(80), ...assistant.render(80)]);
});

test("native tool cards expand by pointer and stay expanded across refreshes without mutating history", () => {
  const tui = new TuiAltScreen(new RecordingTerminal()), entry = fixture(), before = JSON.stringify(entry);
  const chat = new HelperChat(() => tui, "/tmp", () => style(1));
  const initial = chat.renderHelper(entry, 80);
  expect(stripTerminalSequences(initial.join("\n"))).not.toContain("export { answer }");
  const row = initial.findIndex(line => stripTerminalSequences(line).trim().startsWith("read sample.ts"));
  expect(row).toBeGreaterThan(0);
  const event: TuiMouseEvent = { type: "click", button: "left", x: 2, y: row, width: 80, height: initial.length,
    shift: false, alt: false, ctrl: false, screenX: 2, screenY: row };
  expect(chat.handleMouse(event)?.handled).toBe(true);
  expect(stripTerminalSequences(chat.renderHelper(entry, 80).join("\n"))).toContain("export { answer }");
  chat.invalidate();
  expect(stripTerminalSequences(chat.renderHelper(entry, 80).join("\n"))).toContain("export { answer }");
  expect(JSON.stringify(entry)).toBe(before);
});

test("incomplete argument JSON uses native generic rendering, never a built-in renderer with corrupt args", () => {
  const tui = new TuiAltScreen(new RecordingTerminal()); let resolved = 0;
  const chat = new HelperChat(() => tui, "/tmp", () => ({ ...style(1), renderers: () => { resolved++; return undefined; } }));
  const entry = fixture();
  const incomplete = { ...entry, messages: [{ id: "t", kind: "tool" as const, tool: "read", phase: "error" as const,
    arguments: '{"path":"truncated', text: "Arguments omitted by retention limit" }] };
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
  const rendered = chat.renderHelper(entry, 80).join("\n");
  expect(rendered).not.toContain("\x1b]52"); expect(rendered).not.toContain("\x1b[2J");
  expect(rendered).not.toContain("SECRET"); expect(rendered).not.toContain("CLIPBOARD");
  expect(rendered).toContain("\x1b["); expect(stripTerminalSequences(rendered)).toContain("Visible");
  expect(stripTerminalSequences(rendered)).toContain("safe.ts");
});
