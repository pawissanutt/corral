// The Anthropic route inside Pi (T-479, T-493): model calls run through the Claude Agent SDK lane,
// so the host process never reaches Anthropic's messages endpoint itself. Pi's own
// anthropic-messages client, given a subscription OAuth credential, is refused by the guard unsent.
import { expect, test } from "bun:test";

import type { Credential } from "@earendil-works/pi-ai";

import { guardedFetch } from "../src/host/restrict.ts";

test("Pi's direct Anthropic stream for claude-opus-5-5 is refused by the host guard", async () => {
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
  expect(sent).toEqual([]);
  expect(guard.ledger().allowed).toBe(0);
  expect(guard.ledger().refusedDestinations).toEqual(["https://api.anthropic.com/v1/messages"]);
});
