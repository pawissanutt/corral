import { openSync, closeSync, readFileSync, writeFileSync, renameSync, unlinkSync, existsSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { ProbeFailure, record } from "../credentials.mjs";
import { ANTHROPIC_ORIGIN, BUDGET, MAX_TOKENS_PER_REQUEST, MESSAGES_PATH, OAUTH_TOKEN_MARKER } from "./policy.mjs";

/**
 * A live run needs its own recorded owner authorization (a canonical comment timestamp).
 * @param {string | undefined} value
 */
export function liveAuthorization(value) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T[\d:.]+Z$/.test(value)) {
    throw new ProbeFailure("live_check_not_authorized");
  }
  return value;
}

/** One persistent ledger per authorization, so rerunning never resets the budget.
 * @param {string} dir @param {string} authorization */
export function ledgerPath(dir, authorization) {
  return join(dir, "claude-live-budget-" + createHash("sha256").update(authorization).digest("hex").slice(0, 12) + ".json");
}

/** @typedef {{sequence: number, at: string, model: string, maxTokens: number, state: string, outputTokens?: number}} LedgerEntry */

/** @param {string} path @param {string} authorization @returns {{authorization: string, requests: LedgerEntry[]}} */
function load(path, authorization) {
  if (!existsSync(path)) return { authorization, requests: [] };
  /** @type {unknown} */
  const data = JSON.parse(readFileSync(path, "utf8"));
  if (!record(data) || data.authorization !== authorization || !Array.isArray(data.requests)) {
    throw new ProbeFailure("invalid_budget_ledger");
  }
  return /** @type {{authorization: string, requests: LedgerEntry[]}} */ (data);
}

/** Output tokens already spent or still reserved by earlier entries.
 * @param {LedgerEntry[]} requests */
export function consumedTokens(requests) {
  return requests.reduce((sum, entry) => sum + (entry.outputTokens ?? entry.maxTokens), 0);
}

/** Serialized read-modify-write; a crashed holder leaves a lock that must be cleared by the owner.
 * @template T
 * @param {string} path @param {string} authorization
 * @param {(ledger: {authorization: string, requests: LedgerEntry[]}) => T} update */
function transact(path, authorization, update) {
  /** @type {number} */
  let lock;
  try {
    lock = openSync(path + ".lock", "wx", 0o600);
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "EEXIST") throw new ProbeFailure("ledger_locked");
    throw error;
  }
  try {
    const ledger = load(path, authorization);
    const result = update(ledger);
    writeFileSync(path + ".tmp", JSON.stringify(ledger, null, 2) + "\n", { mode: 0o600 });
    renameSync(path + ".tmp", path);
    return result;
  } finally {
    closeSync(lock);
    unlinkSync(path + ".lock");
  }
}

/**
 * Reserve before dispatch: request count and output tokens (the request's max_tokens counts as spent
 * until settled), so the budget holds even when a process dies mid-stream.
 * @param {string} path @param {string} authorization @param {string} model @param {number} maxTokens
 */
export function reserve(path, authorization, model, maxTokens) {
  return transact(path, authorization, (ledger) => {
    if (ledger.requests.length >= BUDGET.requests) throw new ProbeFailure("live_call_budget_exhausted");
    if (consumedTokens(ledger.requests) + maxTokens > BUDGET.outputTokens) {
      throw new ProbeFailure("live_output_budget_exhausted");
    }
    const sequence = ledger.requests.length + 1;
    ledger.requests.push({ sequence, at: new Date().toISOString(), model, maxTokens, state: "reserved_before_dispatch" });
    return sequence;
  });
}

/** Replace a request's reservation with the output tokens the provider reported.
 * @param {string} path @param {string} authorization @param {number} sequence @param {number} outputTokens */
export function settle(path, authorization, sequence, outputTokens) {
  transact(path, authorization, (ledger) => {
    const entry = ledger.requests.find((candidate) => candidate.sequence === sequence);
    if (!entry) throw new ProbeFailure("invalid_budget_ledger");
    entry.state = "settled";
    entry.outputTokens = Math.min(outputTokens, entry.maxTokens);
  });
}

/** @param {string} path @param {string} authorization */
export function readLedger(path, authorization) {
  return load(path, authorization);
}

/** Replace any key-shaped text before a provider string is stored as evidence.
 * @param {string} text */
export function redact(text) {
  return text.replace(/sk-ant-[A-Za-z0-9_-]+/g, "[redacted]").slice(0, 300);
}

/** Non-secret response headers kept as billing evidence.
 * @param {Headers} headers @returns {Record<string, string>} */
export function evidenceHeaders(headers) {
  /** @type {Record<string, string>} */
  const kept = {};
  for (const [name, value] of headers.entries()) {
    if (name.startsWith("anthropic-ratelimit-") || name === "request-id" || name === "retry-after" ||
        name === "x-should-retry") kept[name] = redact(value);
  }
  return kept;
}

/**
 * The only path to Anthropic. It validates every request before dispatch: the messages endpoint on
 * the Anthropic origin, an OAuth bearer (never an API key header), a capped streamed body, and the
 * budget. A null ledger is the offline fake transport: it requires an injected `send` and never
 * reaches the network.
 * @param {{access: string, model: string, ledger: string | null, authorization?: string, allowance: number,
 * receipt: (value: Record<string, unknown>) => void, send?: typeof globalThis.fetch}} options
 */
export function guardedFetch(options) {
  if (options.ledger === null && !options.send) throw new ProbeFailure("fake_transport_required");
  if (options.ledger !== null && typeof options.authorization !== "string") {
    throw new ProbeFailure("live_check_not_authorized");
  }
  if (!options.access.includes(OAUTH_TOKEN_MARKER)) throw new ProbeFailure("subscription_credential_required");
  const send = options.send ?? globalThis.fetch;
  const destination = options.ledger === null ? "fake_transport" : "provider";
  let dispatched = 0;
  /** @type {typeof globalThis.fetch} */
  const guarded = async (input, init) => {
    const url = new URL(String(input));
    if (url.origin !== ANTHROPIC_ORIGIN || url.pathname !== MESSAGES_PATH || init?.method !== "POST") {
      throw new ProbeFailure("provider_destination_forbidden");
    }
    const headers = new Headers(init.headers);
    if (headers.get("authorization") !== "Bearer " + options.access) throw new ProbeFailure("subscription_credential_not_reused");
    if (headers.has("x-api-key")) throw new ProbeFailure("api_key_header_forbidden");
    if (typeof init.body !== "string" || init.body.length > 262_144) throw new ProbeFailure("unchecked_provider_body");
    if (headers.get("content-encoding") !== null) throw new ProbeFailure("unchecked_provider_encoding");
    /** @type {unknown} */
    const body = JSON.parse(init.body);
    if (!record(body) || body.model !== options.model || body.stream !== true ||
        typeof body.max_tokens !== "number" || !Number.isInteger(body.max_tokens) ||
        body.max_tokens < 1 || body.max_tokens > MAX_TOKENS_PER_REQUEST) {
      throw new ProbeFailure("provider_route_or_output_cap_forbidden");
    }
    if (dispatched >= options.allowance) throw new ProbeFailure("phase_call_budget_exhausted");
    const sequence = options.ledger === null ? dispatched + 1 :
      reserve(options.ledger, /** @type {string} */ (options.authorization), options.model, body.max_tokens);
    dispatched++;
    options.receipt({ type: "request_reserved", sequence, provider: "anthropic", model: options.model,
      maxTokens: body.max_tokens, bearerMatched: true, apiKeyHeader: false, destination });
    const response = await send(input, { ...init, redirect: "error" });
    /** @type {Record<string, unknown>} */
    const observed = { type: "provider_http", sequence, status: response.status, headers: evidenceHeaders(response.headers) };
    if (!response.ok) {
      /** @type {unknown} */
      const errorBody = await response.clone().json().catch(() => null);
      const error = record(errorBody) && record(errorBody.error) ? errorBody.error : null;
      if (error) {
        observed.error = { type: typeof error.type === "string" ? redact(error.type) : null,
          message: typeof error.message === "string" ? redact(error.message) : null };
      }
    }
    options.receipt(observed);
    return response;
  };
  return guarded;
}
