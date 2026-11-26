import { PROVIDERS, validApiKey } from "../../auth/providers.ts";
import type { AuthModule, Fetch } from "../../auth/types.ts";
import { isGptImageModel, type HostedRoute, type ImageProvider } from "../../host/hosted-settings.ts";
import { guardedFetch } from "../../host/restrict.ts";

export interface ImageProviderOptions { auth: AuthModule; fetch: Fetch }
interface ImageUsage { inputTokens?: number; outputTokens?: number; totalTokens?: number; cachedInputTokens?: number }
export interface GeneratedImage { base64: string; servedModel?: string; usage?: ImageUsage }

const API = "https://api.openai.com/v1/images/generations";
const LIMIT = 16 * 1024 * 1024;
const MAX_BASE64 = Math.ceil(4 * 1024 * 1024 / 3) * 4;
class ImageRequestError extends Error {}
const failed = (why: string) => new ImageRequestError(`image_generation_failed: ${why}`);
const unavailable = (why: string) => new ImageRequestError(`image_generation_unavailable: ${why}`);
const invalid = () => failed("provider did not complete exactly one PNG image");
const tooLarge = () => failed("image response exceeds transport or image size limits");
const record = (x: unknown): Record<string, unknown> => x !== null && typeof x === "object" ? x as Record<string, unknown> : {};
const modelId = (x: unknown): x is string => typeof x === "string" && /^[A-Za-z0-9][A-Za-z0-9._-]{0,239}$/.test(x) && !/^(sk-|sess-|eyj)/i.test(x);

function image(value: unknown): string {
  if (typeof value !== "string" || !value) throw invalid();
  if (value.length > MAX_BASE64) throw tooLarge();
  return value; // Publication validates canonical base64, decoded size and PNG structure.
}

function metadata(wire: unknown, served: unknown): Omit<GeneratedImage, "base64"> {
  const raw = record(wire);
  const usage: ImageUsage = {};
  const fields = { inputTokens: raw.input_tokens, outputTokens: raw.output_tokens, totalTokens: raw.total_tokens,
    cachedInputTokens: record(raw.input_tokens_details).cached_tokens };
  for (const key of Object.keys(fields) as (keyof ImageUsage)[]) {
    const count = fields[key];
    if (typeof count === "number" && Number.isSafeInteger(count) && count >= 0) usage[key] = count;
  }
  return { ...(modelId(served) ? { servedModel: served } : {}), ...(Object.keys(usage).length ? { usage } : {}) };
}

async function* chunks(response: Response, signal: AbortSignal): AsyncGenerator<string> {
  if (!response.body) throw invalid();
  const reader = response.body.getReader();
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", cancel, { once: true });
  const decoder = new TextDecoder("utf-8", { fatal: true });
  try {
    if (Number(response.headers.get("content-length")) > LIMIT) throw tooLarge();
    let bytes = 0;
    let reads = 0;
    for (;;) {
      signal.throwIfAborted();
      const { value, done } = await reader.read();
      signal.throwIfAborted();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > LIMIT || ++reads > 262144) throw tooLarge();
      const text = decoder.decode(value, { stream: true });
      if (text) yield text;
    }
    const rest = decoder.decode();
    if (rest) yield rest;
  } finally {
    signal.removeEventListener("abort", cancel);
    cancel();
    reader.releaseLock();
  }
}

async function platformResult(response: Response, signal: AbortSignal): Promise<GeneratedImage> {
  const parts: string[] = [];
  for await (const part of chunks(response, signal)) parts.push(part);
  const final = record(JSON.parse(parts.join("")));
  if (!Array.isArray(final.data) || final.data.length !== 1) throw invalid();
  return { base64: image(record(final.data[0]).b64_json), ...metadata(final.usage, final.model) };
}

async function codexResult(response: Response, signal: AbortSignal): Promise<GeneratedImage> {
  if (response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() !== "text/event-stream") {
    void response.body?.cancel().catch(() => {});
    throw invalid();
  }
  let complete: { id: string; base64: string } | undefined;
  const collect = (wire: unknown) => {
    const item = record(wire);
    if (item.type !== "image_generation_call") return;
    if (item.status !== "completed" || (item.output_format != null && item.output_format !== "png")) throw invalid();
    if (typeof item.id !== "string" || !item.id || item.id.length > 256) throw invalid();
    const base64 = image(item.result);
    if (complete && (complete.id !== item.id || complete.base64 !== base64)) throw invalid();
    complete = { id: item.id, base64 };
  };
  const dispatch = (data: string): GeneratedImage | undefined => {
    if (data === "[DONE]") throw invalid(); // Termination without a completed response.
    const event = record(JSON.parse(data));
    if (event.type === "error" || event.type === "response.failed" || event.type === "response.incomplete") throw invalid();
    if (event.type === "response.output_item.done") collect(event.item);
    if (event.type !== "response.completed" && event.type !== "response.done") return;
    const final = record(event.response);
    if (final.status !== "completed" || !Array.isArray(final.output)) throw invalid();
    for (const item of final.output) collect(item);
    if (!complete) throw invalid();
    const tool = Array.isArray(final.tools) ? final.tools.map(record).find(x => x.type === "image_generation") : undefined;
    // final.model is the carrier, not the backend-selected image model.
    return { base64: complete.base64, ...metadata(final.usage, tool?.model) };
  };
  let line: string[] = [];
  let data: string[] = [];
  let sawCR = false;
  for await (const chunk of chunks(response, signal)) {
    const breaks = /[\r\n]/g;
    let start = 0;
    for (let end; (end = breaks.exec(chunk));) {
      const offset = end.index;
      if (offset === start && sawCR && end[0] === "\n") {
        start = offset + 1;
        sawCR = false;
        continue;
      }
      if (offset > start) line.push(chunk.slice(start, offset));
      const field = line.join("");
      line = [];
      start = offset + 1;
      sawCR = end[0] === "\r";
      if (field === "" && data.length) {
        const message = data.join("\n");
        data = [];
        const final = dispatch(message);
        if (final) return final;
      } else if (field.startsWith("data:")) data.push(field.slice(field[5] === " " ? 6 : 5));
    }
    if (start < chunk.length) {
      line.push(chunk.slice(start));
      sawCR = false;
    }
  }
  throw invalid();
}

function accountId(token: string): string | undefined {
  if (token.length > 16384 || !/^eyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(token)) return;
  try {
    const payload = record(JSON.parse(Buffer.from(token.split(".")[1]!, "base64url").toString("utf8")));
    const id = record(payload["https://api.openai.com/auth"]).chatgpt_account_id;
    if (typeof id === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(id)) return id;
  } catch { /* An invalid account claim is not usable OAuth auth. */ }
}

async function request(options: ImageProviderOptions, route: HostedRoute<ImageProvider>, prompt: string, signal: AbortSignal, preflight?: () => void) {
  const { provider, model } = route;
  if ((provider !== "openai" && provider !== "openai-codex") || !modelId(model)) throw unavailable("invalid OpenAI image route");
  if (typeof prompt !== "string" || !prompt.trim() || prompt.length > 8000) throw failed("prompt must contain 1–8000 characters");
  const codex = provider === "openai-codex";
  if (!codex && !isGptImageModel(model)) throw unavailable("select a GPT Image model");
  const context = { provider, apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: PROVIDERS[provider].refresh };
  if (!(options.auth.providers ?? [options.auth.provider]).includes(provider)) throw unavailable("selected auth route is not bound");
  let credential;
  try { credential = await options.auth.credentials(context).read(provider, { signal }); }
  catch { throw unavailable("could not access the selected OpenAI credential"); }
  signal.throwIfAborted();
  const headers = new Headers({ "content-type": "application/json" });
  let token: string;
  let body: object;
  const endpoint = codex ? context.apiEndpoint : API;
  if (codex) {
    // Pinned Pi refresh uses global fetch and an unbounded JSON read. Do not perform it in this tool.
    if (credential?.type !== "oauth" || !Number.isFinite(credential.expires) || credential.expires <= Date.now()) {
      throw unavailable("renew the Corral OpenAI subscription login before using experimental generation");
    }
    token = credential.access;
    const account = typeof token === "string" ? accountId(token) : undefined;
    if (!account) throw unavailable("subscription credential has no usable account claim");
    headers.set("chatgpt-account-id", account);
    headers.set("originator", "pi"); // This is Pi, not the Codex CLI. Match the pinned Pi SSE adapter.
    headers.set("openai-beta", "responses=experimental");
    headers.set("accept", "text/event-stream");
    body = { model, instructions: "Generate one PNG from the user's prompt using image_generation.", input: [{ role: "user", content: [{ type: "input_text", text: prompt }] }], store: false, stream: true, parallel_tool_calls: false,
      tools: [{ type: "image_generation", action: "generate", output_format: "png" }], tool_choice: { type: "image_generation" } };
  } else {
    if (credential?.type !== "api_key" || !validApiKey(credential.key)) throw unavailable("a Corral OpenAI platform key is required");
    token = credential.key;
    body = { model, prompt, n: 1, size: "1024x1024", output_format: "png" };
  }
  const authorization = `Bearer ${token}`;
  headers.set("authorization", authorization);
  const send = options.auth.transport?.(context) ?? options.fetch;
  const network = guardedFetch(send, undefined, provider, [{ method: "POST", display: endpoint,
    matches: (url, sent) => url === endpoint && sent.get("authorization") === authorization }]);
  signal.throwIfAborted();
  preflight?.();
  const response = await network.fetch(endpoint, { method: "POST", headers, body: JSON.stringify(body), signal });
  if (signal.aborted || !response.ok || response.redirected || (response.url && response.url !== endpoint)) {
    void response.body?.cancel().catch(() => {});
    signal.throwIfAborted();
    if (response.status === 401 || response.status === 403) throw unavailable("selected route denied access; check login and model access");
    if (response.status === 429) throw failed("selected route is rate limited or out of quota");
    if (codex) throw failed("experimental subscription generation was rejected; check carrier support or explicitly choose the API route");
    throw failed("OpenAI rejected the image request; check model and account access");
  }
  return codex ? codexResult(response, signal) : platformResult(response, signal);
}

export async function generateImage(options: ImageProviderOptions, route: HostedRoute<ImageProvider>, prompt: string,
  caller: AbortSignal, preflight?: () => void): Promise<GeneratedImage> {
  const deadline = AbortSignal.timeout(180_000);
  const signal = AbortSignal.any([caller, deadline]);
  const aborted = () => failed(deadline.aborted ? "request exceeded the 180-second deadline" : "request was canceled");
  if (signal.aborted) throw aborted();
  let cancel!: () => void;
  const cancellation = new Promise<never>((_, reject) => {
    cancel = () => reject(aborted());
    signal.addEventListener("abort", cancel, { once: true });
  });
  try {
    const result = await Promise.race([cancellation, request(options, route, prompt, signal, preflight)]);
    if (signal.aborted) throw aborted();
    return result;
  } catch (error) {
    if (signal.aborted) throw aborted();
    if (error instanceof ImageRequestError) throw error;
    throw failed("could not complete the selected OpenAI image request");
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}
