import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zstdCompressSync } from "node:zlib";
import { cappedPayload, guardedFetch, liveAuthorization, liveLedger, T136_AUTHORIZATION } from "./request-guard.mjs";

const endpoint = "https://chatgpt.com/backend-api/codex/responses";
const authorization = "2026-10-23T00:00:00.000000000Z";
const credential = { type: /** @type {const} */ ("oauth"), access: "fixture-access", refresh: "", expires: 0, accountId: "fixture-account" };
const body = { model: "gpt-6.1-sol", store: false, stream: true };
const headers = { authorization: "Bearer fixture-access", "chatgpt-account-id": "fixture-account" };

test("rejects uncapped wire output without dispatching or reserving a call", async () => {
  // Given a provider body that silently omits the requested output cap.
  const dir = mkdtempSync(join(tmpdir(), "pi-budget-test-"));
  let sends = 0;
  const guarded = guardedFetch({ credential, ledger: join(dir, "budget"), authorization, allowance: 1,
    receipt() {}, send: async () => { sends++; return new Response(); } });
  try {
    // When the actual HTTP request reaches the guard.
    await assert.rejects(guarded(endpoint, { method: "POST", headers, body: JSON.stringify(body) }), {
      code: "provider_route_or_output_cap_forbidden",
    });
    // Then nothing reaches the provider.
    assert.equal(sends, 0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("checks the real compressed body and forces the cap after payload overrides", async () => {
  // Given the actual zstd encoding used by the pinned Codex adapter.
  const dir = mkdtempSync(join(tmpdir(), "pi-budget-test-"));
  let sends = 0;
  const guarded = guardedFetch({ credential, ledger: join(dir, "budget"), authorization, allowance: 1,
    receipt() {}, send: async () => { sends++; return new Response(); } });
  try {
    // When an oversized caller cap is replaced and the encoded request sent.
    await guarded(endpoint, { method: "POST", headers: { ...headers, "content-encoding": "zstd" },
      body: zstdCompressSync(JSON.stringify(cappedPayload({ ...body, max_output_tokens: 9999 }))) });
    // Then the wire-validated request is admitted exactly once.
    assert.equal(sends, 1);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("keeps the five-request allowance across independent guard instances", async () => {
  // Given a shared durable ledger and independently recreated session guards.
  const dir = mkdtempSync(join(tmpdir(), "pi-budget-test-"));
  const ledger = join(dir, "budget");
  let sends = 0;
  const options = { credential, ledger, authorization, allowance: 1, receipt() {},
    send: async () => { sends++; return new Response(); } };
  const init = { method: "POST", headers, body: JSON.stringify(cappedPayload(body)) };
  try {
    // When six independent sessions attempt one request each.
    for (let i = 0; i < 5; i++) await guardedFetch(options)(endpoint, init);
    await assert.rejects(guardedFetch(options)(endpoint, init), { code: "live_call_budget_exhausted" });
    // Then only five can reach HTTP, and their reservations survive.
    assert.equal(sends, 5);
    assert.equal(JSON.parse(readFileSync(ledger, "utf8")).requests.length, 5);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("rejects paid destinations before touching the provider", async () => {
  // Given a valid subscription body but a paid API destination.
  const guarded = guardedFetch({ credential, ledger: "/unused", authorization, allowance: 1, receipt() {},
    send: async () => { throw new Error("must not send"); } });
  // When the request is retargeted, then it is denied before dispatch.
  await assert.rejects(guarded("https://api.openai.com/v1/responses", {
    method: "POST", headers, body: JSON.stringify(cappedPayload(body)),
  }), { code: "provider_destination_forbidden" });
});

test("a live check needs a new owner authorization and its own ledger", () => {
  // Given no grant, a malformed grant and the allowance T-136 already consumed.
  for (const value of [undefined, "", "approve", T136_AUTHORIZATION]) {
    // When a live driver resolves its authorization, then it refuses before any request.
    assert.throws(() => liveAuthorization(value), { code: "live_check_not_authorized" });
  }
  assert.equal(liveAuthorization(authorization), authorization);
  // A later grant never shares, and so never resets, an earlier grant's ledger.
  assert.notEqual(liveLedger("/x", authorization), liveLedger("/x", T136_AUTHORIZATION));
  assert.throws(() => guardedFetch({ credential, ledger: "/unused", allowance: 1, receipt() {} }),
    { code: "live_check_not_authorized" });
});

test("the offline fake transport never reserves live budget or reaches the network", async () => {
  // Given no ledger: the offline mode, whose only destination is the injected fake.
  assert.throws(() => guardedFetch({ credential, ledger: null, allowance: 1, receipt() {} }),
    { code: "fake_transport_required" });
  /** @type {Record<string, unknown>[]} */
  const receipts = [];
  let sends = 0;
  const guarded = guardedFetch({ credential, ledger: null, allowance: 2,
    receipt(value) { receipts.push(value); }, send: async () => { sends++; return new Response(); } });
  const init = { method: "POST", headers, body: JSON.stringify(cappedPayload(body)) };
  // When the fake allowance is used up.
  await guarded(endpoint, init);
  await guarded(endpoint, init);
  await assert.rejects(guarded(endpoint, init), { code: "phase_call_budget_exhausted" });
  // Then only the fake was called and each receipt names it as the destination.
  assert.equal(sends, 2);
  assert.deepEqual(receipts.filter((item) => item.type === "request_reserved").map((item) => item.destination),
    ["fake_transport", "fake_transport"]);
});
