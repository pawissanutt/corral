import { expect, test } from "bun:test";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import { getModel } from "@earendil-works/pi-ai/compat";
import { applyStreamEvent, emptyOutput, type EventContext } from "./events.ts";

function context(): EventContext {
  const model = getModel("anthropic", "claude-sonnet-5-5");
  if (!model) throw new Error("missing model fixture");
  return { model, output: emptyOutput(model), blocks: [], argumentsStreams: new Map(),
    stream: createAssistantMessageEventStream(), customToolNameToPi: new Map() };
}

test("Claude tool arguments grow in delta events and map at completion", async () => {
  const ctx = context();
  const events = ctx.stream[Symbol.asyncIterator]();
  applyStreamEvent(ctx, { type: "content_block_start", index: 0,
    content_block: { type: "tool_use", id: "call", name: "Write" } });
  await events.next();
  const chunks = ['{"file_path":"a.txt","con', 'tent":"hel', 'lo"}'];
  const expected = [{ path: "a.txt" }, { path: "a.txt", content: "hel" }, { path: "a.txt", content: "hello" }];
  for (const [index, partial_json] of chunks.entries()) {
    applyStreamEvent(ctx, { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json } });
    const event = (await events.next()).value;
    expect(event?.type).toBe("toolcall_delta");
    if (event?.type !== "toolcall_delta") throw new Error("expected toolcall_delta");
    const block = event.partial.content[event.contentIndex];
    expect(block?.type).toBe("toolCall");
    if (block?.type !== "toolCall") throw new Error("expected tool call");
    expect(block.arguments).toEqual(expected[index]);
  }
  applyStreamEvent(ctx, { type: "content_block_stop", index: 0 });
  const event = (await events.next()).value;
  expect(event?.type).toBe("toolcall_end");
  if (event?.type !== "toolcall_end") throw new Error("expected toolcall_end");
  expect(event.toolCall.arguments).toEqual({ path: "a.txt", content: "hello" });
  expect(event.toolCall.argumentsError).toBeUndefined();
});

test("truncated Claude tool arguments carry an error instead of a repaired prefix", async () => {
  const ctx = context();
  const events = ctx.stream[Symbol.asyncIterator]();
  applyStreamEvent(ctx, { type: "content_block_start", index: 0,
    content_block: { type: "tool_use", id: "call", name: "Write" } });
  await events.next();
  const partial_json = '{"file_path":"a.txt","content":"hel';
  applyStreamEvent(ctx, { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json } });
  await events.next();
  applyStreamEvent(ctx, { type: "content_block_stop", index: 0 });
  const event = (await events.next()).value;
  if (event?.type !== "toolcall_end") throw new Error("expected toolcall_end");
  expect(event.toolCall.arguments).toEqual({});
  expect(event.toolCall.argumentsError).toContain(partial_json);
});
