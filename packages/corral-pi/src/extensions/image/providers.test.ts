import { expect, test } from "bun:test";
import type { Credential } from "@earendil-works/pi-ai";
import type { Fetch } from "../../auth/types.ts";
import { generateImage, type ImageProviderOptions } from "./providers.ts";

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const platform = { provider: "openai", model: "gpt-image-1" } as const;
const codex = { provider: "openai-codex", model: "carrier" } as const;
const apiKey: Credential = { type: "api_key", key: "sk-fixtureOnly" };
const claims = Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "fixture" } })).toString("base64url");
const oauth: Credential = { type: "oauth", access: `eyJhbGciOiJIUzI1NiJ9.${claims}.fake`, refresh: "not-used", expires: Date.now() + 3_600_000 };
const signal = () => new AbortController().signal;
function options(c: Credential, fetch: Fetch): ImageProviderOptions {
  return { fetch, auth: { provider: "openai-codex", providers: ["openai", "openai-codex"],
    credentials: () => ({ read: async () => c, list: async () => [], modify: async () => { throw new Error("unexpected write"); }, delete: async () => {} }) } };
}

test("platform image uses only the dedicated destination with one literal-key POST", async () => {
  let calls = 0;
  const result = await generateImage(options(apiKey, async (url, init) => {
    calls++;
    expect(url).toBe("https://api.openai.com/v1/images/generations");
    expect(init?.redirect).toBe("error");
    expect(init?.method).toBe("POST");
    expect(new Headers(init?.headers).get("authorization")).toBe(`Bearer ${apiKey.key}`);
    expect(JSON.parse(String(init?.body))).toEqual({ model: platform.model, prompt: "make an image", n: 1, size: "1024x1024", output_format: "png" });
    return Response.json({ data: [{ b64_json: png }], usage: { input_tokens: 12, output_tokens: 81 } });
  }), platform, "make an image", signal());
  expect(calls).toBe(1);
  expect(result).toEqual({ base64: png, usage: { inputTokens: 12, outputTokens: 81 } });
});

const done = { type: "image_generation_call", status: "completed", id: "ig_a", result: png };
const event = (value: unknown) => `data: ${JSON.stringify(value)}\r\n\r\n`;
const final = (output: unknown[]) => event({ type: "response.completed", response: { status: "completed", model: codex.model,
  output, tools: [{ type: "image_generation", model: "gpt-image-test" }], usage: { total_tokens: 7 } } });
function sse(payload: string) {
  return new Response(new ReadableStream({ start(controller) {
    const enc = new TextEncoder();
    for (let i = 0; i < payload.length; i += 7) controller.enqueue(enc.encode(payload.slice(i, i + 7)));
    controller.close();
  } }), { headers: { "content-type": "text/event-stream" } });
}

test("Codex uses OAuth without a session, ignores partial frames and deduplicates the final item", async () => {
  let calls = 0;
  const result = await generateImage(options(oauth, async (url, init) => {
    calls++;
    expect(url).toBe("https://chatgpt.com/backend-api/codex/responses");
    expect(init?.redirect).toBe("error");
    const headers = new Headers(init?.headers);
    expect(headers.get("authorization")).toBe(`Bearer ${oauth.access}`);
    expect(headers.get("chatgpt-account-id")).toBe("fixture");
    expect(headers.has("session-id")).toBe(false);
    expect(headers.get("originator")).toBe("pi");
    expect(JSON.parse(String(init?.body))).toMatchObject({ model: codex.model, store: false, stream: true,
      tools: [{ type: "image_generation", action: "generate", output_format: "png" }], tool_choice: { type: "image_generation" } });
    return sse(event({ type: "response.image_generation_call.partial_image", partial_image_b64: "bogus" }) +
      event({ type: "response.output_item.done", item: done }) + final([done]));
  }), codex, "make an image", signal());
  expect(result).toEqual({ base64: png, servedModel: "gpt-image-test", usage: { totalTokens: 7 } });
  expect(calls).toBe(1);
});

test("Codex accepts final-only bytes and refuses truncation, partial-only, text-only and distinct images", async () => {
  const run = (payload: string) => generateImage(options(oauth, async () => sse(payload)), codex, "draw", signal());
  expect((await run(final([done]))).base64).toBe(png);
  for (const wire of [final([done]).slice(0, -2), final([{ type: "message", content: [{ text: png }] }]),
    event({ type: "response.output_item.done", item: done }),
    event({ type: "response.image_generation_call.partial_image", partial_image_b64: png }) + final([]),
    final([done, { ...done, id: "ig_b" }])]) {
    await expect(run(wire)).rejects.toThrow("image_generation_failed:");
  }
});

test("requires the selected credential and fails closed for expired OAuth", async () => {
  let calls = 0;
  const fetch: Fetch = async () => { calls++; throw new Error("unexpected send"); };
  await expect(generateImage(options(apiKey, fetch), codex, "draw", signal())).rejects.toThrow("image_generation_unavailable:");
  await expect(generateImage(options({ ...oauth, expires: 0 }, fetch), codex, "draw", signal())).rejects.toThrow("renew");
  await expect(generateImage(options(oauth, fetch), platform, "draw", signal())).rejects.toThrow("image_generation_unavailable:");
  expect(calls).toBe(0);
});

test("never fetches returned URLs, retries, or reveals provider errors", async () => {
  for (const response of [Response.json({ data: [{ url: "https://evil.invalid/file.png" }] }),
    new Response("raw secret", { status: 429 }), new Response("raw secret", { status: 401 })]) {
    let calls = 0;
    const result = generateImage(options(apiKey, async () => { calls++; return response; }), platform, "draw", signal());
    const error = await result.catch((x: Error) => x);
    expect(error instanceof Error).toBe(true);
    expect(String(error)).not.toContain("raw secret");
    expect(calls).toBe(1);
  }
});

test("caps streamed bytes and encoded image size without trusting Content-Length", async () => {
  const oversized = () => new Response(new ReadableStream({ start(controller) {
    controller.enqueue(new Uint8Array(16 * 1024 * 1024 + 1));
    controller.close();
  } }));
  for (const fetch of [async () => oversized(), async () => Response.json({ data: [{ b64_json: "A".repeat(6 * 1024 * 1024) }] })]) {
    await expect(generateImage(options(apiKey, fetch), platform, "draw", signal())).rejects.toThrow("size limits");
  }
});

test("caller abort cancels the body and does not surface a raw abort reason", async () => {
  const caller = new AbortController();
  let canceled = false;
  let reading!: () => void;
  const started = new Promise<void>(resolve => { reading = resolve; });
  const result = generateImage(options(apiKey, async () => new Response(new ReadableStream({
    pull() { reading(); }, cancel() { canceled = true; },
  }, { highWaterMark: 0 }))), platform, "draw", caller.signal);
  await started;
  caller.abort(new Error("private reason"));
  await expect(result).rejects.toThrow("request was canceled");
  expect(canceled).toBe(true);
});
