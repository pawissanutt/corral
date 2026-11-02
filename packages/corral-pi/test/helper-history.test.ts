import { expect, test } from "bun:test";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { AuxiliaryHistory, HELPER_HISTORY_LIMIT, HELPER_TEXT_LIMIT } from "../src/extensions/subagent/inspection.ts";
import { observeHelperEvent } from "../src/extensions/subagent/session.ts";
import { AuxiliaryRuntime, type Invocation } from "../src/extensions/subagent/runtime.ts";

function barrier<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const assistant: AssistantMessage = { role: "assistant", api: "openai-responses", provider: "fixture", model: "fixture", timestamp: 1,
  content: [{ type: "thinking", thinking: "PRIVATE_REASONING", thinkingSignature: "PRIVATE_SIGNATURE" }, { type: "text", text: "Visible answer" }],
  stopReason: "stop", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };

test("SDK projection includes visible text and tool output, never thinking, signatures, image bytes or tool details", () => {
  const history = new AuxiliaryHistory(); history.begin("r", "explore", "fixture/model", 1, "Research this");
  const emit = (event: Parameters<AuxiliaryHistory["observe"]>[1]) => history.observe("r", event);
  observeHelperEvent({ type: "message_start", message: assistant }, emit);
  observeHelperEvent({ type: "message_update", message: assistant,
    assistantMessageEvent: { type: "thinking_delta", contentIndex: 0, delta: "PRIVATE_DELTA", partial: assistant } }, emit);
  observeHelperEvent({ type: "message_update", message: assistant,
    assistantMessageEvent: { type: "text_delta", contentIndex: 1, delta: "Visible partial", partial: assistant } }, emit);
  const before = history.snapshot(); expect(before[0]?.messages[0]?.text).toBe("Visible partial");
  observeHelperEvent({ type: "message_end", message: assistant }, emit);
  observeHelperEvent({ type: "message_end", message: { role: "toolResult", toolCallId: "read-1", toolName: "read", timestamp: 2,
    content: [{ type: "text", text: "File contents" }, { type: "image", data: "PRIVATE_IMAGE_BYTES", mimeType: "image/png" }],
    details: { secret: "PRIVATE_TOOL_DETAILS" }, isError: false } }, emit);
  const projected = history.snapshot();
  expect(projected[0]?.messages[0]?.text).toBe("Visible answer");
  expect(projected[0]?.messages[1]?.text).toBe("File contents");
  expect(JSON.stringify(projected)).not.toContain("PRIVATE_");
  expect(before[0]?.messages[0]?.text).toBe("Visible partial");
});

test("native text boundaries and response errors are projected safely, bounded, and replaced by final content", () => {
  const history = new AuxiliaryHistory(); history.begin("r", "explore", "fixture/model", 1, "Question");
  const emit = (event: Parameters<AuxiliaryHistory["observe"]>[1]) => history.observe("r", event);
  const partial: AssistantMessage = { ...assistant, stopReason: "pending", content: [...assistant.content, { type: "text", text: "Second block" }] };
  observeHelperEvent({ type: "message_start", message: partial }, emit);
  observeHelperEvent({ type: "message_update", message: partial,
    assistantMessageEvent: { type: "text_delta", contentIndex: 2, delta: "Second block", partial } }, emit);
  const before = history.snapshot(); expect(before[0]?.messages[0]?.textBlocks).toEqual(["Visible answer", "Second block"]);
  observeHelperEvent({ type: "message_end", message: { ...assistant, stopReason: "error", errorMessage: "Provider interrupted" } }, emit);
  const final = history.snapshot()[0]?.messages[0];
  expect(final?.textBlocks).toBeUndefined(); expect(final?.text).toBe("Visible answer");
  expect(final?.stopReason).toBe("error"); expect(final?.errorMessage).toBe("Provider interrupted");
  expect(before[0]?.messages[0]?.textBlocks).toEqual(["Visible answer", "Second block"]);
  history.begin("cap", "oracle", "fixture/model", 1, "Question");
  observeHelperEvent({ type: "message_end", message: { ...assistant, content: [...assistant.content,
    ...Array.from({ length: 20 }, () => ({ type: "text" as const, text: "x".repeat(5000) }))] } }, event => history.observe("cap", event));
  const capped = history.snapshot()[0];
  expect(capped?.truncated).toBe(true);
  expect(capped?.messages[0]?.textBlocks?.length).toBeLessThanOrEqual(16);
  expect(capped?.messages[0]?.textBlocks?.reduce((sum, text) => sum + text.length, 0)).toBeLessThanOrEqual(HELPER_TEXT_LIMIT);
  expect(JSON.stringify(history.snapshot())).not.toContain("PRIVATE_");
});

test("broken UI observers cannot abort projection, change canonical accounting, or retain a slot", async () => {
  expect(() => observeHelperEvent({ type: "message_end", message: assistant }, () => { throw new Error("observer failed"); })).not.toThrow();
  const terminal: unknown[] = [];
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: "model", timeout_secs: 30 } }, generation: 1, provider: "fixture", model: "parent", sessionId: () => "parent",
    changed: () => { throw new Error("renderer failed"); },
    record: async value => { if (value.action === "finish") terminal.push(value.outcome); return { session_id: value.request, session_file: "/unused/reference" }; },
    execute: async invocation => { invocation.observe?.({ type: "assistant_delta", text: "Live text" }); return { outcome: "completed", text: "Final answer" }; },
  });
  await runtime.run("oracle", "one", "First"); await runtime.run("oracle", "two", "Second");
  expect(terminal).toEqual(["completed", "completed"]);
  expect(runtime.state().oracle).toEqual({ running: 0, queued: 0 });
  expect(runtime.inspections().every(entry => entry.phase === "completed" && entry.inspectionError?.includes("renderer failed"))).toBeTrue();
  await runtime.close();
});

test("history is bounded and completed entries ignore late callbacks without resurrecting evicted records", () => {
  const history = new AuxiliaryHistory();
  history.begin("r", "oracle", "fixture/model", 1, "p".repeat(10000));
  for (let index = 0; index < 100; index++) history.observe("r", { type: "tool", id: String(index), name: "read", phase: "completed", text: "x".repeat(10000) });
  history.finish("r", "completed", "answer".repeat(10000));
  const entry = history.snapshot()[0];
  expect(entry?.truncated).toBeTrue(); expect(entry?.prompt.length).toBe(HELPER_TEXT_LIMIT);
  expect(entry?.result?.length).toBe(HELPER_TEXT_LIMIT);
  expect(entry?.messages.reduce((sum, message) => sum + message.text.length, 0)).toBeLessThanOrEqual(32768);
  history.observe("r", { type: "assistant_delta", text: "late" });
  expect(history.snapshot()[0]).toEqual(entry);
  for (let index = 0; index < HELPER_HISTORY_LIMIT; index++) history.begin(String(index), "oracle", "fixture/model", 1, "next");
  history.observe("r", { type: "assistant_delta", text: "must not resurrect" });
  expect(history.snapshot()).toHaveLength(HELPER_HISTORY_LIMIT);
  expect(history.snapshot().some(value => value.request === "r")).toBeFalse();
});

test("live runtime inspection retains prompt, partials and result; queued cancellation never becomes provider execution", async () => {
  const started = barrier<Invocation>(), release = barrier(), queuedReady = barrier();
  const runtime = new AuxiliaryRuntime({ settings: { explore: { model: null, max_parallel: 1, max_turns: 2, timeout_secs: 30 } },
    generation: 4, provider: "fixture", model: "model", sessionId: () => "parent",
    changed: () => { if (runtime.inspections().some(entry => entry.prompt === "Second prompt" && entry.phase === "queued")) queuedReady.resolve(); },
    record: async value => ({ session_id: value.request, session_file: "/unused/owned-reference" }),
    execute: async invocation => {
      invocation.observe?.({ type: "assistant_start" }); invocation.observe?.({ type: "assistant_delta", text: "Live partial" });
      started.resolve(invocation); await release.promise;
      invocation.observe?.({ type: "assistant_end", text: "Final answer" }); return { text: "Final answer", outcome: "completed" };
    },
  });
  const first = runtime.run("explore", "one", "First prompt");
  const invocation = await started.promise;
  const cancel = new AbortController();
  const queued = runtime.run("explore", "two", "Second prompt", cancel.signal);
  const rejected = queued.catch(error => error);
  await queuedReady.promise;
  expect(runtime.inspections().find(value => value.prompt === "Second prompt")?.phase).toBe("queued");
  expect(runtime.inspections().find(value => value.prompt === "First prompt")?.messages[0]?.text).toBe("Live partial");
  cancel.abort(); await rejected;
  expect(runtime.inspections().find(value => value.prompt === "Second prompt")?.phase).toBe("not_started");
  release.resolve(); await first;
  invocation.observe?.({ type: "assistant_delta", text: "Late callback" });
  expect(runtime.inspections().find(value => value.prompt === "First prompt")?.result).toBe("Final answer");
  expect(JSON.stringify(runtime.inspections())).not.toContain("Late callback");
  expect(runtime.run("explore", "one", "First prompt")).toBe(first);
  expect(runtime.inspections()).toHaveLength(2); await runtime.close();
});

test("a failed terminal record remains uncertain, not a successful helper or task", async () => {
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: "model", timeout_secs: 30 } }, generation: 1, provider: "fixture", model: "parent", sessionId: () => "parent",
    record: async value => { if (value.action === "finish") throw new Error("record unavailable"); return { session_id: value.request, session_file: "/unused/reference" }; },
    execute: async () => ({ outcome: "completed", text: "Returned response" }),
  });
  await expect(runtime.run("oracle", "one", "Question")).rejects.toThrow("record unavailable");
  expect(runtime.inspections()[0]?.phase).toBe("uncertain"); expect(runtime.inspections()[0]?.error).toBe("record unavailable");
  await runtime.close();
});
