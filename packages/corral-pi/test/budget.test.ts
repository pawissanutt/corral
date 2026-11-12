import { expect, test } from "bun:test";
import { normalizeContext, createAssistantMessageEventStream, type Api, type AssistantMessage, type Model, type StreamFunction } from "@earendil-works/pi-ai";
import { generateSummaryWithUsage } from "@earendil-works/pi-coding-agent";
import { RunBudget, reservation } from "../src/capabilities/budget.ts";

const model: Model<Api> = { id: "fixture", provider: "fixture", api: "openai-responses", baseUrl: "https://invalid.example",
  name: "fixture", input: ["text"], reasoning: false, contextWindow: 20, maxTokens: 5,
  cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 0.5 } };
const message = (responseId: string): AssistantMessage => ({ role: "assistant", api: model.api, provider: model.provider,
  model: model.id, content: [], timestamp: 1, stopReason: "stop", responseId,
  usage: { input: 4, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 6,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } });
function barrier() { let resolve!: () => void; const promise = new Promise<void>((r) => { resolve = r; }); return { promise, resolve }; }

const context = normalizeContext({ messages: [] });
for (const mode of ["tokens", "USD"]) test(`${mode}: concurrent main/summary/explore/oracle cancel, persist uncertainty, adapt limits only on explicit resume`, async () => {
  // Wire-shaped deterministic authority. Rust fixtures exercise the real transactional authority.
  const bound = reservation(model, 5);
  let cap = mode === "tokens" ? 125 : bound.usd_nanos * 5;
  const entries = new Map<string, { tokens: number; usd_nanos: number; response?: string }>();
  let cancelled = 0;
  const totals = () => [...entries.values()].reduce((n, r) => n + (mode === "tokens" ? r.tokens : r.usd_nanos), 0);
  const record = async (p: Record<string, unknown>) => {
    const key = String(p.request);
    let admitted = true;
    if (p.action === "reserve") {
      const amount = Number(mode === "tokens" ? p.tokens : p.usd_nanos);
      admitted = totals() + amount <= cap;
      if (admitted) entries.set(key, { tokens: Number(p.tokens), usd_nanos: Number(p.usd_nanos) });
    } else if (p.action === "settle") {
      const duplicate = [...entries.values()].some((r) => r.response === p.response);
      entries.set(key, { tokens: duplicate ? 0 : Number(p.tokens), usd_nanos: duplicate ? 0 : Number(p.usd_nanos), response: String(p.response) });
    }
    return { admitted, exhausted: totals() >= cap, tokens: totals() };
  };
  const budget = () => new RunBudget({ record, cancel: () => { cancelled++; } });
  const host = budget();
  const started = [barrier(), barrier(), barrier(), barrier()];
  let launched = 0;
  let aborted = 0;
  const held: StreamFunction = (_m, _c, options) => {
    const stream = createAssistantMessageEventStream();
    const finish = () => { aborted++; const error = { ...message("partial"), stopReason: "aborted" as const };
      stream.push({ type: "error", reason: "aborted", error }); stream.end(error); };
    options?.signal?.addEventListener("abort", finish, { once: true });
    started[launched++]?.resolve();
    return stream;
  };
  // Both stream and streamSimple are installed through wrap in HostSession: same instance/balance.
  const wrapped = host.wrap(held);
  const active = ["main", "summary", "explore", "oracle"].map((purpose) => wrapped(model, context, { sessionId: purpose }).result());
  await Promise.all(started.map((b) => b.promise));
  expect(launched).toBe(4);
  const denied = await wrapped(model, context).result();
  expect(denied.stopReason).toBe("error");
  await Promise.all(active);
  expect(aborted).toBe(4);
  expect(cancelled).toBeGreaterThan(0);
  expect(entries.size).toBe(5);
  expect([...entries.values()].every((r) => r.response === undefined)).toBeTrue();
  expect(host.state().active).toBe(0);
  const unchanged = budget();
  await unchanged.refresh();
  expect((await unchanged.wrap(held)(model, context).result()).errorMessage).toContain("budget_exhausted");
  expect(launched).toBe(4);
  cap += mode === "tokens" ? 50 : bound.usd_nanos * 2;
  // Existing generation remains stopped even if configuration is edited underneath it.
  expect((await host.wrap(held)(model, context).result()).stopReason).toBe("error");
  const resumed = budget();
  await resumed.refresh();
  const success: StreamFunction = () => {
    const stream = createAssistantMessageEventStream(); const result = message("same-provider-response");
    stream.push({ type: "done", reason: "stop", message: result }); stream.end(result); return stream;
  };
  for (let i = 0; i < 2; i++) expect((await resumed.wrap(success)(model, context).result()).stopReason).toBe("stop");
  expect(totals()).toBe((mode === "tokens" ? 125 : bound.usd_nanos * 5) + (mode === "tokens" ? 6 : 6000));
  console.log(`${mode}: four exact start barriers, cancellation 4/4; unchanged resume stopped; raised resume preserves reservations and response dedupe`);
}, 10_000);

test("unbounded models fail before transport and zero-priced nonlocal models expose a pricing gap", () => {
  expect(() => reservation({ ...model, contextWindow: Infinity }, 5)).toThrow("budget_unbounded");
  const free = { ...model, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } };
  expect(reservation(free, 5).priced).toBeFalse();
  expect(reservation({ ...free, provider: "llama.cpp" }, 5).priced).toBeTrue();
});

test("an unbounded subprocess lane is labelled before admission and refuses a hard-cap request", async () => {
  let launched = false;
  let unbounded: unknown;
  const host = new RunBudget({ unbounded: true, cancel: () => {}, record: async (p) => {
    unbounded = p.unbounded;
    return { admitted: false };
  } });
  const source: StreamFunction = () => { launched = true; throw new Error("must not launch"); };
  const result = await host.wrap(source)(model, context).result();
  expect(unbounded).toBeTrue();
  expect(launched).toBeFalse();
  expect(result.errorMessage).toContain("unbounded transport");
});

test("Codex priority reserves and settles at the SDK tier rate without changing tokens or wire model prices", async () => {
  for (const item of [
    { id: "gpt-6-luna", tier: "priority", multiplier: 2, aborted: false },
    { id: "gpt-5.5", tier: "priority", multiplier: 2.5, aborted: false },
    { id: "gpt-6-luna", tier: undefined, multiplier: 1, aborted: false },
    { id: "gpt-6-luna", tier: "default", multiplier: 1, aborted: false },
    { id: "gpt-6-luna", tier: "priority", multiplier: 2, aborted: true },
  ]) {
    const priced: Model<Api> = { ...model, id: item.id, provider: "openai-codex", api: "openai-codex-responses", contextWindow: 1_000_000,
      cost: { input: 1, output: 0, cacheRead: 0, cacheWrite: 0 } };
    const records: Record<string, unknown>[] = []; let forwardedTier: unknown;
    const budget = new RunBudget({ cancel() {}, record: async payload => { records.push(payload); return { admitted: true }; } });
    const source: StreamFunction = (wireModel, _context, options) => {
      expect(wireModel.cost).toEqual({ input: 1, output: 0, cacheRead: 0, cacheWrite: 0 });
      forwardedTier = options && "serviceTier" in options ? options.serviceTier : undefined;
      const stream = createAssistantMessageEventStream();
      const result: AssistantMessage = { ...message("fixture-tier"), api: priced.api, provider: priced.provider, model: priced.id,
        stopReason: item.aborted ? "aborted" : "stop", usage: { ...message("usage").usage, input: 1_000_000, output: 0, totalTokens: 1_000_000 } };
      if (item.aborted) stream.push({ type: "error", reason: "aborted", error: result });
      else stream.push({ type: "done", reason: "stop", message: result });
      stream.end(result); return stream;
    };
    const options = { maxTokens: 5, serviceTier: item.tier };
    const terminal: string[] = [];
    for await (const event of budget.wrap(source)(priced, context, options)) terminal.push(event.type);
    expect(terminal).toEqual(item.aborted ? ["error"] : ["done"]);
    expect(forwardedTier).toBe(item.tier);
    expect(records.map(record => record.action)).toEqual(item.aborted ? ["reserve"] : ["reserve", "settle"]);
    expect(records[0]?.usd_nanos).toBe(1_000_000_000 * item.multiplier);
    expect(records[0]?.tokens).toBe(1_000_005);
    if (!item.aborted) {
      expect(records[1]?.usd_nanos).toBe(1_000_000_000 * item.multiplier);
      expect(records[1]?.tokens).toBe(1_000_000);
    }
    expect(budget.state().active).toBe(0);
  }
});

test("a priority request exceeding the USD cap is refused before transport", async () => {
  let launched = false; let reserved: unknown;
  const priced: Model<Api> = { ...model, id: "gpt-6-luna", provider: "openai-codex", api: "openai-codex-responses", contextWindow: 1_000_000,
    cost: { input: 1, output: 0, cacheRead: 0, cacheWrite: 0 } };
  const budget = new RunBudget({ cancel() {}, record: async payload => {
    reserved = payload.usd_nanos;
    return { admitted: Number(payload.usd_nanos) <= 1_500_000_000, limits: { max_usd_nanos: 1_500_000_000 } };
  } });
  const source: StreamFunction = () => { launched = true; throw new Error("must not contact provider"); };
  const options = { maxTokens: 5, serviceTier: "priority" };
  const result = await budget.wrap(source)(priced, context, options).result();
  expect(reserved).toBe(2_000_000_000); expect(launched).toBe(false);
  expect(result.errorMessage).toContain("budget_exhausted");
});

test("cache guard refusal preserves the session and retries admission after owner resume", async () => {
  let paused = true; let launched = 0; let cancelled = 0; let stopped = 0;
  const records: Record<string, unknown>[] = [];
  const budget = new RunBudget({
    cancel: () => { cancelled++; }, onStop: () => { stopped++; },
    record: async payload => {
      records.push(payload);
      if (payload.action === "reserve" && paused) {
        return { admitted: false, reason: "cache_guard_paused", guard_paused: true, exhausted: false };
      }
      return { admitted: true, exhausted: false };
    },
  });
  const source: StreamFunction = () => {
    launched++;
    const stream = createAssistantMessageEventStream();
    const result = message("guard-resumed");
    result.usage = { ...result.usage, input: 3, cacheRead: 7, cacheWrite: 2, output: 1, totalTokens: 13 };
    stream.push({ type: "done", reason: "stop", message: result }); stream.end(result); return stream;
  };
  const wrapped = budget.wrap(source);
  const denied = await wrapped(model, context).result();
  expect(denied.errorMessage).toContain("cache_guard_paused");
  expect(denied.errorMessage).not.toContain("budget_exhausted");
  expect(launched).toBe(0);
  expect(cancelled).toBe(0);
  expect(stopped).toBe(0);
  expect(budget.state().stopped).toBeFalse();
  paused = false; // Authoritative owner command clears the server record, not a host latch.
  expect((await wrapped(model, context).result()).stopReason).toBe("stop");
  expect(launched).toBe(1);
  const settle = records.find(record => record.action === "settle");
  expect(settle?.usage).toEqual({ input: 3, cache_read: 7, cache_write: 2, output: 1 });
  expect(settle?.tokens).toBe(13);
  expect(settle?.model).toBe(model.id);
  expect(settle?.provider).toBe(model.provider);
  expect(settle?.request).toBe(records[1]?.request);
  expect(budget.state().active).toBe(0);
});

test("Pi summary requests and auxiliary lineages are distinct from the session conversation", async () => {
  const records: Record<string, unknown>[] = [];
  const budget = new RunBudget({
    sessionId: () => "primary", cancel() {},
    record: async payload => { records.push(payload); return { admitted: true, exhausted: false }; },
  });
  let requests = 0;
  const source: StreamFunction = () => {
    const stream = createAssistantMessageEventStream();
    const result = message(`purpose-${requests++}`);
    result.content = [{ type: "text", text: "summary" }];
    stream.push({ type: "done", reason: "stop", message: result }); stream.end(result); return stream;
  };
  const wrapped = budget.wrap(source);
  const conversationOptions = { sessionId: "primary", convertToLlm: () => [] };
  await wrapped(model, context, conversationOptions).result();
  // Exercise the installed Pi summary call site, not a handwritten options fixture.
  await generateSummaryWithUsage(
    [{ role: "user", content: "Summarize this fixture.", timestamp: 1 }],
    model, undefined, undefined, undefined, undefined, undefined, undefined, undefined,
    wrapped, undefined, undefined, undefined, "primary",
  );
  await wrapped(model, context, { ...conversationOptions, sessionId: "auxiliary" }).result();
  await wrapped(model, context, { ...conversationOptions, cacheRetention: "none" }).result();
  await wrapped(model, context, { sessionId: "primary" }).result();
  expect(records.filter(record => record.action === "settle").map(record => record.purpose))
    .toEqual(["conversation", "summary", "other", "conversation", "other"]);
  expect(records.filter(record => record.action === "settle").map(record => record.cache_lineage))
    .toEqual(["primary", "primary", "auxiliary", "primary", "primary"]);
});
