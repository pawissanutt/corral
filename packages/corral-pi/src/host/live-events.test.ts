import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { EVENT_MAX_BYTES, EventLog } from "../bridge/events.ts";
import { EVENT_PAGE_BYTES } from "../bridge/protocol.ts";
import { LiveEvents } from "./live-events.ts";
import { MARK_ENTRY, observeTranscriptEntries } from "./transcript-marks.ts";

const message: AssistantMessage = {
  role: "assistant", content: [], api: "openai-responses", provider: "openai", model: "fixture",
  timestamp: 0, stopReason: "stop",
  usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
};
function start(live: LiveEvents, leaf: string | null = null): void {
  live.observe({ type: "agent_start" }, 7, leaf);
  live.observe({ type: "message_start", message }, 7);
}
function delta(live: LiveEvents, text: string): void {
  live.observe({ type: "message_update", message,
    assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: text, partial: message } }, 7);
}

test("host notice IDs are unique across host instances", () => {
  const first = new LiveEvents(), second = new LiveEvents();
  try {
    const now = Date.now();
    first.note("error", "failure", now);
    second.note("error", "failure", now);
    expect(first.notices[0]?.id).toBeDefined();
    expect(first.notices[0]?.id).not.toBe(second.notices[0]?.id);
  } finally { first.close(); second.close(); }
});

test("live deltas do not evict or renumber events of the existing ring", () => {
  const lifecycle = new EventLog(2);
  const live = new LiveEvents(2);
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
  const live = new LiveEvents();
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
  const live = new LiveEvents(2);
  start(live);
  for (const text of ["a", "b", "c"]) delta(live, text);
  const page = live.events.since(0, EVENT_PAGE_BYTES, 1);
  expect(page.gap).toEqual({ after: 0, resumed: 3, missing: 2, reconcile: true });
  expect(page).toMatchObject({ last: 4, next: 3, more: true });
  expect(page.events.map(event => event.seq)).toEqual([3]);
  const next = live.events.since(page.next, EVENT_PAGE_BYTES);
  expect(next.events.map(event => event.seq)).toEqual([4]);
  expect(next.gap).toBeUndefined();
  expect(next.more).toBeFalse();
});

// A reader that joined a long reply got only the ring's last chunks, at offsets it could not place.
test("a reader behind the ring is told where the open turn is and what was written", () => {
  const live = new LiveEvents(2);
  start(live);
  for (const text of ["one ", "two ", "three"]) delta(live, text);
  const first = live.since(0, EVENT_PAGE_BYTES).open!;
  expect(first).toEqual({
    turn: 7, message: 1, ended: false, message_at: first.message_at,
    journal: { message: 0, entry_id: null },
    text_offset: 0, text: "one two three", thinking_offset: 0, thinking: "",
  });
  // The journal stamps the message's record on this clock, after this.
  expect(Date.parse(first.message_at!)).toBeLessThanOrEqual(Date.now());
  expect(live.since(live.events.last, EVENT_PAGE_BYTES).open).toBeUndefined();
  delta(live, "é".repeat(20_000));
  const long = live.since(0, EVENT_PAGE_BYTES);
  expect(long.open!.text_offset + Buffer.byteLength(long.open!.text)).toBe(13 + 40_000);
  expect(long.open!.text).toMatch(/^é+$/);
  expect(Buffer.byteLength(JSON.stringify(long))).toBeLessThanOrEqual(EVENT_PAGE_BYTES);
  live.observe({ type: "message_end", message }, 7);
  expect(live.since(0, EVENT_PAGE_BYTES).open).toMatchObject({ message: 1, ended: true, text: "" });
  live.observe({ type: "turn_end", message, toolResults: [] }, 7);
  live.observe({ type: "agent_settled", aborted: false }, 7);
  expect(live.since(0, EVENT_PAGE_BYTES).open).toBeUndefined();
});

// The open turn was budgeted against the events only: beside a full fleet and state the reply
// passed the bridge's frame bound, and the reader that needed it could not read at all.
test("a joining reader's page fits the frame with a full fleet, state and open turn", () => {
  const live = new LiveEvents(2);
  live.observeFleet({ children: [], maxParallel: 3, mail: Array.from({ length: 5 }, (_, index) => ({
    seq: index + 1, at: index, from: "parent", to: "all",
    text: "\0".repeat(2050), delivery: "delivered" as const,
  })) });
  live.bindState(() => ({ parts: {}, state: { notices: [{ id: "n", tone: "info", text: "x".repeat(11_000) }],
    queue: [], cards: [] } as never }));
  const leaf = "\0".repeat(128); // An imported entry ID at the limit, with maximum JSON escaping.
  start(live, leaf);
  // Control characters are six bytes each in JSON, line breaks two.
  delta(live, "\u0001".repeat(1750) + "\n".repeat(6442));
  const page = live.since(0, EVENT_PAGE_BYTES);
  expect(page.fleet).toBeDefined();
  expect(page.state).toBeDefined();
  expect(page.events.length).toBeGreaterThan(0);
  expect(page.open?.journal).toEqual({ message: 0, entry_id: leaf });
  expect(page.open!.text_offset + Buffer.byteLength(page.open!.text)).toBe(8192);
  expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(EVENT_PAGE_BYTES);

  live.observe({ type: "agent_start" }, 8, "é".repeat(65));
  const oversized = live.since(0, EVENT_PAGE_BYTES);
  expect(oversized.open?.journal).toBeUndefined();
  expect(oversized.events.at(-1)).toMatchObject({ kind: "live_turn", state: "started", turn: 8 });
});

test("a reader's first page says where the open turn is without a gap", () => {
  const live = new LiveEvents();
  live.observe({ type: "agent_start" }, 7);
  // Before the first message the turn's own start parts its records from earlier ones.
  const waiting = live.since(0, EVENT_PAGE_BYTES).open!;
  expect(waiting).toMatchObject({ turn: 7, message: 0, ended: true });
  expect(Date.parse(waiting.message_at!)).toBeLessThanOrEqual(Date.now());
  live.observe({ type: "message_start", message }, 7);
  delta(live, "so far");
  const page = live.since(0, EVENT_PAGE_BYTES);
  expect(page.gap).toBeUndefined();
  expect(page.open).toMatchObject({ turn: 7, message: 1, ended: false, text_offset: 0, text: "so far" });
});

test("only the pending assistant reply gets a journal marker and anchors after a successful append", () => {
  const live = new LiveEvents();
  const dir = mkdtempSync(join(tmpdir(), "corral-live-reply-"));
  const manager = SessionManager.create(dir, dir);
  const file = manager.getSessionFile()!;
  const readJournal = () => readFileSync(file, "utf8");
  const recorded = (id: string) => readJournal().trimEnd().split("\n").map(line => JSON.parse(line)).find(entry => entry.id === id);
  const base = manager.appendCustomEntry("fixture", {});
  const next = { ...message }, failed = { ...message };
  let advanceLeaf = false, failAppend = false;
  const append = manager.appendMessage.bind(manager);
  manager.appendMessage = reply => {
    if (failAppend) throw new Error("append failed");
    const id = append(reply);
    // Simulate an append wrapper that advances the leaf again before returning the reply ID.
    if (advanceLeaf) manager.appendCustomEntry(MARK_ENTRY, {});
    return id;
  };
  const unobserve = observeTranscriptEntries(manager, () => live.entry(), () => {}, (reply, id) => {
    if (reply.role === "assistant") live.appendedAssistant(reply, id);
  }, reply => live.replyMarker(reply));
  try {
    start(live, manager.getLeafId());
    const started = { turn: 7, journal: { message: 0, entry_id: base } };
    expect(live.since(0, EVENT_PAGE_BYTES).events[0]).toMatchObject({ kind: "live_turn", state: "started", ...started });
    live.observe({ type: "message_end", message }, 7);
    // A different assistant object is not this reply, even if it has the same words
    // and carries a marker inherited from a previously persisted message.
    const unrelated = { ...message, corralReply: { turn: 7, message: 2 } };
    const unrelatedId = manager.appendMessage(unrelated);
    expect(recorded(unrelatedId)?.message).toEqual(message);
    expect(Object.hasOwn(unrelated, "corralReply")).toBeTrue();
    expect(live.since(0, EVENT_PAGE_BYTES).open).toMatchObject(started);
    expect(live.events.last).toBe(1);

    advanceLeaf = true;
    const id = manager.appendMessage(message);
    advanceLeaf = false;
    const journal = { message: 1, entry_id: id };
    expect(recorded(id)?.message).toEqual({ ...message, corralReply: { turn: 7, message: 1 } });
    expect(Object.hasOwn(message, "corralReply")).toBeFalse();
    expect(manager.getLeafId()).not.toBe(id);
    const entries = live.events.since(1, EVENT_PAGE_BYTES).events;
    expect(entries).toHaveLength(2);
    expect(entries[0]).toMatchObject({ kind: "live_entry" });
    expect(entries[0]?.turn).toBeUndefined();
    expect(entries[0]?.journal).toBeUndefined();
    expect(entries[1]).toMatchObject({ kind: "live_entry", journal_turn: 7, journal });

    const result = { role: "toolResult" as const, toolCallId: "call", toolName: "read",
      content: [], isError: false, timestamp: 0 };
    live.observe({ type: "message_end", message: result }, 7);
    manager.appendMessage(result);
    live.observe({ type: "turn_end", message, toolResults: [result] }, 7);
    live.observe({ type: "message_start", message: next }, 7);
    expect(live.since(0, EVENT_PAGE_BYTES).open).toMatchObject({ message: 2, ended: false, journal });
    live.observe({ type: "message_end", message: next }, 7);
    const nextId = manager.appendMessage(next);
    expect(recorded(nextId)?.message).toEqual({ ...next, corralReply: { turn: 7, message: 2 } });
    const latest = { message: 2, entry_id: nextId };
    expect(live.events.since(3, EVENT_PAGE_BYTES).events).toMatchObject([{ kind: "live_entry", journal_turn: 7, journal: latest }]);
    live.observe({ type: "turn_end", message: next, toolResults: [] }, 7);
    live.observe({ type: "message_start", message: failed }, 7);
    live.observe({ type: "message_end", message: failed }, 7);
    const cursor = live.events.last, beforeFailure = readJournal();
    failAppend = true;
    expect(() => manager.appendMessage(failed)).toThrow("append failed");
    expect(Object.hasOwn(failed, "corralReply")).toBeFalse();
    expect(readJournal()).toBe(beforeFailure);
    expect(live.events.last).toBe(cursor);
    expect(live.since(0, EVENT_PAGE_BYTES).open).toMatchObject({ message: 3, ended: true, journal: latest });

    live.observe({ type: "agent_start" }, 8, manager.getLeafId());
    expect(live.since(0, EVENT_PAGE_BYTES).open).toMatchObject({
      turn: 8, message: 0, journal: { message: 0, entry_id: nextId },
    });
  } finally {
    unobserve();
    live.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a completing live event never precedes its session-file entry", () => {
  const live = new LiveEvents();
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

  const failedAppend = new LiveEvents();
  start(failedAppend);
  failedAppend.observe({ type: "message_end", message }, 7);
  failedAppend.observe({ type: "agent_settled", aborted: false }, 7);
  expect(failedAppend.events.since(0, EVENT_PAGE_BYTES).events.some(event => event.state === "ended")).toBeFalse();
});

test("fleet pages cap Unicode text and shed complete oldest messages before children", () => {
  const live = new LiveEvents();
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
