import { expect, test } from "bun:test";
import { providerErrorClass } from "../src/host/session.ts";
import { type FailureMessage, normalizeFailure, withProviderFailures } from "../src/host/provider-failure.ts";
import { createAssistantMessageEventStream, normalizeContext, type StreamFunction } from "@earendil-works/pi-ai";
import { getModel } from "@earendil-works/pi-ai/compat";
import { emptyOutput } from "../src/claude/events.ts";

// A failed provider turn with unknown wording must stop, not consume transient retries.
test("unknown provider failures are not transient", () => {
  expect(providerErrorClass("unrecognized failure format")).toBe("unknown");
});

test("native failure evidence distinguishes subscription exhaustion from 429 and paid billing without retaining secrets", () => {
  const at = 1_800_000_000_000;
  const codex = normalizeFailure("openai-subscription", { status: 429,
    body: { error: { type: "usage_limit_reached", resets_at: 1_800_000_600, message: "sk-secret must not persist" } },
    headers: new Headers({ "retry-after": "3", "x-request-id": "req_secret" }) }, at);
  expect(codex).toMatchObject({ version: 1, connection: "openai-subscription", category: "subscription_quota", scope: "account",
    reset_at: "2027-01-15T08:10:00.000Z", retry_after_secs: 3 });
  expect(codex.request_ref).toMatch(/^[a-f0-9]{64}$/);
  const cases = [
    ["openai-subscription", { status: 429, body: { error: { type: "rate_limit_exceeded", message: "usage limit quota exceeded" } } }, "transient"],
    ["openai-subscription", { status: 401, body: { error: { type: "usage_limit_reached" } } }, "auth"],
    ["openai-api", { status: 429, body: { error: { code: "insufficient_quota" } } }, "billing"],
    ["anthropic-api", { status: 400, body: { error: { type: "invalid_request_error", message: "credit balance is too low" } } }, "billing"],
    ["openai-subscription", { status: 400, body: { error: { type: "invalid_request_error" } } }, "invalid_request"],
    ["openai-subscription", { message: "You have hit your ChatGPT usage limit. Try again in 5 minutes." }, "unknown"],
    ["openai-subscription", { message: "tool says quota exhausted" }, "unknown"],
    ["anthropic-subscription", { sdk_error: "rate_limit" }, "transient"],
    ["anthropic-subscription", { sdk_error: "rate_limit", rate_limit: { status: "allowed_warning", rateLimitType: "five_hour" } }, "transient"],
    ["anthropic-subscription", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1_800_000_600 } }, "subscription_quota"],
    ["anthropic-subscription", { sdk_error: "billing_error", rate_limit: { status: "rejected", rateLimitType: "five_hour" } }, "billing"],
    ["anthropic-subscription", { sdk_error: "unknown", rate_limit: { status: "rejected", rateLimitType: "novel_period" } }, "unknown"],
    ["anthropic-subscription", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "novel_period" } }, "unknown"],
    ["anthropic-subscription", { sdk_error: "rate_limit", rate_limit: { status: "rejected", rateLimitType: "five_hour", errorCode: "credits_required" } }, "billing"],
  ] as const;
  for (const [connection, input, category] of cases) expect(normalizeFailure(connection, input, at).category).toBe(category);
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
      error.errorMessage = "You have hit your ChatGPT usage limit. sk-secret";
      output.push({ type: "error", reason: "error", error }); output.end();
    })();
    return output;
  };
  const run = withProviderFailures("openai-subscription", source);
  for (const type of ["usage_limit_reached", "rate_limit_exceeded", "new_error"] as const) {
    const stream = run(model, normalizeContext({ messages: [] }), { fetch: async () => Response.json({ error: { type } }, { status: type === "new_error" ? 418 : 429 }) });
    const result = await stream.result() as FailureMessage;
    expect(result.providerFailure?.category).toBe(type === "usage_limit_reached" ? "subscription_quota" : type === "rate_limit_exceeded" ? "transient" : "unknown");
    expect(JSON.stringify(result)).not.toContain("sk-secret");
  }
});
