import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { expertTool } from "../src/extensions/subagent/index.ts";
import { oracleTool } from "../src/extensions/oracle/index.ts";
import { RetainedChildren } from "../src/host/retained-children.ts";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { AuxiliaryHistory, HELPER_HISTORY_LIMIT, HELPER_TEXT_LIMIT } from "../src/extensions/subagent/inspection.ts";
import { observeHelperEvent } from "../src/extensions/subagent/session.ts";
import { AuxiliaryRuntime } from "../src/extensions/subagent/runtime.ts";
import * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { FleetRuntime } from "../src/extensions/fleet/runtime.ts";
import type { ChildHooks } from "../src/extensions/fleet/child.ts";

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
  const history = new AuxiliaryHistory(); history.begin("r", "explore", "fixture/model", "Research this");
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
  const history = new AuxiliaryHistory(); history.begin("r", "explore", "fixture/model", "Question");
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
  history.begin("cap", "oracle", "fixture/model", "Question");
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
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: "model", timeout_secs: 30 } }, provider: "fixture", model: "parent", sessionId: () => "parent",
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
  history.begin("r", "oracle", "fixture/model", "p".repeat(10000));
  for (let index = 0; index < 100; index++) history.observe("r", { type: "tool", id: String(index), name: "read", phase: "completed", text: "x".repeat(10000) });
  history.finish("r", "completed", "answer".repeat(10000));
  const entry = history.snapshot()[0];
  expect(entry?.truncated).toBeTrue(); expect(entry?.prompt.length).toBe(HELPER_TEXT_LIMIT);
  expect(entry?.result?.length).toBe(HELPER_TEXT_LIMIT);
  expect(entry?.messages.reduce((sum, message) => sum + message.text.length, 0)).toBeLessThanOrEqual(32768);
  history.observe("r", { type: "assistant_delta", text: "late" });
  expect(history.snapshot()[0]).toEqual(entry);
  for (let index = 0; index < HELPER_HISTORY_LIMIT; index++) history.begin(String(index), "oracle", "fixture/model", "next");
  history.observe("r", { type: "assistant_delta", text: "must not resurrect" });
  expect(history.snapshot()).toHaveLength(HELPER_HISTORY_LIMIT);
  expect(history.snapshot().some(value => value.request === "r")).toBeFalse();
});

test("live runtime inspection retains prompt, partials and result; queued cancellation never becomes provider execution", async () => {
  const started = barrier<ChildHooks>(), release = barrier(), queuedReady = barrier();
  const records: Record<string, unknown>[] = [];
  let executions = 0;
  const runtime = new FleetRuntime({ sdk: {} as typeof Sdk, models: {} as ModelRuntime, approved: ["fixture"],
    parent: { provider: "fixture", model: "model" }, agentDir: "/unused", localTools: [],
    context: { dir: "/unused", session: { worktree: "/repo", name: "parent" }, settings: { maxParallel: 1, profiles: [] } },
    allowedKinds: ["explore"],
    research: { settings: { model: null, max_parallel: 1, max_turns: 2, timeout_secs: 30 } },
    changed: () => { if (runtime.snapshot().children.some(entry => entry.brief === "Second prompt" && entry.state === "queued")) queuedReady.resolve(); },
    deliverToParent() { throw new Error("Blocking research must not be redelivered"); },
    record: async value => { records.push(value); return { session_id: value.request, session_file: "/unused/owned-reference" }; },
    runner: { run(_spec, _model, hooks) {
      executions++;
      hooks.onEvent({ type: "assistant_start" }); hooks.onEvent({ type: "assistant_delta", text: "Live partial" });
      started.resolve(hooks);
      return { done: release.promise.then(() => {
        hooks.onEvent({ type: "assistant_end", text: "Final answer" });
        return { report: "Final answer", researchReport: { status: "completed" as const, stopReason: "completed" as const } };
      }), usage: () => ({ input: 1, output: 1, turns: 1 }), async steer() {}, async setModel() { return "applied"; } };
    } },
  });
  try {
    const first = runtime.explore("one", "First prompt");
    const hooks = await started.promise;
    const cancel = new AbortController();
    const queued = runtime.explore("two", "Second prompt", cancel.signal);
    await queuedReady.promise;
    expect(runtime.snapshot().children.find(value => value.brief === "Second prompt")?.state).toBe("queued");
    expect(runtime.snapshot().children.find(value => value.brief === "First prompt")?.messages[0]?.text).toBe("Live partial");
    cancel.abort();
    const result = await queued;
    expect(result.report?.stopReason).toBe("cancelled");
    expect(result.conversationId).toBeUndefined();
    expect(runtime.snapshot().children.find(value => value.brief === "Second prompt")?.state).toBe("cancelled");
    expect(records.filter(value => value.request === result.request && value.action === "finish").map(value => value.outcome)).toEqual(["not_started"]);
    expect(executions).toBe(1);
    release.resolve(); await first;
    hooks.onEvent({ type: "assistant_delta", text: "Late callback" });
    expect(runtime.snapshot().children.find(value => value.brief === "First prompt")?.report).toBe("Final answer");
    expect(JSON.stringify(runtime.snapshot())).not.toContain("Late callback");
    expect(runtime.explore("one", "First prompt")).toBe(first);
    expect(runtime.snapshot().children).toHaveLength(2);
  } finally { release.resolve(); await runtime.close(); }
});

test("the ID returned by expert opens its retained transcript after the roster drops it", async () => {
  const dir = mkdtempSync(join(tmpdir(), "expert-readback-"));
  const kept = new RetainedChildren(dir);
  const records: Record<string, unknown>[] = [];
  const runtime = new AuxiliaryRuntime({ settings: { expert: { model: "model", max_parallel: 1, max_turns: 24, timeout_secs: 30 },
    oracle: { model: "model", timeout_secs: 30 } }, provider: "fixture", model: "parent", sessionId: () => "parent",
    retain: (helper, file, report) => kept.helper(helper, file, report),
    async record(value) { records.push(value); return { session_id: value.request, session_file: join(dir, `runtime/auxiliary/${value.request}.jsonl`) }; },
    async execute({ kind, sessionId, sessionFile }) {
      mkdirSync(dirname(sessionFile), { recursive: true });
      writeFileSync(sessionFile, JSON.stringify({ type: "session", version: Sdk.CURRENT_SESSION_VERSION,
        id: sessionId, timestamp: new Date().toISOString(), cwd: dir }) + "\n");
      if (kind === "oracle") throw new Error("Unrelated test helper failed. ".repeat(10));
      Sdk.SessionManager.open(sessionFile).appendMessage(assistant);
      return { text: "Visible answer", outcome: "completed", turns: 2 };
    },
  });
  try {
    const result = await expertTool(runtime).execute("parent-call", { question: "What did you find?" }, undefined, undefined, undefined as never);
    const id = (result.details as { conversationId: string }).conversationId;
    expect(id).toBe(records[0]?.request as string);
    expect(id).not.toBe("parent-call");
    expect(records.slice(0, 3).map(record => [record.action, record.outcome])).toEqual([
      ["prepare", undefined], ["started", undefined], ["finish", "completed"],
    ]);
    const oracle = oracleTool(runtime);
    // Exercise both bounds: live auxiliary observations keep 100, and the file roster is byte-bounded.
    for (let index = 0; index < 100; index++) {
      const reply = await oracle.execute(`evict-${index}`, { prompt: "New call" }, undefined, undefined, undefined as never);
      if (index === 0) {
        expect(reply.isError).toBe(true);
        expect(reply.details).toMatchObject({ conversationId: records[3]?.request });
        expect(records[5]?.outcome).toBe("uncertain");
      }
    }
    expect(runtime.inspections().some(helper => helper.request === id)).toBe(false);
    expect(runtime.inspection().some(helper => helper.request === id)).toBe(false);
    const read = (file: string) => JSON.parse(readFileSync(join(dir, file), "utf8"));
    expect(read("child-records/fleet").children.some((child: { id: string }) => child.id === id)).toBe(false);
    const retained = read(`child-records/${id}.json`);
    expect(retained.detail).toMatchObject({ id, kind: "helper:expert", state: "done", report: "Visible answer" });
    const transcript = Sdk.SessionManager.open(join(dir, retained.session_file));
    expect(transcript.getSessionId()).toBe(id);
    expect(transcript.getBranch()).toMatchObject([{ type: "message", message: assistant }]);
  } finally { await runtime.close(); rmSync(dir, { recursive: true, force: true }); }
});

test("a failed terminal record remains uncertain, not a successful helper or task", async () => {
  const runtime = new AuxiliaryRuntime({ settings: { oracle: { model: "model", timeout_secs: 30 } }, provider: "fixture", model: "parent", sessionId: () => "parent",
    record: async value => { if (value.action === "finish") throw new Error("record unavailable"); return { session_id: value.request, session_file: "/unused/reference" }; },
    execute: async () => ({ outcome: "completed", text: "Returned response" }),
  });
  await expect(runtime.run("oracle", "one", "Question")).rejects.toThrow("record unavailable");
  expect(runtime.inspections()[0]?.phase).toBe("uncertain"); expect(runtime.inspections()[0]?.error).toBe("record unavailable");
  expect(runtime.inspection()[0]).toMatchObject({ state: "uncertain", parent: "parent", sessionFile: "/unused/reference" });
  await runtime.close();
});
