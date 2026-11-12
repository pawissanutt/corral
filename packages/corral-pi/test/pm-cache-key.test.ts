import { expect, test } from "bun:test";
import { createAssistantMessageEventStream, normalizeContext, type Api, type AssistantMessage, type Model,
  type StreamFunction } from "@earendil-works/pi-ai";
import { pmCacheKeyStream } from "../src/host/pm-cache-key.ts";
import { z } from "zod";

const model: Model<Api> = { id: "fixture", provider: "openai-codex", api: "openai-codex-responses", baseUrl: "https://invalid.example",
  name: "fixture", input: ["text"], reasoning: false, contextWindow: 20, maxTokens: 5,
  cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 0 } };

test("stable PM cache keys change only conversation payloads and preserve session identity", async () => {
  const captured: { payload: unknown; sessionId: string | undefined }[] = [];
  const source: StreamFunction = (current, _context, request) => {
    const stream = createAssistantMessageEventStream();
    void (async () => {
      const body = { model: current.id, prompt_cache_key: request?.sessionId, input: [] };
      const replacement = await request?.onPayload?.(body, current);
      captured.push({ payload: replacement === undefined ? body : replacement, sessionId: request?.sessionId });
      const result: AssistantMessage = { role: "assistant", api: current.api, provider: current.provider, model: current.id,
        content: [], timestamp: 0, stopReason: "stop", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      stream.push({ type: "done", reason: "stop", message: result }); stream.end(result);
    })();
    return stream;
  };
  const identity = { store: "store-one", role: "pm" };
  const context = normalizeContext({ messages: [] });
  const conversation = (sessionId: string) => ({ sessionId, convertToLlm: () => [] });
  const unchanged = pmCacheKeyStream(source, { mode: "session", identity, sessionId: () => "one" });
  expect(unchanged).toBe(source);
  await unchanged(model, context, conversation("one")).result();
  expect(JSON.stringify(captured.at(-1)?.payload)).toBe(JSON.stringify({ model: model.id, prompt_cache_key: "one", input: [] }));
  for (const sessionId of ["one", "two"]) {
    const wrapped = pmCacheKeyStream(source, { mode: "stable", identity, sessionId: () => sessionId });
    await wrapped(model, context, conversation(sessionId)).result();
    expect(captured.at(-1)?.sessionId).toBe(sessionId);
  }
  const Key = z.object({ prompt_cache_key: z.string() });
  const first = Key.parse(captured[1]?.payload);
  const second = Key.parse(captured[2]?.payload);
  expect(first.prompt_cache_key).toMatch(/^[a-f0-9]{64}$/);
  expect(second.prompt_cache_key).toBe(first.prompt_cache_key);
  const wrapped = pmCacheKeyStream(source, { mode: "stable", identity, sessionId: () => "one" });
  await wrapped({ ...model, id: "another-model" }, context, conversation("one")).result();
  expect(Key.parse(captured.at(-1)?.payload).prompt_cache_key).not.toBe(first.prompt_cache_key);
  for (const request of [
    { sessionId: "one", cacheRetention: "none" as const },
    conversation("auxiliary"),
  ]) {
    await wrapped(model, context, request).result();
    expect(captured.at(-1)?.payload).toEqual({ model: model.id, prompt_cache_key: request.sessionId, input: [] });
  }
  const augmented = { ...conversation("one"), onPayload: (body: unknown) => ({ ...(body as object), custom: true }) };
  await wrapped(model, context, augmented).result();
  expect(captured.at(-1)?.payload).toEqual({ model: model.id, input: [], prompt_cache_key: first.prompt_cache_key, custom: true });
  expect(identity).toEqual({ store: "store-one", role: "pm" });
});
