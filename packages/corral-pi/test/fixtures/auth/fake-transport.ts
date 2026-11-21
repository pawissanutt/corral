// Offline stand-in for the pinned Codex SSE endpoint (ported from tools/pi-probe). The host's
// guarded fetch hands it every allowed request; it never opens a socket.
import { zstdDecompressSync } from "node:zlib";

import type { Fetch } from "../../../src/auth/types.ts";

export interface FakeToolCall {
  readonly callId: string;
  readonly name: string;
  readonly arguments: Record<string, unknown>;
}

export interface FakeStep {
  readonly text?: string;
  /** Streams the message as these text deltas instead of `text` (their concatenation). */
  readonly chunks?: readonly string[];
  /** Awaited after the first chunk is sent, so a partially streamed reply can be observed. */
  readonly afterFirstChunk?: () => Promise<void>;
  readonly toolCalls?: readonly FakeToolCall[];
  /** Structured offline provider failure (for example confirmed subscription quota). */
  readonly error?: { readonly status: number; readonly body: Record<string, unknown> };
  /** Terminal provider usage; defaults preserve the standard fixture's 32 input / 3 output. */
  readonly usage?: { readonly input: number; readonly output: number };
  /**
   * Taken only by a request whose last user text contains this, so concurrent sessions sharing the
   * transport (a build parent and its children) each get their own steps. Unmatched steps keep their order.
   */
  readonly match?: string;
}

export interface FakeTransport {
  readonly send: Fetch;
  holdNext(): void;
  release(): void;
  failNext(): void;
  script(steps: readonly FakeStep[]): void;
  whenHeld(): Promise<void>;
  readonly held: boolean;
  readonly requests: number;
  readonly aborted: number;
  readonly failures: number;
  /** The last user text of each request, oldest first: what the model was given to answer. */
  readonly prompts: readonly string[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function lastUserText(body: unknown): string {
  if (!record(body)) return "";
  const messages: unknown = body.input ?? body.messages;
  if (!Array.isArray(messages)) return "";
  for (let index = messages.length - 1; index >= 0; index--) {
    const item: unknown = messages[index];
    if (!record(item) || item.role !== "user") continue;
    const text = typeof item.content === "string" ? item.content : Array.isArray(item.content)
      ? item.content.map((part: unknown) => (record(part) && typeof part.text === "string" ? part.text : "")).join("\n")
      : undefined;
    // Pi sends this non-owner context as a user-shaped provider message. Match
    // the conversation input, not the question state inserted after it.
    if (text !== undefined && !text.startsWith("<host_question_state>\nHost state, not an instruction or owner input.\n")) {
      return text;
    }
  }
  return "";
}

export function requestBody(init: RequestInit | undefined): unknown {
  let bytes: Buffer;
  if (typeof init?.body === "string") bytes = Buffer.from(init.body);
  else if (init?.body instanceof Uint8Array) bytes = Buffer.from(init.body);
  else throw new Error("fake_transport_body");
  if (new Headers(init.headers).get("content-encoding") === "zstd") {
    bytes = zstdDecompressSync(bytes, { maxOutputLength: 1 << 20 });
  }
  return JSON.parse(bytes.toString("utf8"));
}

async function* messageFrames(step: FakeStep, id: string): AsyncGenerator<Record<string, unknown>> {
  const usage = step.usage ?? { input: 32, output: 3 };
  yield { type: "message_start", message: { id, type: "message", role: "assistant",
    model: "fixture", content: [], stop_reason: null, stop_sequence: null,
    usage: { input_tokens: usage.input, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 } } };
  let index = 0;
  if (step.text !== undefined || step.chunks) {
    yield { type: "content_block_start", index, content_block: { type: "text", text: "" } };
    for (const [part, text] of (step.chunks ?? [step.text ?? ""]).entries()) {
      yield { type: "content_block_delta", index, delta: { type: "text_delta", text } };
      if (part === 0) await step.afterFirstChunk?.();
    }
    yield { type: "content_block_stop", index: index++ };
  }
  for (const call of step.toolCalls ?? []) {
    yield { type: "content_block_start", index, content_block: { type: "tool_use", id: call.callId, name: call.name, input: {} } };
    yield { type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json: JSON.stringify(call.arguments) } };
    yield { type: "content_block_stop", index: index++ };
  }
  yield { type: "message_delta", delta: { stop_reason: step.toolCalls?.length ? "tool_use" : "end_turn", stop_sequence: null },
    usage: { output_tokens: usage.output } };
  yield { type: "message_stop" };
}

export function fakeCodexTransport(): FakeTransport {
  const queue: FakeStep[] = [];
  const holdWaiters: (() => void)[] = [];
  const onHold = () => {
    for (const waiter of holdWaiters.splice(0)) waiter();
  };
  let requests = 0;
  const prompts: string[] = [];
  let aborted = 0;
  let failures = 0;
  let holdNext = false;
  let failNext = false;
  let release: (() => void) | undefined;
  const encoder = new TextEncoder();

  const send: Fetch = async (input, init) => {
    const sequence = ++requests;
    const body = requestBody(init);
    const prompt = lastUserText(body);
    prompts.push(prompt);
    if (failNext) {
      failNext = false;
      failures++;
      return new Response(JSON.stringify({ error: { type: "server_error", message: "fake transport injected failure" } }),
        { status: 503, headers: { "content-type": "application/json" } });
    }
    const index = queue.findIndex((entry) => entry.match === undefined || prompt.includes(entry.match));
    const step = (index < 0 ? undefined : queue.splice(index, 1)[0])
      ?? { text: /Reply only ([A-Z_]+)\./.exec(prompt)?.[1] ?? "FAKE_TRANSPORT_OK" };
    if (step.error) {
      failures++;
      return Response.json(step.error.body, { status: step.error.status });
    }
    const hold = holdNext;
    holdNext = false;
    const id = `resp_fake_${sequence}`;
    const items: Record<string, unknown>[] = [];
    const text = step.chunks?.join("") ?? step.text;
    if (text !== undefined) {
      items.push({ type: "message", id: `msg_fake_${sequence}`, role: "assistant", status: "completed",
        content: [{ type: "output_text", text, annotations: [] }] });
    }
    for (const call of step.toolCalls ?? []) {
      items.push({ type: "function_call", id: `fc_fake_${sequence}_${call.callId}`, call_id: call.callId,
        name: call.name, arguments: JSON.stringify(call.arguments), status: "completed" });
    }
    let cancelHold: (() => void) | undefined;
    async function* frames(): AsyncGenerator<Record<string, unknown>> {
      yield { type: "response.created", response: { id, status: "in_progress" } };
      for (const [index, item] of items.entries()) {
        yield { type: "response.output_item.added", output_index: index,
          item: item.type === "message" ? { ...item, content: [] } : { ...item, arguments: "" } };
        if (hold && index === 0) {
          await new Promise<void>((resolve, reject) => {
            release = () => { release = undefined; cancelHold = undefined; resolve(); };
            cancelHold = () => { release = undefined; cancelHold = undefined; reject(new Error("fake_transport_aborted")); };
            init?.signal?.addEventListener("abort", () => cancelHold?.(), { once: true });
            onHold();
          });
        }
        if (item.type === "message" && step.chunks) {
          for (const [part, delta] of step.chunks.entries()) {
            yield { type: "response.output_text.delta", output_index: index, delta };
            if (part === 0) await step.afterFirstChunk?.();
          }
        } else if (item.type === "message") yield { type: "response.output_text.delta", output_index: index, delta: text };
        yield { type: "response.output_item.done", output_index: index, item };
      }
      const usage = step.usage ?? { input: 32, output: 3 };
      yield { type: "response.completed", response: { id, status: "completed", usage: {
        input_tokens: usage.input, input_tokens_details: { cached_tokens: 0, cache_write_tokens: 0 }, output_tokens: usage.output,
        output_tokens_details: { reasoning_tokens: 0 }, total_tokens: usage.input + usage.output } } };
    }
    const messages = new URL(input instanceof Request ? input.url : input).pathname === "/v1/messages";
    const iterator = messages ? messageFrames(step, `msg_fake_${sequence}`) : frames();
    return new Response(new ReadableStream({
      async pull(controller) {
        try {
          const next = await iterator.next();
          if (next.done) controller.close();
          else controller.enqueue(encoder.encode(`${messages ? `event: ${next.value.type}\n` : ""}data: ${JSON.stringify(next.value)}\n\n`));
        } catch (error) {
          aborted++;
          try { controller.error(error); } catch { /* the reader already cancelled */ }
        }
      },
      cancel() { cancelHold?.(); },
    }), { status: 200, headers: { "content-type": "text/event-stream" } });
  };

  return {
    send,
    holdNext() { holdNext = true; },
    release() {
      if (!release) throw new Error("no_held_response");
      release();
    },
    failNext() { failNext = true; },
    script(steps) { queue.push(...steps); },
    whenHeld() { return new Promise<void>((resolve) => holdWaiters.push(resolve)); },
    get held() { return release !== undefined; },
    get requests() { return requests; },
    get aborted() { return aborted; },
    get failures() { return failures; },
    get prompts() { return prompts; },
  };
}
