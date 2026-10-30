import { expect, test } from "bun:test";
import { createServer } from "node:http";

import { LlamaCppSettingsSchema, localInferenceRules, type LlamaCppSettings } from "../src/auth/local.ts";
import { createLocalModels } from "../src/host/local-models.ts";
import { guardedFetch } from "../src/host/restrict.ts";

const settings: LlamaCppSettings = {
  endpoint: "http://127.0.0.1:8080", discover: true, context_window: 32768, max_tokens: 8192, timeout_ms: 15000,
};

test("the inference guard rejects foreign origins, credentials, redirects and model management", async () => {
  // Given a local-origin-only transport, separate from existing cloud rules.
  const sent: string[] = [];
  const network = guardedFetch(async (url, init) => {
    expect(init?.redirect).toBe("error");
    sent.push(String(url));
    return new Response("ok");
  }, undefined, "llama.cpp", localInferenceRules(settings));
  // When permitted reads/inference and forbidden destinations/methods are requested.
  await network.fetch(`${settings.endpoint}/v1/models`);
  await network.fetch(`${settings.endpoint}/v1/chat/completions`, { method: "POST" });
  for (const [url, method] of [
    ["http://127.0.0.1:8081/v1/models", "GET"],
    ["https://example.org/v1/chat/completions", "POST"],
    ["https://chatgpt.com/backend-api/codex/responses", "POST"],
    [`${settings.endpoint}/models/load`, "POST"],
    [`${settings.endpoint}/models`, "POST"],
    [`${settings.endpoint}/v1/models`, "POST"],
    [`${settings.endpoint}/v1/models?download=true`, "GET"],
    [`${settings.endpoint}/v1/models#fragment`, "GET"],
    ["http://owner:secret@127.0.0.1:8080/v1/models", "GET"],
  ]) {
    await expect(network.fetch(url ?? "", { method })).rejects.toThrow(/network_refused/);
  }
  for (const headers of [
    { authorization: "Bearer cloud-secret" }, { cookie: "owner=secret" }, { "x-api-key": "foreign" },
  ]) {
    await expect(network.fetch(`${settings.endpoint}/v1/chat/completions`, { method: "POST", headers }))
      .rejects.toThrow(/network_refused/);
  }
  // Then only the catalog and chat requests reach the wire.
  expect(sent).toEqual([`${settings.endpoint}/v1/models`, `${settings.endpoint}/v1/chat/completions`]);
  expect(network.ledger().allowed).toBe(2);
  expect(guardedFetch(async () => new Response("bad"), undefined, "llama.cpp").destinations).toEqual([]);
});

test("local settings reject non-origins and inconsistent token limits", () => {
  // Given malformed endpoint and budget settings at the launch boundary.
  for (const endpoint of [
    "", "file:///model.gguf", "http://user:secret@localhost", "http://localhost/path",
    "http://localhost?", "http://localhost#", "http://localhost\\evil", "http://localhost:99999",
  ]) {
    // When parsed, then invalid settings cannot create an inference exception.
    expect(LlamaCppSettingsSchema.safeParse({ ...settings, endpoint }).success).toBeFalse();
  }
  for (const invalid of [{ context_window: 0 }, { max_tokens: 32769 }, { timeout_ms: 2147483648 }]) {
    expect(LlamaCppSettingsSchema.safeParse({ ...settings, ...invalid }).success).toBeFalse();
  }
  expect(LlamaCppSettingsSchema.safeParse({ ...settings, endpoint: "http://[::1]:8080/" }).success).toBeTrue();
});

test("a real endpoint can advertise another model and stream zero-USD usage without a source patch", async () => {
  // Given a real HTTP fixture advertising one arbitrary model and a complete chat SSE response.
  let ids = ["original.gguf"];
  const requests: { readonly url?: string; readonly method?: string; readonly authorization?: string; readonly body: string }[] = [];
  const server = createServer(async (request, response) => {
    const parts: Buffer[] = [];
    for await (const part of request) parts.push(Buffer.from(part));
    const body = Buffer.concat(parts).toString();
    requests.push({ url: request.url, method: request.method, authorization: request.headers.authorization, body });
    if (request.url === "/v1/models") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ data: ids.map((id) => ({ id, object: "model" })) }));
      return;
    }
    if (request.url !== "/v1/chat/completions") {
      response.writeHead(404);
      response.end();
      return;
    }
    const input = JSON.parse(body);
    response.writeHead(200, { "content-type": "text/event-stream" });
    for (const chunk of [
      { choices: [{ index: 0, delta: { role: "assistant", content: "Local answer" }, finish_reason: null }] },
      { choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
        usage: { prompt_tokens: 23, completion_tokens: 7, total_tokens: 30 } },
    ]) {
      response.write(`data: ${JSON.stringify({ id: "local-response", object: "chat.completion.chunk", model: input.model, ...chunk })}\n\n`);
    }
    response.end("data: [DONE]\n\n");
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("fixture needs a TCP port");
    const local = { ...settings, endpoint: `http://127.0.0.1:${address.port}` };
    const network = guardedFetch(globalThis.fetch, undefined, "llama.cpp", localInferenceRules(local));
    const first = await createLocalModels(local, ids[0] ?? "", network.fetch);
    expect(first.models.getModels().map((model) => model.id)).toEqual(["original.gguf"]);
    // When the endpoint adds an unseen ID, a refresh discovers it and config selects it.
    ids = [...ids, "new-family/new-model.gguf"];
    await first.models.refresh({ allowNetwork: true });
    expect(first.models.getModel("llama.cpp", "new-family/new-model.gguf")?.id).toBe("new-family/new-model.gguf");
    const selected = await createLocalModels(local, "llama.cpp/new-family/new-model.gguf", network.fetch);
    const answer = await selected.models.completeSimple({ ...selected.model, baseUrl: "https://foreign.invalid/v1" }, {
      messages: [{ role: "user", content: "Answer", timestamp: 0 }],
    }, { maxTokens: 99999 });
    // Then the bound origin, arbitrary ID, token counts and zero cost all survive the real adapter.
    expect(answer.stopReason).toBe("stop");
    expect(answer.model).toBe("new-family/new-model.gguf");
    expect(answer.provider).toBe("llama.cpp");
    expect(answer.usage.input).toBe(23);
    expect(answer.usage.output).toBe(7);
    expect(answer.usage.cost).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 });
    const inference = requests.filter((request) => request.method === "POST");
    expect(inference).toHaveLength(1);
    expect(inference[0]?.url).toBe("/v1/chat/completions");
    expect(inference[0]?.authorization).toBe("Bearer local");
    expect(JSON.parse(inference[0]?.body ?? "{}").model).toBe("new-family/new-model.gguf");
    expect(JSON.parse(inference[0]?.body ?? "{}").max_tokens).toBe(8192);
    expect(requests.every((request) => ["/v1/models", "/v1/chat/completions"].includes(request.url ?? ""))).toBeTrue();
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
      server.closeAllConnections();
    });
  }
});

test("explicit model configuration needs no discovery request and rejects missing discovered IDs", async () => {
  // Given a server with no catalog endpoint and an explicit model choice.
  let calls = 0;
  const send = async () => { calls++; return Response.json({ data: [] }); };
  // When discovery is disabled, then config alone supplies the model and no request is made.
  const configured = await createLocalModels({ ...settings, discover: false }, "configured/new.gguf", send);
  expect(configured.model.id).toBe("configured/new.gguf");
  expect(calls).toBe(0);
  await expect(createLocalModels(settings, "missing.gguf", send)).rejects.toThrow(/local_model_unavailable/);
  expect(calls).toBe(1);
});

test("catalog discovery refuses redirects and bounds malformed or oversized responses", async () => {
  // Given a local catalog returning a redirect, non-JSON, wrong schema or excessive body.
  for (const response of [
    new Response(null, { status: 302, headers: { location: "https://foreign.invalid/v1/models" } }),
    new Response("not JSON"), Response.json({ data: [{ id: 3 }] }), new Response("x".repeat(1_048_577)),
  ]) {
    // When discovery runs, then it fails without selecting an unverified model.
    await expect(createLocalModels(settings, "model", async () => response)).rejects.toThrow();
  }
});
