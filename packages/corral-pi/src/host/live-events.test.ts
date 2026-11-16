import { expect, test } from "bun:test";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { EVENT_MAX_BYTES, EventLog } from "../bridge/events.ts";
import { EVENT_PAGE_BYTES } from "../bridge/protocol.ts";
import { LiveEvents } from "./live-events.ts";

const message: AssistantMessage = {
  role: "assistant", content: [], api: "openai-responses", provider: "openai", model: "fixture",
  timestamp: 0, stopReason: "stop",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
};
function start(live: LiveEvents): void {
  live.observe({ type: "agent_start" }, 7);
  live.observe({ type: "message_start", message }, 7);
}
function delta(live: LiveEvents, text: string): void {
  live.observe({ type: "message_update", message,
    assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: text, partial: message } }, 7);
}

test("live deltas do not evict or renumber events of the existing ring", () => {
  const lifecycle = new EventLog(3, 2);
  const live = new LiveEvents(3, 2);
  const input = lifecycle.emit("input", { idempotency_key: "owner" });
  start(live);
  for (let i = 0; i < 4; i++) delta(live, "answer");
  const settled = lifecycle.emit("input_settled", { idempotency_key: "owner" });
  const page = lifecycle.since(0, EVENT_PAGE_BYTES);
  expect(page.events).toEqual([input, settled]);
  expect(page.events.map(event => event.seq)).toEqual([1, 2]);
  expect(page.gap).toBeUndefined();
  expect(live.events.last).toBe(5);
});

test("a multi-byte character is never split across two live_text chunks", () => {
  const live = new LiveEvents(3);
  start(live);
  const text = "a".repeat(1023) + "💬" + "\u0000".repeat(2048);
  delta(live, text);
  delta(live, "é");
  const chunks = live.events.since(0, EVENT_PAGE_BYTES).events.filter(event => event.kind === "live_text");
  expect(chunks.length).toBeGreaterThan(1);
  expect(chunks.map(event => event.text).join("")).toBe(text + "é");
  let offset = 0;
  for (const event of chunks) {
    expect(event).toMatchObject({ turn: 7, message: 1, offset });
    expect(typeof event.text).toBe("string");
    const piece = String(event.text);
    expect(piece).not.toContain("\ufffd");
    expect(Buffer.byteLength(piece)).toBeLessThanOrEqual(4096);
    expect(Buffer.byteLength(JSON.stringify(event))).toBeLessThanOrEqual(EVENT_MAX_BYTES);
    offset += Buffer.byteLength(piece);
  }
});

test("a lagging live reader gets a gap marker instead of silent loss", () => {
  const live = new LiveEvents(3, 2);
  start(live);
  for (const text of ["a", "b", "c"]) delta(live, text);
  const page = live.events.since(0, EVENT_PAGE_BYTES, 1);
  expect(page.gap).toEqual({ after: 0, resumed: 3, missing: 2, reconcile: true });
  expect(page).toMatchObject({ generation: 3, last: 4, next: 3, more: true });
  expect(page.events.map(event => event.seq)).toEqual([3]);
  const next = live.events.since(page.next, EVENT_PAGE_BYTES);
  expect(next.events.map(event => event.seq)).toEqual([4]);
  expect(next.gap).toBeUndefined();
  expect(next.more).toBeFalse();
});

test("a completing live event never precedes its session-file entry", () => {
  const live = new LiveEvents(3);
  start(live);
  const result = { role: "toolResult" as const, toolCallId: "call", toolName: "read",
    content: [{ type: "text" as const, text: "done" }], isError: false, timestamp: 0 };
  live.observe({ type: "message_end", message }, 7);
  live.observe({ type: "tool_execution_end", toolCallId: "call", toolName: "read",
    result: { content: result.content }, isError: false, durationMs: 12 }, 7);
  live.observe({ type: "message_end", message: result }, 7);
  // Both public callbacks above run before Pi appends their message_end entry.
  expect(live.events.since(0, EVENT_PAGE_BYTES).events.some(event => event.state === "ended")).toBeFalse();
  // Pi awaits those appends before dispatching this barrier.
  live.observe({ type: "turn_end", message, toolResults: [result] }, 7);
  expect(live.events.since(0, EVENT_PAGE_BYTES).events.at(-1)).toMatchObject({
    kind: "live_tool", turn: 7, call: "call", tool: "read", state: "ended", failed: false, elapsed_ms: 12,
  });
  live.observe({ type: "agent_settled", aborted: false }, 7);
  expect(live.events.since(0, EVENT_PAGE_BYTES).events.at(-1)).toMatchObject({ kind: "live_turn", state: "ended" });

  const failedAppend = new LiveEvents(3);
  start(failedAppend);
  failedAppend.observe({ type: "message_end", message }, 7);
  failedAppend.observe({ type: "agent_settled", aborted: false }, 7);
  expect(failedAppend.events.since(0, EVENT_PAGE_BYTES).events.some(event => event.state === "ended")).toBeFalse();
});

test("fleet pages cap Unicode text and shed complete oldest messages before children", () => {
  const live = new LiveEvents(3);
  live.observeFleet({ children: [], maxParallel: 3, mail: Array.from({ length: 5 }, (_, index) => ({
    seq: index + 1, at: index, from: "parent", to: "all",
    text: "\0".repeat(2050), delivery: "delivered" as const,
  })) });
  const page = live.since(0, EVENT_PAGE_BYTES);
  expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(EVENT_PAGE_BYTES);
  expect(page.fleet?.messages_total).toBe(5);
  expect(page.fleet?.messages.length).toBeLessThanOrEqual(4);
  expect(page.fleet?.messages.at(-1)).toMatchObject({ seq: 5, text: "\0".repeat(2048) });
  live.observeFleet({ children: [], maxParallel: 3, mail: [{
    seq: 6, at: 6, from: "parent", to: "all", text: "💬".repeat(2050), delivery: "delivered",
  }] });
  expect(live.since(0, EVENT_PAGE_BYTES).fleet?.messages[0]?.text).toBe("💬".repeat(2048));
});
