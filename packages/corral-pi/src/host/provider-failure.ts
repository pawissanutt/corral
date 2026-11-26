// Native failure evidence, not model/tool prose. Only allowlisted fields cross the host boundary.
import { createHash } from "node:crypto";
import type { AssistantMessageEvent, StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import type { ProviderId } from "../auth/providers.ts";
import { codexFailureCopy } from "./codex-failure-copy.ts";

// Keep the version-1 diagnostic vocabulary for existing transcript readers. It does not select auth.
const FAILURE_CONNECTION = {
  "openai-codex": "openai-subscription", openai: "openai-api",
  anthropic: "anthropic-subscription", "anthropic-api": "anthropic-api",
  "amazon-bedrock": "amazon-bedrock", "llama.cpp": "llama.cpp",
} as const satisfies Record<ProviderId, string>;

export interface ProviderFailure {
  readonly version: 1;
  readonly connection: (typeof FAILURE_CONNECTION)[ProviderId];
  readonly category: "subscription_quota" | "transient" | "auth" | "billing" | "invalid_request" | "unavailable" | "unknown" | "local_budget" | "network_policy";
  readonly scope: "account" | "model" | "request" | "unknown";
  readonly retry_after_secs?: number;
  readonly reset_at?: string;
  /** One-way reference: request headers/identifiers may contain secrets. */
  readonly request_ref?: string;
  /** The provider adapter dispatched the one allowed replay, not a Pi turn retry. */
  readonly replayed?: true;
  /** An output event was seen even if the terminal message no longer carries it. */
  readonly output_observed?: true;
}

export interface FailureInput {
  readonly status?: number;
  readonly body?: unknown;
  readonly headers?: Headers;
  readonly sdk_error?: string;
  readonly sdk_subtype?: string;
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

export function normalizeFailure(provider: ProviderId, input: FailureInput, now = Date.now()): ProviderFailure {
  const error = record(record(input.body).error);
  const codes = [error.code, error.type, input.sdk_error];
  const has = (...values: string[]) => codes.some(code => typeof code === "string" && values.includes(code));
  const rate = record(input.rate_limit);
  const claudeLimitFailure = input.sdk_error === "rate_limit" || input.terminal_reason === "blocking_limit";
  let category: ProviderFailure["category"] = "unknown";
  let scope: ProviderFailure["scope"] = "unknown";
  let resetAt: string | undefined;
  if (input.message?.startsWith("budget_exhausted:")) category = "local_budget";
  else if (input.message?.startsWith("network_refused:")) category = "network_policy";
  else if (input.message?.startsWith("credentials_unavailable:") || input.status === 401 || input.status === 403
      || has("authentication_failed", "authentication_error", "invalid_api_key", "invalid_token", "token_expired", "oauth_org_not_allowed", "cloud_credential_error", "account_on_hold", "verification_required")) {
    category = "auth"; scope = "account";
  } else if (input.status === 402 || has("billing_error", "insufficient_quota", "credit_balance_too_low")
      || provider === "anthropic-api" && has("invalid_request_error") && typeof error.message === "string"
        && /credit balance is too low/i.test(error.message)
      || provider === "anthropic" && rate.status === "rejected"
        && (rate.rateLimitType === "overage" || rate.isUsingOverage === true || rate.overageInUse === true
          || rate.errorCode === "credits_required" || rate.overageDisabledReason === "out_of_credits" || rate.limitScope === "group_pool")) {
    category = "billing"; scope = "account";
  } else if (has("invalid_request", "invalid_request_error", "context_length_exceeded", "content_filter", "content_policy_violation",
      "policy_violation", "safety_violation", "max_output_tokens", "missing_required_parameter", "unsupported_parameter",
      "unknown_parameter", "invalid_parameter", "invalid_value", "invalid_type", "unsupported_value", "invalid_json", "invalid_prompt")
      || input.sdk_subtype === "error_max_turns" || input.terminal_reason === "prompt_too_long" || input.terminal_reason === "image_error") {
    category = "invalid_request"; scope = "request";
  } else if (has("model_not_found", "not_found_error", "usage_not_included") || input.status === 404) {
    category = "unavailable"; scope = "model";
  } else if (provider === "openai-codex" && has("usage_limit_reached", "subscription_sharing_usage_limit_exceeded", "subscription_sharing_workplace_individual_usage_limit_exceeded")) {
    category = "subscription_quota"; scope = "account"; resetAt = reset(error.resets_at);
  } else if (provider === "anthropic" && claudeLimitFailure
      && rate.status === "rejected" && rate.limitScope === undefined && ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_overage_included"].includes(String(rate.rateLimitType))) {
    category = "subscription_quota";
    scope = rate.rateLimitType === "seven_day_opus" || rate.rateLimitType === "seven_day_sonnet" ? "model" : "account";
    resetAt = reset(rate.resetsAt);
  } else if (provider === "anthropic" && rate.status === "rejected" && claudeLimitFailure) {
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
  return { version: 1, connection: FAILURE_CONNECTION[provider], category, scope,
    ...(retrySeconds !== undefined && (category === "transient" || category === "subscription_quota") ? { retry_after_secs: retrySeconds } : {}),
    ...(resetAt ? { reset_at: resetAt } : {}),
    ...(typeof request === "string" && request.length > 0 && request.length <= 1024
      ? { request_ref: createHash("sha256").update(request).digest("hex") } : {}),
  };
}

export const failureClass = (failure: ProviderFailure): string => failure.category === "transient" ? "rate_limit"
  : failure.category === "subscription_quota" ? "quota" : failure.category;
export function failureSummary(failure: ProviderFailure, message?: string): string {
  // Keep only fixed local invariant codes, never the raw suffix (which may be provider text).
  const diagnostic = message?.match(/^(claude_compaction_conflict|claude_query_incomplete|search turn limit):/)?.[1];
  return `${diagnostic ? `${diagnostic}: ` : ""}${failure.connection}: ${failure.category} (${failure.scope})`;
}

export class NativeFailure extends Error {
  constructor(readonly failure: ProviderFailure, diagnostic?: string) { super(failureSummary(failure, diagnostic)); }
}

export type FailureMessage = { errorMessage?: string; providerFailure?: ProviderFailure };
export function messageFailure(message: FailureMessage, provider: ProviderId): ProviderFailure {
  return message.providerFailure?.connection === FAILURE_CONNECTION[provider] ? message.providerFailure
    : normalizeFailure(provider, { message: message.errorMessage });
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

// Pinned Codex can fail after HTTP 200, or before it has a response at all. Match only
// known native diagnostics, not arbitrary mentions of timeouts, quota or retry in prose.
function codexTransportFailure(message: string | undefined): boolean {
  return !!message && (/^(?:fetch failed|terminated|socket hang up|The socket connection was closed unexpectedly\.?|OpenAI Responses stream ended before a terminal response event|Codex stream ended without a stop reason|Codex SSE response headers timed out after \d+ms|(?:read |write )?ECONNRESET)$/i.test(message)
    || message.startsWith("Codex error: An error occurred while processing your request. You can retry your request"));
}

const AFTER_PI_RETRY = Symbol("after Pi retry");
type StreamOptions = Parameters<StreamFunction>[2] & { [AFTER_PI_RETRY]?: true };
export function afterPiRetry(options: Parameters<StreamFunction>[2]): StreamOptions {
  return { ...options, [AFTER_PI_RETRY]: true };
}

// Not reasons to assert in error copy: these are conservative vetoes on replay
// when a new upstream code or free-form diagnostic resembles a terminal denial.
const CODEX_REPLAY_VETO = /abort|cancel|sign[ _-]?in|oauth|unauthori[sz]ed|authenticat|forbidden|access[ _-]?denied|invalid[ _-]?(?:token|api[ _-]?key|request)|expired|quota|billing|credit.balance|usage.limit|budget|policy|safety|content.filter|flagged|refus|not.allowed|context.length/i;

type StartEvent = Extract<AssistantMessageEvent, { type: "start" }>;

/** Capture native evidence before Pi sanitizes it; replay only a single, output-free SSE request.
 * Pi still owns its existing recognized-transient retry path when this replay was not used. */
export function withProviderFailures(provider: ProviderId, stream: StreamFunction): StreamFunction {
  return (model, context, options?: StreamOptions) => lazyStream(model, async () => (async function* () {
    const codex = provider === "openai-codex";
    // Auto/WS can already retry or fall back before we see a fetch. The host's
    // agent sessions pin SSE; don't opt other callers into a replay by guessing.
    const eligible = codex && options?.transport === "sse" && !options[AFTER_PI_RETRY];
    const send = options?.fetch ?? globalThis.fetch;
    let replayed = false;
    let payload: unknown;
    for (;;) {
      let fetches = 0;
      let headers: Headers | undefined;
      let http: FailureInput | undefined;
      let streamError: FailureInput | undefined;
      let nativeFailure = false;
      let unsafe = false;
      let start: StartEvent | undefined;
      let output = false;
      let replay = false;
      const fetch: typeof globalThis.fetch = async (input, init) => {
        fetches++;
        headers = http = streamError = undefined;
        const response = await send(input, init);
        headers = response.headers;
        http = response.ok ? undefined : { status: response.status, headers, body: await errorBody(response) };
        return response;
      };
      const source = stream(model, context, { ...options, fetch,
        ...(replayed && { maxRetries: 0 }),
        ...(eligible && options.onPayload && {
          onPayload: async (body: unknown, current: typeof model) => {
            if (!replayed) payload = await options.onPayload!(body, current) ?? body;
            return payload;
          },
        }),
        ...(codex && {
          async onProviderStreamEvent(data: unknown, current: typeof model) {
            const event = record(data);
            const response = record(event.response);
            if (response.output !== undefined && !(Array.isArray(response.output) && response.output.length === 0)) unsafe = true;
            if (event.type === "error" || event.type === "response.failed") {
              const error = record(event.type === "error" ? event.error : response.error);
              const code = event.type === "error" && typeof event.code === "string" ? event.code : undefined;
              nativeFailure = true;
              if ([code, error.code, error.type, event.message, error.message]
                .some(value => typeof value === "string" && CODEX_REPLAY_VETO.test(value))) unsafe = true;
              streamError = {
                ...(code !== undefined ? { sdk_error: code } : {}),
                ...([error.code, error.type].some(value => typeof value === "string")
                  ? { body: { error: { code: error.code, type: error.type, resets_at: error.resets_at } } } : {}),
                request_id: event.request_id ?? response.request_id,
              };
            } else if (event.type !== "response.created" && event.type !== "response.in_progress") {
              // Includes tool activity (even server-side or items Pi doesn't map to content)
              // and any unfamiliar lifecycle events we cannot prove are side-effect free.
              unsafe = true;
            }
            try { return await options?.onProviderStreamEvent?.(data, current); }
            catch (error) { unsafe = true; throw error; }
          },
        }),
      });
      for await (const raw of source) {
        let event = raw;
        if (replayed && raw.type === "done" && raw.reason === "length") {
          // Pi may compact and retry a length stop. This request already spent its
          // replay: retain its contents but surface the truncation as final.
          raw.message.stopReason = "error";
          const message = raw.message as typeof raw.message & FailureMessage;
          message.providerFailure = normalizeFailure(provider, { sdk_error: "max_output_tokens", headers });
          event = { type: "error", reason: "error", error: message };
        }
        if (event.type === "start" && eligible && !output && !start) {
          start = event;
          output = !!event.partial.content.length;
          continue;
        }
        output ||= event.type !== "error" && event.type !== "done" && (event.type !== "start" || !!event.partial.content.length);
        if (event.type === "error" && event.reason === "error") {
          const message = event.error as typeof event.error & FailureMessage;
          const diagnostic = message.errorMessage;
          const hasNativeCode = streamError?.body !== undefined || streamError?.sdk_error !== undefined;
          const failure = message.providerFailure ?? normalizeFailure(provider, {
            ...(!http && !hasNativeCode && codex && codexTransportFailure(diagnostic) && { sdk_error: "server_error" }),
            message: diagnostic, headers, ...streamError, ...http,
          });
          const summary = failureSummary(failure, diagnostic);
          const unclassified = codex && failure.category === "unknown" && summary === failureSummary(failure);
          output ||= !!message.content.length || !!start?.partial.content.length;
          if (eligible && !replayed && unclassified && nativeFailure && fetches === 1 && headers && !http
            && !headers.has("retry-after") && !headers.has("retry-after-ms") && !output && !unsafe && !options.signal?.aborted
            && !(diagnostic && CODEX_REPLAY_VETO.test(diagnostic))) {
            replay = true;
            break;
          }
          message.providerFailure = { ...failure, ...(replayed && { replayed: true }), ...(output && codex && { output_observed: true }) };
          if (unclassified || replayed) {
            const copy = codexFailureCopy({ partial: output, retried: replayed, unknown: failure.category === "unknown", requestRef: failure.request_ref, category: failure.category });
            message.errorMessage = `${copy.sentence} ${copy.step}`;
          } else {
            const restartable = codex && failure.category === "transient"
              && !message.content.length && !options?.signal?.aborted && !failure.retry_after_secs;
            message.errorMessage = restartable ? `Service temporarily unavailable: ${summary}` : summary;
          }
        } else if (event.type === "error" && codex) {
          // A real stop remains a stop; never surface a raw provider diagnostic for it.
          event.error.errorMessage = "Operation aborted";
        }
        if (start) { yield start; start = undefined; }
        yield event;
      }
      if (!replay) {
        if (start) yield start;
        return;
      }
      replayed = true;
    }
  })(), undefined, options?.signal);
}
