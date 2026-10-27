// The Anthropic route end to end inside Pi (T-479): Pi's own anthropic-messages client, given a
// subscription OAuth credential, sends through the host's guarded fetch to an allowed endpoint.
import { expect, test } from "bun:test";

import type { Credential } from "@earendil-works/pi-ai";

import { guardedFetch } from "../src/host/restrict.ts";

test("Pi's Anthropic stream for claude-opus-5-5 reaches only the guarded messages endpoint", async () => {
  const sent: string[] = [];
  const guard = guardedFetch(async (input) => {
    sent.push(String(input instanceof Request ? input.url : input));
    return new Response("synthetic failure", { status: 400 });
  }, undefined, "anthropic");
  const credential: Credential = { type: "oauth", access: "sk-ant-oat-synthetic", refresh: "r", expires: Date.now() + 3_600_000 };
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: {
      async read(provider: string) { return provider === "anthropic" ? credential : undefined; },
      async list() { return [{ providerId: "anthropic", type: "oauth" as const }]; },
      async modify() { throw new Error("read-only"); },
      async delete() { throw new Error("read-only"); },
    },
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const provider = models.getProvider("anthropic");
  const model = models.getModel("anthropic", "claude-opus-5-5");
  expect(provider && model ? model.api : undefined).toBe("anthropic-messages");
  const auth = await models.getAuth("anthropic");
  const stream = provider!.stream(model!, { messages: [{ role: "user", content: "hi", timestamp: Date.now() }] } as never,
    { apiKey: auth?.auth.apiKey, fetch: guard.fetch as typeof fetch, maxRetries: 0 } as never);
  for await (const _event of stream) { /* drain */ }
  expect(sent.length).toBeGreaterThan(0);
  expect(new Set(sent)).toEqual(new Set(["https://api.anthropic.com/v1/messages?beta=true"]));
  expect(guard.ledger().refused).toBe(0);
});
