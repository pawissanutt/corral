import { openSync, closeSync, readFileSync, writeFileSync, renameSync, unlinkSync, existsSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { zstdDecompressSync } from "node:zlib";
import { ProbeFailure, record } from "./credentials.mjs";

/** T-136 consumed this owner authorization; no later live check may reuse it. */
export const T136_AUTHORIZATION = "2026-10-18T02:05:19.15090102Z";
export const endpoint = "https://chatgpt.com/backend-api/codex/responses";

/**
 * A live check needs its own recorded owner authorization (a canonical comment
 * timestamp). Absence or the consumed T-136 authorization refuses before any request.
 * @param {string | undefined} value
 */
export function liveAuthorization(value) {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T[\d:.]+Z$/.test(value) || value === T136_AUTHORIZATION) {
    throw new ProbeFailure("live_check_not_authorized");
  }
  return value;
}

/** One persistent ledger per authorization, so a new grant never resets an old one.
 * @param {string} dir @param {string} authorization */
export function liveLedger(dir, authorization) {
  return join(dir, "live-budget-" + createHash("sha256").update(authorization).digest("hex").slice(0, 12) + ".json");
}

/** @param {unknown} payload */
export function cappedPayload(payload) {
  if (!record(payload)) throw new ProbeFailure("invalid_provider_payload");
  return { ...payload, max_output_tokens: 1024 };
}

/**
 * Reserve before HTTP dispatch. The non-secret ledger survives reruns, reloads
 * and separate native sessions; never delete/reset it to obtain more calls.
 * @param {string} path
 * @param {string} authorization
 */
function reserve(path, authorization) {
  const lock = openSync(path + ".lock", "wx", 0o600);
  try {
    /** @type {unknown} */
    const data = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) :
      { authorization, requests: [] };
    if (!record(data) || data.authorization !== authorization || !Array.isArray(data.requests)) {
      throw new ProbeFailure("invalid_budget_ledger");
    }
    if (data.requests.length >= 5) throw new ProbeFailure("live_call_budget_exhausted");
    const sequence = data.requests.length + 1;
    const updated = { authorization, requests: [...data.requests, {
      sequence, at: new Date().toISOString(), provider: "openai-codex",
      model: "gpt-6.1-sol", maxOutputTokens: 1024, state: "reserved_before_dispatch",
    }] };
    writeFileSync(path + ".tmp", JSON.stringify(updated, null, 2) + "\n", { mode: 0o600 });
    renameSync(path + ".tmp", path);
    return sequence;
  } finally {
    closeSync(lock);
    unlinkSync(path + ".lock");
  }
}

/**
 * A null ledger is the offline fake transport: it requires an injected `send`,
 * never reserves live budget and never reaches the network.
 * @param {{credential: import("./credentials.mjs").SubscriptionCredential,
 * ledger: string | null, authorization?: string, allowance: number,
 * receipt: (value: Record<string, unknown>) => void,
 * send?: typeof globalThis.fetch}} options
 */
export function guardedFetch(options) {
  if (options.ledger === null && !options.send) throw new ProbeFailure("fake_transport_required");
  if (options.ledger !== null && typeof options.authorization !== "string") {
    throw new ProbeFailure("live_check_not_authorized");
  }
  const send = options.send ?? globalThis.fetch;
  const destination = options.ledger === null ? "fake_transport" : "provider";
  let dispatched = 0;
  /** @type {typeof globalThis.fetch} */
  const guarded = async (input, init) => {
    if (String(input) !== endpoint || init?.method !== "POST") {
      throw new ProbeFailure("provider_destination_forbidden");
    }
    const headers = new Headers(init.headers);
    if (headers.get("authorization") !== "Bearer " + options.credential.access ||
        headers.get("chatgpt-account-id") !== options.credential.accountId) {
      throw new ProbeFailure("subscription_credential_not_reused");
    }
    let bytes;
    if (typeof init.body === "string") bytes = Buffer.from(init.body);
    else if (init.body instanceof Uint8Array) bytes = Buffer.from(init.body);
    else throw new ProbeFailure("unchecked_provider_body");
    if (bytes.length > 65_536) throw new ProbeFailure("provider_body_too_large");
    const encoding = headers.get("content-encoding");
    if (encoding === "zstd") bytes = zstdDecompressSync(bytes, { maxOutputLength: 65_536 });
    else if (encoding !== null) throw new ProbeFailure("unchecked_provider_encoding");
    /** @type {unknown} */
    const body = JSON.parse(bytes.toString("utf8"));
    if (!record(body) || body.model !== "gpt-6.1-sol" || body.store !== false ||
        body.stream !== true || !Number.isInteger(body.max_output_tokens) ||
        typeof body.max_output_tokens !== "number" || body.max_output_tokens < 1 ||
        body.max_output_tokens > 1024) {
      throw new ProbeFailure("provider_route_or_output_cap_forbidden");
    }
    if (dispatched >= options.allowance) throw new ProbeFailure("phase_call_budget_exhausted");
    const sequence = options.ledger === null ? dispatched + 1 :
      reserve(options.ledger, /** @type {string} */ (options.authorization));
    dispatched++;
    options.receipt({
      type: "request_reserved", sequence, provider: "openai-codex", model: "gpt-6.1-sol",
      maxOutputTokens: body.max_output_tokens, accessTokenMatched: true, destination,
      accountHeaderMatched: true, transport: "sse", encoding: encoding ?? "identity",
    });
    const response = await send(input, { ...init, redirect: "error" });
    let outputCapRejected = false;
    if (!response.ok) {
      /** @type {unknown} */
      const errorBody = await response.clone().json().catch(() => null);
      if (record(errorBody)) {
        const error = record(errorBody.error) ? errorBody.error : errorBody;
        const message = typeof error.message === "string" ? error.message : error.detail;
        outputCapRejected = typeof message === "string" && message.includes("max_output_tokens") &&
          /unsupported|not supported|unrecognized/i.test(message);
      }
    }
    options.receipt({ type: "provider_http", sequence, status: response.status, outputCapRejected });
    return response;
  };
  return guarded;
}
