import { expect, test } from "bun:test";
import { normalizeContext, type Api, type Model, type StreamFunction } from "@earendil-works/pi-ai";
import { RunBudget } from "../src/capabilities/budget.ts";

test("unlimited legacy history keeps admitting after a stream failure", async () => {
  const model: Model<Api> = { id: "fixture", provider: "fixture", api: "openai-responses", baseUrl: "https://invalid.example", name: "fixture", input: ["text"], reasoning: false, contextWindow: 20, maxTokens: 5, cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 1 } };
  let calls = 0, cancelled = 0;
  const budget = new RunBudget({ cancel: () => { cancelled++; }, record: async () => ({ admitted: true, exhausted: false, legacy_usage_gap: true, limits: { max_tokens: null, max_usd_nanos: null } }) });
  const source: StreamFunction = () => { calls++; throw new Error("transport failure"); };
  const wrapped = budget.wrap(source);
  await wrapped(model, normalizeContext({ messages: [] })).result();
  await wrapped(model, normalizeContext({ messages: [] })).result();
  expect(calls).toBe(2);
  expect(cancelled).toBe(0);
  expect(budget.state().stopped).toBeFalse();
});
