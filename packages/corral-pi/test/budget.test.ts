import { expect, test } from "bun:test";
import { normalizeContext, createAssistantMessageEventStream, type Api, type AssistantMessage, type Model, type StreamFunction } from "@earendil-works/pi-ai";
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
