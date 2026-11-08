// Native failure evidence, not model/tool prose. Only allowlisted fields cross the host boundary.
import { createHash } from "node:crypto";
import { type StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import type { ConnectionId } from "../auth/connections.ts";

export interface ProviderFailure {
  readonly version: 1;
  readonly connection: ConnectionId;
  readonly category: "subscription_quota" | "transient" | "auth" | "billing" | "invalid_request" | "unavailable" | "unknown" | "local_budget" | "network_policy";
  readonly scope: "account" | "model" | "request" | "unknown";
  readonly retry_after_secs?: number;
  readonly reset_at?: string;
  /** One-way reference: request headers/identifiers may contain secrets. */
  readonly request_ref?: string;
}

export interface FailureInput {
  readonly status?: number;
  readonly body?: unknown;
  readonly headers?: Headers;
  readonly sdk_error?: string;
  readonly rate_limit?: unknown;
  readonly terminal_reason?: unknown;
  readonly request_id?: unknown;
  readonly message?: string;
}

const record = (v: unknown): Record<string, unknown> =>
  v !== null && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : {};
const seconds = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0 && v <= 31_536_000 ? v : undefined;
function reset(v: unknown): string | undefined {
  return typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253_402_300_799
    ? new Date(v * 1000).toISOString() : undefined;
}

export function normalizeFailure(connection: ConnectionId, input: FailureInput, now = Date.now()): ProviderFailure {
  const error = record(record(input.body).error);
  const codes = [error.code, error.type, input.sdk_error];
  const has = (...values: string[]) => codes.some(code => typeof code === "string" && values.includes(code));
  const rate = record(input.rate_limit);
  let category: ProviderFailure["category"] = "unknown";
  let scope: ProviderFailure["scope"] = "unknown";
  let resetAt: string | undefined;
  if (input.message?.startsWith("budget_exhausted:")) category = "local_budget";
  else if (input.message?.startsWith("network_refused:")) category = "network_policy";
  else if (input.message?.startsWith("credentials_unavailable:") || input.status === 401 || input.status === 403
      || has("authentication_failed", "authentication_error", "invalid_api_key", "oauth_org_not_allowed", "cloud_credential_error", "account_on_hold", "verification_required")) {
    category = "auth"; scope = "account";
  } else if (input.status === 402 || has("billing_error", "insufficient_quota", "credit_balance_too_low")
      || connection === "anthropic-api" && has("invalid_request_error") && typeof error.message === "string"
        && /credit balance is too low/i.test(error.message)
      || connection === "anthropic-subscription" && rate.status === "rejected"
        && (rate.rateLimitType === "overage" || rate.isUsingOverage === true || rate.overageInUse === true
          || rate.errorCode === "credits_required" || rate.overageDisabledReason === "out_of_credits" || rate.limitScope === "group_pool")) {
    category = "billing"; scope = "account";
  } else if (has("invalid_request", "invalid_request_error", "context_length_exceeded", "content_filter", "max_output_tokens")
      || input.terminal_reason === "prompt_too_long" || input.terminal_reason === "image_error") {
    category = "invalid_request"; scope = "request";
  } else if (has("model_not_found", "not_found_error", "usage_not_included") || input.status === 404) {
    category = "unavailable"; scope = "model";
  } else if (connection === "openai-subscription" && has("usage_limit_reached")) {
    category = "subscription_quota"; scope = "account"; resetAt = reset(error.resets_at);
  } else if (connection === "anthropic-subscription" && (input.sdk_error === "rate_limit" || input.terminal_reason === "blocking_limit")
      && rate.status === "rejected" && rate.limitScope === undefined && ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_overage_included"].includes(String(rate.rateLimitType))) {
    category = "subscription_quota";
    scope = rate.rateLimitType === "seven_day_opus" || rate.rateLimitType === "seven_day_sonnet" ? "model" : "account";
    resetAt = reset(rate.resetsAt);
  } else if (connection === "anthropic-subscription" && rate.status === "rejected" && input.sdk_error === "rate_limit") {
    // A newly introduced subscription limit shape is not proof of exhaustion.
    category = "unknown";
  } else if (has("rate_limit", "rate_limit_error", "rate_limit_exceeded", "overloaded", "overloaded_error", "server_error")
      || input.status === 429 || input.status !== undefined && input.status >= 500 && input.status <= 599) category = "transient";
  else if (input.status === 400 || input.status === 422) { category = "invalid_request"; scope = "request"; }
  if (input.terminal_reason === "budget_exhausted") { category = "local_budget"; scope = "request"; }
  const retry = input.headers?.get("retry-after");
  const retryMs = input.headers?.get("retry-after-ms");
  const retrySeconds = retryMs !== null && retryMs !== undefined && /^\d+$/.test(retryMs) ? seconds(Math.ceil(Number(retryMs) / 1000))
    : retry === null || retry === undefined ? undefined : /^\d+$/.test(retry)
      ? seconds(Number(retry)) : seconds(Math.max(0, Math.ceil((Date.parse(retry) - now) / 1000)));
  const request = input.request_id ?? input.headers?.get("x-request-id") ?? input.headers?.get("request-id");
  return { version: 1, connection, category, scope,
    ...(retrySeconds !== undefined && (category === "transient" || category === "subscription_quota") ? { retry_after_secs: retrySeconds } : {}),
    ...(resetAt ? { reset_at: resetAt } : {}),
    ...(typeof request === "string" && request.length > 0 && request.length <= 1024
      ? { request_ref: createHash("sha256").update(request).digest("hex") } : {}),
  };
}

export const failureClass = (failure: ProviderFailure): string => failure.category === "transient" ? "rate_limit"
  : failure.category === "subscription_quota" ? "quota" : failure.category;
export const failureSummary = (failure: ProviderFailure): string => `${failure.connection}: ${failure.category} (${failure.scope})`;

export class NativeFailure extends Error {
  constructor(readonly failure: ProviderFailure) { super(failureSummary(failure)); }
}

export type FailureMessage = { errorMessage?: string; providerFailure?: ProviderFailure };
export function messageFailure(message: FailureMessage, connection: ConnectionId): ProviderFailure {
  return message.providerFailure?.connection === connection ? message.providerFailure
    : normalizeFailure(connection, { message: message.errorMessage });
}

// Parse only bounded error bodies. A successful SSE response is never buffered or inspected.
async function errorBody(response: Response): Promise<unknown> {
  const reader = response.clone().body?.getReader();
  if (!reader) return undefined;
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.length;
      if (size > 16_384) return undefined;
      chunks.push(next.value);
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch { return undefined; }
  finally { void reader.cancel().catch(() => undefined); }
}

/** Per-stream evidence, before pinned Codex collapses *every* 429 into quota-looking prose.
 * No recovery loop here; retries remain owned by Pi/provider_stall and paid admission. */
export function withProviderFailures(connection: ConnectionId, stream: StreamFunction): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    let http: FailureInput | undefined;
    const send = options?.fetch ?? globalThis.fetch;
    const fetch: typeof globalThis.fetch = async (input, init) => {
      const response = await send(input, init);
      http = response.ok ? undefined : { status: response.status, headers: response.headers, body: await errorBody(response) };
      return response;
    };
    const source = stream(model, context, { ...options, fetch });
    return (async function* () {
      for await (const event of source) {
        if (event.type === "error" && event.reason === "error") {
          const message = event.error as typeof event.error & FailureMessage;
          const failure = message.providerFailure ?? normalizeFailure(connection, http ?? { message: message.errorMessage });
          message.providerFailure = failure;
          message.errorMessage = failureSummary(failure);
        }
        yield event;
      }
    })();
  });
}
