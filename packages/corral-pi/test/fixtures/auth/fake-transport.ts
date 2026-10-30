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
  readonly toolCalls?: readonly FakeToolCall[];
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
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function lastUserText(body: unknown): string {
  if (!record(body) || !Array.isArray(body.input)) return "";
  for (let index = body.input.length - 1; index >= 0; index--) {
    const item: unknown = body.input[index];
    if (!record(item) || item.role !== "user") continue;
    if (typeof item.content === "string") return item.content;
    if (Array.isArray(item.content)) {
      return item.content.map((part: unknown) => (record(part) && typeof part.text === "string" ? part.text : "")).join("\n");
    }
  }
  return "";
}

function requestBody(init: RequestInit | undefined): unknown {
  let bytes: Buffer;
  if (typeof init?.body === "string") bytes = Buffer.from(init.body);
  else if (init?.body instanceof Uint8Array) bytes = Buffer.from(init.body);
  else throw new Error("fake_transport_body");
  if (new Headers(init.headers).get("content-encoding") === "zstd") {
    bytes = zstdDecompressSync(bytes, { maxOutputLength: 1 << 20 });
  }
  return JSON.parse(bytes.toString("utf8"));
}

export function fakeCodexTransport(): FakeTransport {
  const queue: FakeStep[] = [];
  const holdWaiters: (() => void)[] = [];
  const onHold = () => {
    for (const waiter of holdWaiters.splice(0)) waiter();
  };
  let requests = 0;
  let aborted = 0;
  let failures = 0;
  let holdNext = false;
  let failNext = false;
  let release: (() => void) | undefined;
  const encoder = new TextEncoder();

  const send: Fetch = async (_input, init) => {
    const sequence = ++requests;
    const body = requestBody(init);
    if (failNext) {
      failNext = false;
      failures++;
      return new Response(JSON.stringify({ error: { type: "server_error", message: "fake transport injected failure" } }),
        { status: 503, headers: { "content-type": "application/json" } });
    }
    const step = queue.shift() ?? { text: /Reply only ([A-Z_]+)\./.exec(lastUserText(body))?.[1] ?? "FAKE_TRANSPORT_OK" };
    const hold = holdNext;
    holdNext = false;
    const id = `resp_fake_${sequence}`;
    const items: Record<string, unknown>[] = [];
    if (step.text !== undefined) {
      items.push({ type: "message", id: `msg_fake_${sequence}`, role: "assistant", status: "completed",
        content: [{ type: "output_text", text: step.text, annotations: [] }] });
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
        if (item.type === "message") yield { type: "response.output_text.delta", output_index: index, delta: step.text };
        yield { type: "response.output_item.done", output_index: index, item };
      }
      yield { type: "response.completed", response: { id, status: "completed", usage: {
        input_tokens: 32, input_tokens_details: { cached_tokens: 0 }, output_tokens: 3,
        output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 35 } } };
    }
    const iterator = frames();
    return new Response(new ReadableStream({
      async pull(controller) {
        try {
          const next = await iterator.next();
          if (next.done) controller.close();
          else controller.enqueue(encoder.encode(`data: ${JSON.stringify(next.value)}\n\n`));
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
  };
}
