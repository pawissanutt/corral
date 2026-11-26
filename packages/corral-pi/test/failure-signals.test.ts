import { expect, test } from "bun:test";
import { providerErrorClass } from "../src/host/session.ts";
import { afterPiRetry, type FailureMessage, normalizeFailure, withProviderFailures } from "../src/host/provider-failure.ts";
import { withSubscriptionFallback } from "../src/host/continuation.ts";
import { Agent } from "@earendil-works/pi-agent-core";
import { createAssistantMessageEventStream, normalizeContext, type StreamFunction } from "@earendil-works/pi-ai";
import { Type } from "typebox";
import { getModel, isRetryableAssistantError, streamSimple } from "@earendil-works/pi-ai/compat";
import { emptyOutput } from "../src/claude/events.ts";
import { fakeCodexTransport, requestBody } from "./fixtures/auth/fake-transport.ts";

const codexModel = getModel("openai-codex", "gpt-6.1-sol")!;
const codexToken = `fixture.${Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "fixture" } })).toString("base64url")}.fixture`;
const createdFrame = { type: "response.created", response: { id: "resp_fixture", status: "in_progress" } };
const failedFrame = (code = "novel_failure") => ({ type: "response.failed", response: { error: { code, message: "private-upstream-text" } } });
const sse = (frames: readonly unknown[], requestRef?: string) => new Response(
  frames.map(frame => `data: ${JSON.stringify(frame)}\n\n`).join(""),
  { headers: { "content-type": "text/event-stream", ...(requestRef ? { "x-request-id": requestRef } : {}) } },
);

test("an output-free Codex stream failure replays the same request once", async () => {
  for (const final of ["success", "novel_failure", "server_error", "usage_limit_reached", "length"] as const) {
    const wire = fakeCodexTransport();
    const requests: unknown[] = [];
    wire.script([{ toolCalls: [{ callId: "call_fixture", name: "earlier", arguments: {} }] }]);
    const requestsSeen: string[] = [];
    let payloads = 0, tools = 0;
    const stream = withSubscriptionFallback(withProviderFailures("openai-codex", streamSimple), [{ provider: "openai-codex", model: "other" }],
      async () => { throw new Error("Already replayed; the request must not switch models"); }, () => {});
    const agent = new Agent({
      initialState: { model: codexModel, tools: [{ name: "earlier", label: "Earlier tool", description: "Fixture tool",
        parameters: Type.Object({}), async execute() { tools++; return { content: [{ type: "text", text: "already ran" }], details: {} }; } }] },
      transport: "sse", sessionId: "same-session",
      onPayload: payload => ({ ...(payload as object), fixture_marker: ++payloads }),
      streamFn: (model, context, options) => stream(model, context, { ...options, apiKey: codexToken,
        fetch: async (input, init) => {
          requests.push(requestBody(init));
          if (requests.length === 1) return wire.send(input, init);
          if (requests.length === 2) return sse([createdFrame, failedFrame()]);
          if (final === "success") return wire.send(input, init);
          return sse([final === "length" ? { type: "response.incomplete",
            response: { status: "incomplete", incomplete_details: { reason: "max_output_tokens" } } } : failedFrame(final)], "private-request-id");
        },
      }),
    });
    agent.subscribe(event => { if (event.type === "message_end" && event.message.role === "assistant") requestsSeen.push(event.message.stopReason); });
    await agent.prompt("Finish after the tool.");
    const result = agent.state.messages.findLast(message => message.role === "assistant")!;
    expect(requests).toHaveLength(3);
    expect(requests[2]).toEqual(requests[1]);
    expect(JSON.stringify(requests[2])).toContain("already ran");
    expect(tools).toBe(1);
    expect(requestsSeen).toEqual(["toolUse", final === "success" ? "stop" : "error"]);
    if (final === "success") expect(result.stopReason).toBe("stop");
    else {
      expect(isRetryableAssistantError(result)).toBe(false);
      expect(result.errorMessage).toContain("one retry");
      expect(result.errorMessage).toMatch(/ref [a-f0-9]{8}/);
      if (final === "usage_limit_reached") expect(result.errorMessage).toContain("usage limit to reset");
      if (final === "length") expect((result as FailureMessage).providerFailure?.category).toBe("invalid_request");
      expect(JSON.stringify(result)).not.toMatch(/private-upstream-text|private-request-id/);
    }
  }
});

test("Codex stream failures retain safe evidence for Pi's bounded retry decision", async () => {
  const model = getModel("openai-codex", "gpt-6.1-sol")!;
  for (const [code, partial, text, category, retry] of [
    ["server_error", false, "opaque", "transient", true],
    ["server_error", true, "opaque", "transient", false],
    ["invalid_request_error", false, "fetch failed", "invalid_request", false],
    [undefined, false, "fetch failed", "transient", true],
    [undefined, false, "unrecognized error", "unknown", false],
  ] as const) {
    const source: StreamFunction = (current, _context, options) => {
      const output = createAssistantMessageEventStream();
      void (async () => {
        if (code) await options?.onProviderStreamEvent?.({ type: "response.failed", response: { error: { code, message: "sk-secret" } } }, current);
        const error = emptyOutput(current);
        if (partial) error.content.push({ type: "text", text: "partial" });
        error.stopReason = "error"; error.errorMessage = text;
        output.push({ type: "error", reason: "error", error }); output.end();
      })();
      return output;
    };
    const result = await withProviderFailures("openai-codex", source)(model, normalizeContext({ messages: [] })).result();
    expect((result as FailureMessage).providerFailure?.category).toBe(category);
    expect(isRetryableAssistantError(result)).toBe(retry);
    expect(JSON.stringify(result)).not.toContain("sk-secret");
  }
});

test("Codex does not replay after observed output, tool activity, cancellation, denial, or a previous retry", async () => {
  for (const reason of ["text", "tool", "opaque", "cancel", "policy", "quota", "invalid", "nested-auth", "pi-retry", "native-retry"] as const) {
    const controller = new AbortController();
    let requests = 0;
    const options = {
      apiKey: codexToken, transport: "sse" as const, signal: controller.signal, maxRetries: reason === "native-retry" ? 1 : 0,
      fetch: async () => {
        requests++;
        if (reason === "native-retry" && requests === 1) return new Response("retry", { status: 503, headers: { "retry-after": "0" } });
        const frames: unknown[] = [createdFrame];
        if (reason === "text") frames.push({ type: "response.output_item.added", output_index: 0,
          item: { type: "message", role: "assistant", content: [] } },
        { type: "response.output_text.delta", output_index: 0, delta: "partial" });
        if (reason === "tool") frames.push({ type: "response.output_item.added", output_index: 0,
          item: { type: "web_search_call", status: "completed" } });
        if (reason === "opaque") frames.push({ type: "response.new_lifecycle_event" });
        const code = reason === "policy" ? "content_policy_violation" : reason === "quota" ? "usage_limit_reached"
          : reason === "invalid" ? "missing_required_parameter" : undefined;
        frames.push(reason === "nested-auth" ? { type: "error", code: "novel_failure", message: "private-upstream-text",
          error: { code: "verification_required" } } : failedFrame(code));
        return sse(frames);
      },
      onProviderStreamEvent(event: unknown) {
        if (reason === "cancel" && (event as { type?: string }).type === "response.failed") controller.abort();
      },
    };
    const stream = withProviderFailures("openai-codex", streamSimple)(codexModel, normalizeContext({ messages: [] }),
      reason === "pi-retry" ? afterPiRetry(options) : options);
    const result = await stream.result();
    expect(requests).toBe(reason === "native-retry" ? 2 : 1);
    expect(result.stopReason).toBe(reason === "cancel" ? "aborted" : "error");
    expect((result as FailureMessage).providerFailure?.replayed).toBeUndefined();
    expect(JSON.stringify(result)).not.toContain("private-upstream-text");
    if (reason === "nested-auth") expect((result as FailureMessage).providerFailure?.category).toBe("auth");
    if (reason === "text") {
      expect((result as FailureMessage).providerFailure?.output_observed).toBe(true);
      expect(result.errorMessage).toContain("stopped early");
    }
  }
});

for (const [code, category, dispatches] of [
  ["invalid_image", "invalid_request", 1],
  ["image_too_large", "invalid_request", 1],
  ["permission_denied", "auth", 1],
  ["novel_failure", "unknown", 2],
] as const) {
  test(`output-free Codex SSE ${code} dispatches ${dispatches} request(s)`, async () => {
    let requests = 0;
    const result = await withProviderFailures("openai-codex", streamSimple)(codexModel, normalizeContext({ messages: [] }), {
      apiKey: codexToken, transport: "sse", maxRetries: 0,
      fetch: async () => {
        requests++;
        return sse([{ type: "response.failed", response: { status: "failed", output: [], error: { code, message: "opaque" } } }]);
      },
    }).result();
    expect(requests).toBe(dispatches);
    expect((result as FailureMessage).providerFailure?.category).toBe(category);
    expect((result as FailureMessage).providerFailure?.replayed).toBe(dispatches === 2 ? true : undefined);
  });
}

// Unknown wording is not a hint for Pi's whole-agent retry machinery.
test("unknown provider failures are not transient", () => {
  expect(providerErrorClass("unrecognized failure format")).toBe("unknown");
});

test("native failure evidence distinguishes subscription exhaustion from 429 and paid billing without retaining secrets", () => {
  const at = 1_800_000_000_000;
  const codex = normalizeFailure("openai-codex", { status: 429,
    body: { error: { type: "usage_limit_reached", resets_at: 1_800_000_600, message: "sk-secret must not persist" } },
    headers: new Headers({ "retry-after": "3", "x-request-id": "req_secret" }) }, at);
  expect(codex).toMatchObject({ version: 1, connection: "openai-subscription", category: "subscription_quota", scope: "account",
    reset_at: "2027-01-15T08:10:00.000Z", retry_after_secs: 3 });
  expect(codex.request_ref).toMatch(/^[a-f0-9]{64}$/);
  const cases = [
    ["openai-codex", { status: 429, body: { error: { type: "rate_limit_exceeded", message: "usage limit quota exceeded" } } }, "transient"],
    ["openai-codex", { status: 401, body: { error: { type: "usage_limit_reached" } } }, "auth"],
    ["openai", { status: 429, body: { error: { code: "insufficient_quota" } } }, "billing"],
    ["anthropic-api", { status: 400, body: { error: { type: "invalid_request_error", message: "credit balance is too low" } } }, "billing"],
    ["openai-codex", { status: 400, body: { error: { type: "invalid_request_error" } } }, "invalid_request"],
    ["openai-codex", { message: "You have hit your ChatGPT usage limit. Try again in 5 minutes." }, "unknown"],
    ["openai-codex", { message: "tool says quota exhausted" }, "unknown"],
    ["anthropic", { sdk_error: "rate_limit" }, "transient"],
    ["anthropic", { sdk_error: "rate_limit", rate_limit: { status: "allowed_warning", rateLimitType: "five_hour" } }, "transient"],
    ["anthropic", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1_800_000_600 } }, "subscription_quota"],
    ["anthropic", { sdk_error: "billing_error", rate_limit: { status: "rejected", rateLimitType: "five_hour" } }, "billing"],
    ["anthropic", { sdk_error: "unknown", rate_limit: { status: "rejected", rateLimitType: "novel_period" } }, "unknown"],
    ["anthropic", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "novel_period" } }, "unknown"],
    ["anthropic", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "five_hour", errorCode: "credits_required" } }, "billing"],
    ["anthropic", { terminal_reason: "blocking_limit", status: 429, rate_limit: { status: "rejected", rateLimitType: "five_hour" } }, "subscription_quota"],
    ["anthropic", { terminal_reason: "blocking_limit", status: 401, rate_limit: { status: "rejected" } }, "auth"],
    ["anthropic", { terminal_reason: "blocking_limit", status: 429, rate_limit: { status: "rejected", errorCode: "credits_required" } }, "billing"],
    ["anthropic", { terminal_reason: "blocking_limit", status: 429, sdk_subtype: "error_max_turns", rate_limit: { status: "rejected" } }, "invalid_request"],
    ["anthropic", { status: 429 }, "transient"],
  ] as const;
  for (const [provider, input, category] of cases) expect(normalizeFailure(provider, input, at).category).toBe(category);
  // F1: terminal blocking-limit results must not turn unsupported rejected evidence into retries.
  for (const rate_limit of [
    { status: "rejected", rateLimitType: "novel_period" },
    { status: "rejected" },
    { status: "rejected", rateLimitType: "five_hour", limitScope: "service" },
    { status: "rejected", rateLimitType: "five_hour", limitScope: "channel" },
  ]) {
    const assistant = normalizeFailure("anthropic", { sdk_error: "rate_limit", rate_limit }, at);
    const terminal = normalizeFailure("anthropic", { terminal_reason: "blocking_limit", status: 429, rate_limit }, at);
    expect({ assistant: assistant.category, terminal: terminal.category }).toEqual({ assistant: "unknown", terminal: "unknown" });
  }
  expect(JSON.stringify(codex)).not.toContain("secret");
});

test("per-request structured evidence wins over Codex's quota-looking 429 rewrite and cannot leak between streams", async () => {
  const model = getModel("openai-codex", "gpt-6.1-sol")!;
  const source: StreamFunction = (current, _context, options) => {
    const output = createAssistantMessageEventStream();
    void (async () => {
      await options!.fetch!("https://chatgpt.com/backend-api/codex/responses");
      const error = emptyOutput(current);
      error.stopReason = "error";
      error.errorMessage = current.id === "local-invariant"
        ? "claude_compaction_conflict: sk-secret" : "You have hit your ChatGPT usage limit. sk-secret";
      output.push({ type: "error", reason: "error", error }); output.end();
    })();
    return output;
  };
  const run = withProviderFailures("openai-codex", source);
  for (const type of ["usage_limit_reached", "rate_limit_exceeded", "new_error"] as const) {
    const stream = run(type === "new_error" ? { ...model, id: "local-invariant" } : model, normalizeContext({ messages: [] }),
      { fetch: async () => Response.json({ error: { type } }, { status: type === "new_error" ? 418 : 429 }) });
    const result = await stream.result() as FailureMessage;
    expect(result.providerFailure?.category).toBe(type === "usage_limit_reached" ? "subscription_quota" : type === "rate_limit_exceeded" ? "transient" : "unknown");
    expect(JSON.stringify(result)).not.toContain("sk-secret");
    if (type === "new_error") expect(result.errorMessage?.split(":")[0]).toBe("claude_compaction_conflict");
  }
});
