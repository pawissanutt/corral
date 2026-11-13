import { expect, test } from "bun:test";
import { AuxiliaryHistory, HELPER_TEXT_LIMIT, type HelperEvent } from "./inspection.ts";

test("retained size matches messages through deltas, replacement and eviction", () => {
  const history = new AuxiliaryHistory();
  history.begin("r", "explore", "fixture/model", 1, "Question");
  const retained = Reflect.get(history, "entries") as Map<string, { size: number }>;
  const observe = (event: HelperEvent) => {
    history.observe("r", event);
    const messages = history.snapshot()[0]!.messages;
    const expected = messages.reduce((total, message) => total
      + Math.max(message.text.length, message.textBlocks?.reduce((sum, text) => sum + text.length, 0) ?? 0)
      + (message.errorMessage?.length ?? 0) + (message.arguments?.length ?? 0) + (message.tool?.length ?? 0), 0);
    expect(retained.get("r")!.size).toBe(expected);
    expect(expected).toBeLessThanOrEqual(32768);
    expect(messages.length).toBeLessThanOrEqual(64);
  };
  observe({ type: "assistant_start" });
  for (let index = 0; index < 100; index++) observe({ type: "assistant_delta", text: "part" });
  observe({ type: "assistant_delta", text: "x", textBlocks: ["first block", "second block"] });
  observe({ type: "assistant_end", text: "final", errorMessage: "interrupted", stopReason: "error" });
  observe({ type: "assistant_end", text: "replacement", textBlocks: ["one", "two"] });
  for (let index = 0; index < 70; index++) {
    observe({ type: "tool", id: String(index), name: "write", phase: "requested", arguments: "{}", argsComplete: false });
  }
  for (let index = 60; index < 70; index++) {
    observe({ type: "tool", id: String(index), name: "write", phase: "running", text: "x".repeat(HELPER_TEXT_LIMIT), partial: true });
  }
  observe({ type: "tool", id: "69", name: "write", phase: "completed", text: "done", partial: false });
  observe({ type: "assistant_delta", text: "An evicted assistant is replaced" });
  expect(history.snapshot()[0]!.truncated).toBe(true);
});

test("history subscriptions are synchronous, isolated and released on disposal", () => {
  let original = 0;
  let healthy = 0;
  let released = 0;
  const history = new AuxiliaryHistory(() => { original++; });
  history.subscribe(() => { throw new Error("broken observer"); });
  history.subscribe(() => { healthy++; });
  const release = history.subscribe(() => { released++; });
  history.begin("r", "explore", "fixture/model", 1, "Question");
  expect([original, healthy, released]).toEqual([1, 1, 1]);
  expect(history.snapshot()[0]!.inspectionError).toContain("broken observer");
  release(); release();
  history.observe("r", { type: "assistant_delta", text: "reply" });
  expect([original, healthy, released]).toEqual([2, 2, 1]);
  history.dispose();
  history.subscribe(() => { healthy++; });
  history.observe("r", { type: "assistant_delta", text: "late" });
  expect([original, healthy, released]).toEqual([2, 2, 1]);
});

test("latest activity survives transcript head clipping and stays a bounded visible line", () => {
  const history = new AuxiliaryHistory();
  history.begin("r", "explore", "fixture/model", 1, "Question");
  history.observe("r", { type: "assistant_start" });
  history.observe("r", { type: "assistant_delta", text: "old".repeat(2000) });
  history.observe("r", { type: "assistant_delta", text: "\nLatest reply" });
  history.observe("r", { type: "assistant_delta", text: " continues" });
  expect(history.snapshot()[0]?.latestActivity).toBe("Latest reply continues");
  expect(history.snapshot()[0]?.messages[0]?.text.includes("Latest")).toBe(false);
  history.observe("r", { type: "assistant_end", text: `${"old".repeat(2000)}\nFinal reply` });
  expect(history.snapshot()[0]?.latestActivity).toBe("Final reply");
  history.observe("r", { type: "tool", id: "one", name: "read", phase: "running", text: `${"old".repeat(2000)}\nResult tail` });
  expect(history.snapshot()[0]?.latestActivity).toBe("read: Result tail");
  history.observe("r", { type: "tool", id: "one", name: "read", phase: "completed", text: `first\n${"tail".repeat(100)} ending\n` });
  const latest = history.snapshot()[0]?.latestActivity ?? "";
  expect(latest.startsWith("read: ")).toBe(true);
  expect(latest.endsWith(" ending")).toBe(true);
  expect(latest.includes("\n")).toBe(false);
  expect(latest.length).toBeLessThanOrEqual(200);
});
