import { expect, test } from "bun:test";
import type { Message, ToolResultMessage } from "@earendil-works/pi-ai";

import { ReplayCache } from "../src/claude/cache.ts";
import { buildPromptBlocks, type PromptBlock } from "../src/claude/prompt.ts";

const user = (content: string) => ({ role: "user" as const, content, timestamp: 0 });
const result = (content: ToolResultMessage["content"]): ToolResultMessage => ({ role: "toolResult",
  toolName: "read", toolCallId: "call", content, isError: false, timestamp: 0 });
const marker = (blocks: readonly PromptBlock[]) => blocks.findLastIndex((block) => "cache_control" in block);
const plain = (blocks: readonly PromptBlock[]) => blocks.map((block) => block.type === "text"
  ? { type: "text", text: block.text } : block);

test("parallel text results reach the newest cache boundary in one request", () => {
  const cache = new ReplayCache();
  const history = [user("Inspect"), result([{ type: "text", text: "first" }])];
  const first = cache.prepare(buildPromptBlocks([...history, user("continue")], new Map()), "text-batch");
  first.commit();
  const batch = Array.from({ length: 25 }, (_, index) => result([{ type: "text", text: `result ${index}` }]));
  const messages = [...history, user("continue"), ...batch, user("next")];
  const rendered = buildPromptBlocks(messages, new Map());
  const next = cache.prepare(rendered, "text-batch");
  expect(marker(next.blocks)).toBe(marker(rendered));
  expect(plain(next.blocks.slice(0, marker(first.blocks) + 1))).toEqual(plain(first.blocks.slice(0, marker(first.blocks) + 1)));
  const text = next.blocks.flatMap(block => block.type === "text" ? [block.text] : []).join("");
  expect(text.match(/TOOL RESULT/g)).toHaveLength(26);
});

test("a successful Claude frontier remains reachable across large parallel and image batches with one marker", () => {
  const cache = new ReplayCache();
  const render = (messages: Message[]) => buildPromptBlocks(messages, new Map());
  const history = [user("Inspect"), result([{ type: "text", text: "first" }])];
  const first = cache.prepare(render([...history, user("continue")]), "session-a");
  first.commit();
  const batch = Array.from({ length: 25 }, (_, index) => result([
    { type: "text", text: `result ${index}` }, { type: "image", mimeType: "image/png", data: "aW1hZ2U=" },
  ]));
  const messages = [...history, user("continue"), ...batch, user("next")];
  const end = marker(render(messages));
  let previous = first.blocks;
  for (let turn = 0; turn < 8; turn++) {
    const next = cache.prepare(render(messages), "session-a");
    const oldAt = marker(previous);
    const nextAt = marker(next.blocks);
    expect(next.blocks.filter((block) => "cache_control" in block)).toHaveLength(1);
    expect(nextAt).toBeGreaterThanOrEqual(oldAt);
    expect(nextAt - oldAt + 1).toBeLessThanOrEqual(20);
    expect(plain(next.blocks.slice(0, oldAt + 1))).toEqual(plain(previous.slice(0, oldAt + 1)));
    next.commit();
    previous = next.blocks;
  }
  expect(marker(previous)).toBe(end);
  // Unconfirmed requests do not advance the recorded write; scopes never borrow another session's frontier.
  const failureCache = new ReplayCache();
  const write = failureCache.prepare(first.blocks, "session-a");
  write.commit();
  const unconfirmed = failureCache.prepare(render(messages), "session-a");
  expect(failureCache.prepare(render(messages), "session-a").blocks).toEqual(unconfirmed.blocks);
  expect(marker(failureCache.prepare(render(messages), "session-b").blocks)).toBe(end);
  const compacted = render([user("summary"), result([{ type: "text", text: "new prefix" }]), user("go")]);
  expect(failureCache.prepare(compacted, "session-a").blocks).toEqual(compacted);
});
