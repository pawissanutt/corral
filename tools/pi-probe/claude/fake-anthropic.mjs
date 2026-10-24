import { record } from "../credentials.mjs";

/** Unified rate-limit headers of a request that counted against a subscription window. */
export const SUBSCRIPTION_HEADERS = Object.freeze({
  "anthropic-ratelimit-unified-status": "allowed",
  "anthropic-ratelimit-unified-representative-claim": "five_hour",
  "anthropic-ratelimit-unified-5h-utilization": "0.01",
  "anthropic-ratelimit-unified-overage-status": "disabled",
});

/** @param {string} event @param {unknown} data */
const frame = (event, data) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;

/**
 * @typedef {{text?: string, tool?: {input: Record<string, unknown>}, outputTokens?: number,
 * headers?: Record<string, string>, status?: number, error?: {type: string, message: string}}} Step
 */

/**
 * Anthropic Messages SSE answers for the pinned adapter's injected `fetch`. It reads the real request
 * body Pi built, so tool names are echoed exactly as Pi sent them (Pi may rename them for OAuth).
 * @param {{script: Step[]}} options
 */
export function fakeAnthropic(options) {
  /** @type {Record<string, unknown>[]} */
  const requests = [];
  let next = 0;
  /** @type {typeof globalThis.fetch} */
  const send = async (_input, init) => {
    const step = options.script[next++];
    if (!step) throw new Error("fake_script_exhausted");
    /** @type {unknown} */
    const body = JSON.parse(String(init?.body));
    if (!record(body)) throw new Error("fake_request_shape");
    requests.push(body);
    const headers = { "content-type": "text/event-stream", "request-id": "req_fake_" + next, ...(step.headers ?? SUBSCRIPTION_HEADERS) };
    if (step.status && step.status >= 400) {
      return new Response(JSON.stringify({ type: "error", error: step.error ?? { type: "api_error", message: "fake failure" } }),
        { status: step.status, headers: { ...headers, "content-type": "application/json" } });
    }
    const tools = Array.isArray(body.tools) ? body.tools : [];
    const toolName = record(tools[0]) && typeof tools[0].name === "string" ? tools[0].name : "tool";
    const output = step.outputTokens ?? 5;
    let text = frame("message_start", { type: "message_start", message: { id: "msg_fake_" + next, type: "message", role: "assistant",
      model: body.model, content: [], stop_reason: null, usage: { input_tokens: 12, output_tokens: 1 } } });
    if (step.tool) {
      text += frame("content_block_start", { type: "content_block_start", index: 0,
        content_block: { type: "tool_use", id: "toolu_fake_" + next, name: toolName, input: {} } });
      text += frame("content_block_delta", { type: "content_block_delta", index: 0,
        delta: { type: "input_json_delta", partial_json: JSON.stringify(step.tool.input) } });
    } else {
      text += frame("content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
      text += frame("content_block_delta", { type: "content_block_delta", index: 0,
        delta: { type: "text_delta", text: step.text ?? "PROBE_READY" } });
    }
    text += frame("content_block_stop", { type: "content_block_stop", index: 0 });
    text += frame("message_delta", { type: "message_delta", delta: { stop_reason: step.tool ? "tool_use" : "end_turn", stop_sequence: null },
      usage: { output_tokens: output } });
    text += frame("message_stop", { type: "message_stop" });
    return new Response(text, { status: 200, headers });
  };
  return { send, requests };
}
