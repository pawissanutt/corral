import { expect, test } from "bun:test";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { AgentSessionEvent } from "@earendil-works/pi-coding-agent";
import { AuxiliaryHistory, HELPER_TEXT_LIMIT, type HelperEvent } from "./inspection.ts";
import { observeHelperEvent } from "./session.ts";

function assistant(content: string, id = "write-1"): AssistantMessage {
  return { role: "assistant", api: "openai-responses", provider: "fixture", model: "fixture", timestamp: 1,
    content: [{ type: "toolCall", id, name: "write", arguments: { path: "src/a.ts", content } }], stopReason: "toolUse",
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
}

test("a child call grows in one history entry, then streams its result tail before settlement", () => {
  const history = new AuxiliaryHistory();
  history.begin("r", "explore", "fixture/model", "Write a file");
  const events: HelperEvent[] = [];
  const emit = (event: HelperEvent) => { events.push(event); history.observe("r", event); };
  let now = 0;
  const observe = (event: AgentSessionEvent) => observeHelperEvent(event, emit, () => now);
  const tool = () => {
    const messages = history.snapshot()[0]!.messages.filter(message => message.kind === "tool");
    expect(messages).toHaveLength(1);
    return messages[0]!;
  };
  const update = (type: "toolcall_start" | "toolcall_delta", content: string, id = "write-1") => {
    const message = assistant(content, id);
    observe({ type: "message_update", message, assistantMessageEvent: type === "toolcall_start"
      ? { type, contentIndex: 0, partial: message }
      : { type, contentIndex: 0, delta: content, partial: message } });
  };
  observe({ type: "message_start", message: assistant("") });
  update("toolcall_start", "");
  const id = tool().id;
  expect(tool()).toMatchObject({ phase: "requested", argsComplete: false });
  expect(JSON.parse(tool().arguments!).content).toBe("");
  now = 50; update("toolcall_delta", "a");
  expect(JSON.parse(tool().arguments!).content).toBe("");
  now = 100; update("toolcall_delta", "ab");
  expect(JSON.parse(tool().arguments!).content).toBe("ab");
  now = 200; update("toolcall_delta", "abc");
  expect(JSON.parse(tool().arguments!).content).toBe("abc");
  now = 201;
  observe({ type: "message_end", message: assistant("abcd") });
  expect(tool()).toMatchObject({ id, phase: "requested", argsComplete: true });
  expect(JSON.parse(tool().arguments!).content).toBe("abcd");
  observe({ type: "tool_execution_start", toolCallId: "write-1", toolName: "write", args: {} });
  expect(tool().phase).toBe("running");
  const text = "old output\n".repeat(1000) + "newest output";
  observe({ type: "tool_execution_update", toolCallId: "write-1", toolName: "write", args: {},
    partialResult: { content: [{ type: "text", text: "earlier block" }, { type: "text", text }], details: { secret: "private" } } });
  expect(tool()).toMatchObject({ id, phase: "running", partial: true, text: text.slice(-HELPER_TEXT_LIMIT) });
  now = 250;
  observe({ type: "tool_execution_update", toolCallId: "write-1", toolName: "write", args: {},
    partialResult: { content: [{ type: "text", text: "throttled" }] } });
  expect(tool().text.endsWith("newest output")).toBe(true);
  observe({ type: "tool_execution_end", toolCallId: "write-1", toolName: "write", isError: false,
    result: { content: [{ type: "text", text: "File written" }] } });
  expect(tool()).toMatchObject({ id, phase: "completed", partial: false, text: "File written", argsComplete: true });
  const requested = events.filter(event => event.type === "tool" && event.phase === "requested");
  expect(requested).toHaveLength(4);
  expect(events.filter(event => event.type === "tool" && event.partial)).toHaveLength(1);
});

test("large write arguments are clipped before serialization and throttles are per call", () => {
  const events: HelperEvent[] = [];
  const emit = (event: HelperEvent) => { events.push(event); };
  let now = 0;
  const stringify = JSON.stringify;
  const serialized: unknown[] = [];
  JSON.stringify = (value: unknown) => { serialized.push(value); return stringify(value); };
  try {
    for (const [id, at] of [["a", 0], ["a", 50], ["b", 50], ["a", 100]] as const) {
      now = at;
      const message = assistant("\n\"\\".repeat(35000).slice(0, 100 * 1024), id);
      observeHelperEvent({ type: "message_update", message,
        assistantMessageEvent: { type: "toolcall_delta", contentIndex: 0, delta: "more", partial: message } }, emit, () => now);
    }
    const message = assistant("x".repeat(100 * 1024), "a");
    observeHelperEvent({ type: "message_end", message }, emit, () => now);
    for (const value of serialized) {
      expect(typeof value === "object").toBe(false);
      if (typeof value === "string") expect(value.length).toBeLessThanOrEqual(HELPER_TEXT_LIMIT);
    }
  } finally { JSON.stringify = stringify; }
  const requested = events.filter(event => event.type === "tool" && event.arguments !== undefined);
  expect(requested).toHaveLength(4);
  for (const event of requested) if (event.type === "tool") expect(event.arguments!.length).toBeLessThanOrEqual(HELPER_TEXT_LIMIT);
});
