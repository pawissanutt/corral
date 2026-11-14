import { expect, test } from "bun:test";
import { normalizeContext, type Api, type AssistantMessage, type Model } from "@earendil-works/pi-ai";
import { RunBudget, reservation } from "../capabilities/budget.ts";
import { bindingDigest, type ConnectionBinding } from "./connections.ts";

// Lowest-layer physical-attempt accounting; transport/auth journeys already live in
// test/api-connections.test.ts. No network or real credential storage here.
test("physical API retries retain failed attempts and share a finite allowance after restart", async () => {
  const model: Model<Api> = { id: "fixture", provider: "openai", api: "openai-responses", baseUrl: "https://api.openai.com/v1",
    name: "fixture", input: ["text"], reasoning: false, contextWindow: 20, maxTokens: 5,
    cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 0.5 } };
  const base = { version: 1 as const, connection: "openai-api" as const, identity: "openai-api", origin: "registered" as const,
    candidate: "exact-api", model: model.id, effort: null };
  const binding: ConnectionBinding = { ...base, digest: bindingDigest(base) };
  const entries = new Map<string, { nanos: number; response?: string }>();
  const cap = reservation(model, 5).usd_nanos * 2;
  const total = () => [...entries.values()].reduce((n, r) => n + r.nanos, 0);
  const record = async (p: Record<string, unknown>) => {
    let admitted = true;
    if (p.action === "reserve") {
      expect(p.model).toBe(binding.model); expect(p.provider).toBe("openai");
      expect("episode" in p).toBeFalse(); expect("connection" in p).toBeFalse();
      admitted = total() + Number(p.usd_nanos) <= cap;
      if (admitted) entries.set(String(p.request), { nanos: Number(p.usd_nanos) });
    } else if (p.action === "settle") entries.set(String(p.request), { nanos: Number(p.usd_nanos), response: String(p.response) });
    return { admitted, exhausted: total() >= cap, connection: binding.connection, binding_digest: binding.digest,
      paid: { episode: "synthetic-canonical", epoch: 1 } };
  };
  const host = () => new RunBudget({ record, cancel() {} });
  const budget = host();
  const attempt = { binding, model, context: normalizeContext({ messages: [] }), options: { maxTokens: 5 } };
  await budget.physical(attempt); // failed internal compatibility attempt: no completion evidence
  const final = await budget.physical(attempt); // exact cap still permits THIS admitted POST
  expect(entries.size).toBe(2);
  const message: AssistantMessage = { role: "assistant", api: model.api, provider: model.provider, model: model.id,
    content: [], timestamp: 1, stopReason: "stop", responseId: "canonical-response",
    usage: { input: 4, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 6,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
  if (!final) throw new Error("physical budget adapter must return a settlement permit");
  await final.settle(message);
  expect([...entries.values()].filter(r => r.response === undefined)).toHaveLength(1);
  expect(total()).toBe(reservation(model, 5).usd_nanos + 6000);
  await expect(host().physical(attempt)).rejects.toThrow("canonical paid admission refused");
  expect(entries.size).toBe(2);
  await expect(final.settle(message)).rejects.toThrow("already settled");
  const unbound = new RunBudget({ cancel() {}, record: async () => ({ admitted: true }) });
  await expect(unbound.physical(attempt)).rejects.toThrow("canonical paid admission refused");
});
