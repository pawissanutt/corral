import { expect, test } from "bun:test";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Message, ToolResultMessage } from "@earendil-works/pi-ai";

import { ReplayCache } from "../src/claude/cache.ts";
import { buildPromptBlocks, type PromptBlock } from "../src/claude/prompt.ts";
import { requestCheckpoint } from "../src/host/request-checkpoint.ts";
import { summarize, summaryText } from "../src/host/request.ts";
import { ProgressSchema } from "../src/ui/todo-widget.ts";

const user = (content: string) => ({ role: "user" as const, content, timestamp: 0 });
const progress = (revision: number) => ProgressSchema.parse({ plan: "P1", revision, spec_revision: "assistant:A-1",
  phases: [{ id: "p", label: "Work", steps: [{ id: "a", label: "Inspect", status: { state: "in_progress" } }] }] });
const result = (content: ToolResultMessage["content"]): ToolResultMessage => ({ role: "toolResult",
  toolName: "read", toolCallId: "call", content, isError: false, timestamp: 0 });
const marker = (blocks: readonly PromptBlock[]) => blocks.findLastIndex((block) => "cache_control" in block);
const plain = (blocks: readonly PromptBlock[]) => blocks.map((block) => block.type === "text"
  ? { type: "text", text: block.text } : block);

test("checkpoints repair effective visibility, never replace ordinary canonical results or bless uncertainty", () => {
  const current = progress(2);
  const summary = summarize(current, null);
  if (!summary) throw new Error("missing summary");
  const text = summaryText(summary);
  const initial: AgentMessage[] = [user("Inspect")];
  const checkpoint = requestCheckpoint(initial, current, text, false, false);
  expect(checkpoint?.role).toBe("custom");
  if (!checkpoint) throw new Error("missing checkpoint");
  expect(requestCheckpoint([...initial, checkpoint], current, text, false, false)).toBeUndefined();
  // A compaction, rewind or new host that cannot see the record must restore it, even at the same revision.
  expect(requestCheckpoint(initial, current, text, false, false)?.content).toBe(checkpoint.content);
  expect(requestCheckpoint([{ role: "compactionSummary", summary: "Earlier work", tokensBefore: 10_000,
    timestamp: 0 }], current, text, false, false)?.content).toBe(checkpoint.content);
  const receipt: ToolResultMessage = { ...result([{ type: "text", text: JSON.stringify({ state: "committed",
    result: { progress: current } }) }]), toolName: "corral_todo" };
  expect(requestCheckpoint([...initial, receipt], current, text, false, false)).toBeUndefined();
  expect(requestCheckpoint([...initial, { ...receipt, toolName: "untrusted" }], current, text, false, false)).toBeDefined();
  // Details that the model cannot see are insufficient, as are older receipts or hidden Code Mode changes.
  expect(requestCheckpoint([...initial, { ...receipt, content: [], details: { result: { progress: current } } }],
    current, text, false, false)).toBeDefined();
  expect(requestCheckpoint([...initial, receipt], progress(3), "new revision", false, false)).toBeDefined();
  const unavailable = requestCheckpoint([...initial, receipt], current, text, true, false);
  expect(unavailable).toBeDefined();
  if (!unavailable) throw new Error("missing uncertainty checkpoint");
  expect(requestCheckpoint([...initial, unavailable], current, text, true, false)).toBeUndefined();
  expect(requestCheckpoint([...initial, checkpoint, unavailable], current, text, false, false)).toBeDefined();
  expect(requestCheckpoint([...initial, receipt], current, "stopped", false, true)).toBeDefined();
  expect(requestCheckpoint(initial, null, undefined, false, false)).toBeUndefined();
  expect(requestCheckpoint([...initial, checkpoint], null, undefined, false, false)).toBeDefined();
});

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
