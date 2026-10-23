// Offline stand-in for the pinned Codex SSE endpoint. The upstream provider
// reaches it only through its injected fetch, after request-guard has
// validated the real compressed wire request. It never opens a socket.
import { writeFileSync } from "node:fs";
import { zstdDecompressSync } from "node:zlib";
import { ProbeFailure, record } from "./credentials.mjs";

/**
 * A base64 segment that both `atob` (the pinned adapter) and base64url
 * readers accept, so the synthetic token needs no alphabet translation.
 * @param {Record<string, unknown>} value
 */
function segment(value) {
  for (let nonce = 0; ; nonce++) {
    const text = Buffer.from(JSON.stringify({ ...value, jti: "synthetic-" + nonce })).toString("base64").replace(/=+$/, "");
    if (!/[+/]/.test(text)) return text;
  }
}

/**
 * Writes a synthetic Codex-shaped ChatGPT auth file. It is not a credential:
 * the signature is a fixed string, the account is synthetic and the fake
 * transport is the only destination that accepts it.
 * @param {string} path @param {number} nowMs
 */
export function writeSyntheticAuth(path, nowMs) {
  const access = [segment({ alg: "none", typ: "JWT" }), segment({ exp: Math.floor(nowMs / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic-account" } }), "synthetic-signature"].join(".");
  writeFileSync(path, JSON.stringify({ auth_mode: "chatgpt", tokens: { access_token: access,
    refresh_token: "synthetic-refresh-never-used", account_id: "synthetic-account" } }) + "\n", { mode: 0o600, flag: "wx" });
}

/**
 * @typedef {{input: number, cached?: number, cacheWrite?: number, output: number, reasoning?: number}} FakeUsage
 * @typedef {{text?: string, toolCalls?: Array<{callId: string, name: string, arguments: Record<string, unknown>}>,
 * usage?: FakeUsage}} FakeStep
 */

/** @param {unknown} body */
function lastUserText(body) {
  if (!record(body) || !Array.isArray(body.input)) return "";
  for (let index = body.input.length - 1; index >= 0; index--) {
    const item = body.input[index];
    if (!record(item) || item.role !== "user") continue;
    if (typeof item.content === "string") return item.content;
    if (Array.isArray(item.content)) {
      return item.content.map((part) => record(part) && typeof part.text === "string" ? part.text : "").join("\n");
    }
  }
  return "";
}

/** @param {RequestInit | undefined} init */
function requestBody(init) {
  let bytes;
  if (typeof init?.body === "string") bytes = Buffer.from(init.body);
  else if (init?.body instanceof Uint8Array) bytes = Buffer.from(init.body);
  else throw new ProbeFailure("fake_transport_body");
  if (new Headers(init.headers).get("content-encoding") === "zstd") {
    bytes = zstdDecompressSync(bytes, { maxOutputLength: 65_536 });
  }
  return JSON.parse(bytes.toString("utf8"));
}

/**
 * Serves Responses-API SSE frames: scripted steps first, then an echo of the
 * last `Reply only X.` prompt. `holdNext` keeps the next response stream open
 * after it starts, until `release` or the reader's cancellation; `failNext`
 * answers the next request with HTTP 503.
 * @param {{receipt: (value: Record<string, unknown>) => void, onHold?: () => void, script?: FakeStep[]}} options
 */
export function fakeCodexTransport(options) {
  const script = [...(options.script ?? [])];
  let requests = 0;
  let aborted = 0;
  let failures = 0;
  let holdNext = false;
  let failNext = false;
  /** @type {(() => void) | undefined} */
  let release;
  const encoder = new TextEncoder();

  /** @type {typeof globalThis.fetch} */
  const send = async (_input, init) => {
    const sequence = ++requests;
    const body = requestBody(init);
    if (failNext) {
      failNext = false;
      failures++;
      options.receipt({ type: "fake_transport_failure", sequence, status: 503 });
      return new Response(JSON.stringify({ error: { type: "server_error", message: "fake transport injected failure" } }),
        { status: 503, headers: { "content-type": "application/json" } });
    }
    const step = script.shift() ?? { text: /Reply only ([A-Z_]+)\./.exec(lastUserText(body))?.[1] ?? "FAKE_TRANSPORT_OK" };
    const hold = holdNext;
    holdNext = false;
    const id = `resp_fake_${sequence}`;
    const usage = { cached: 0, cacheWrite: 0, reasoning: 0, ...(step.usage ?? { input: 32, output: 3 }) };
    const inputTokens = usage.input + usage.cached + usage.cacheWrite;
    /** @type {Array<Record<string, unknown>>} */
    const items = [];
    if (step.text !== undefined) {
      items.push({ type: "message", id: `msg_fake_${sequence}`, role: "assistant", status: "completed",
        content: [{ type: "output_text", text: step.text, annotations: [] }] });
    }
    for (const call of step.toolCalls ?? []) {
      items.push({ type: "function_call", id: `fc_fake_${sequence}_${call.callId}`, call_id: call.callId,
        name: call.name, arguments: JSON.stringify(call.arguments), status: "completed" });
    }
    options.receipt({ type: "fake_transport_request", sequence, responseId: id, held: hold,
      encoding: new Headers(init?.headers).get("content-encoding") ?? "identity" });
    /** @type {(() => void) | undefined} */
    let cancelHold;
    async function* frames() {
      yield { type: "response.created", response: { id, status: "in_progress" } };
      for (const [index, item] of items.entries()) {
        yield { type: "response.output_item.added", output_index: index,
          item: item.type === "message" ? { ...item, content: [] } : { ...item, arguments: "" } };
        if (hold && index === 0) {
          await new Promise((resolve, reject) => {
            release = () => { release = undefined; cancelHold = undefined; resolve(undefined); };
            cancelHold = () => { release = undefined; cancelHold = undefined; reject(new ProbeFailure("fake_transport_aborted")); };
            init?.signal?.addEventListener("abort", () => cancelHold?.(), { once: true });
            options.onHold?.();
          });
        }
        if (item.type === "message") yield { type: "response.output_text.delta", output_index: index, delta: step.text };
        yield { type: "response.output_item.done", output_index: index, item };
      }
      yield { type: "response.completed", response: { id, status: "completed", usage: {
        input_tokens: inputTokens, input_tokens_details: { cached_tokens: usage.cached, cache_write_tokens: usage.cacheWrite },
        output_tokens: usage.output, output_tokens_details: { reasoning_tokens: usage.reasoning },
        total_tokens: inputTokens + usage.output } } };
      options.receipt({ type: "fake_transport_completed", sequence, responseId: id });
    }
    const iterator = frames();
    return new Response(new ReadableStream({
      async pull(controller) {
        try {
          const next = await iterator.next();
          if (next.done) controller.close();
          else controller.enqueue(encoder.encode(`data: ${JSON.stringify(next.value)}\n\n`));
        } catch (error) {
          if (!(error instanceof ProbeFailure)) throw error;
          aborted++;
          options.receipt({ type: "fake_transport_aborted", sequence, responseId: id });
          try { controller.error(error); } catch { /* the reader already cancelled the stream */ }
        }
      },
      cancel() { cancelHold?.(); },
    }), { status: 200, headers: { "content-type": "text/event-stream" } });
  };

  return {
    send,
    holdNext() { holdNext = true; },
    failNext() { failNext = true; },
    release() {
      if (!release) throw new ProbeFailure("no_held_provider", "upstream_incompatibility");
      release();
    },
    get requests() { return requests; },
    get aborted() { return aborted; },
    get failures() { return failures; },
  };
}
