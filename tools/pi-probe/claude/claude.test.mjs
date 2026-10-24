import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { classifyRoute, billingOf } from "./classify.mjs";
import { fakeAnthropic, SUBSCRIPTION_HEADERS } from "./fake-anthropic.mjs";
import { consumedTokens, guardedFetch, ledgerPath, liveAuthorization, readLedger, reserve, settle } from "./guard.mjs";
import { runProbe } from "./probe.mjs";
import { openCredentialStore } from "./store.mjs";

const probeRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ACCESS = "sk-ant-oat01-synthetic-test-token";
const REFRESH = "synthetic-refresh-test-token";
const AUTH = "2026-10-24T03:11:01.406036577Z";

/** @template T @param {(dir: string) => Promise<T> | T} body */
async function scratch(body) {
  const dir = mkdtempSync(join(tmpdir(), "claude-probe-test-"));
  try { return await body(dir); } finally { rmSync(dir, { recursive: true, force: true }); }
}

/** @param {object} [overrides] */
function request(overrides = {}) {
  return ["https://api.anthropic.com/v1/messages", { method: "POST", headers: { authorization: "Bearer " + ACCESS },
    body: JSON.stringify({ model: "claude-haiku-5-5", stream: true, max_tokens: 128, ...overrides }) }];
}

/** @param {Partial<Parameters<typeof guardedFetch>[0]>} [extra] */
function guard(extra = {}) {
  const fake = fakeAnthropic({ script: [{}, {}, {}] });
  return { fake, receipts: /** @type {Record<string, unknown>[]} */ ([]),
    fetch: guardedFetch({ access: ACCESS, model: "claude-haiku-5-5", ledger: null, allowance: 2, receipt() {}, send: fake.send, ...extra }) };
}

test("the guard refuses every request that could leave the subscription route", async () => {
  // Given a guard on the offline transport.
  const { fetch, fake } = guard();
  const [url, init] = /** @type {[string, RequestInit]} */ (request());
  // When a request names another host, path, key header, token, cap or stream mode, then it is refused before sending.
  await assert.rejects(fetch("https://example.com/v1/messages", init), { code: "provider_destination_forbidden" });
  await assert.rejects(fetch("https://api.anthropic.com/v1/complete", init), { code: "provider_destination_forbidden" });
  await assert.rejects(fetch(url, { ...init, headers: { authorization: "Bearer " + ACCESS, "x-api-key": "sk-ant-api03-x" } }),
    { code: "api_key_header_forbidden" });
  await assert.rejects(fetch(url, { ...init, headers: { authorization: "Bearer other" } }), { code: "subscription_credential_not_reused" });
  await assert.rejects(fetch(...(/** @type {[string, RequestInit]} */ (request({ max_tokens: 257 })))), { code: "provider_route_or_output_cap_forbidden" });
  await assert.rejects(fetch(...(/** @type {[string, RequestInit]} */ (request({ stream: false })))), { code: "provider_route_or_output_cap_forbidden" });
  await assert.rejects(fetch(...(/** @type {[string, RequestInit]} */ (request({ model: "claude-opus-5" })))), { code: "provider_route_or_output_cap_forbidden" });
  assert.equal(fake.requests.length, 0);
});

test("the guard refuses an API key credential and a missing fake transport", () => {
  assert.throws(() => guardedFetch({ access: "sk-ant-api03-key", model: "m", ledger: null, allowance: 1, receipt() {}, send: globalThis.fetch }),
    { code: "subscription_credential_required" });
  assert.throws(() => guardedFetch({ access: ACCESS, model: "m", ledger: null, allowance: 1, receipt() {} }), { code: "fake_transport_required" });
  assert.throws(() => guardedFetch({ access: ACCESS, model: "m", ledger: "/tmp/x", allowance: 1, receipt() {} }), { code: "live_check_not_authorized" });
  assert.throws(() => liveAuthorization(undefined), { code: "live_check_not_authorized" });
});

test("the per-process allowance stops a third request", async () => {
  const { fetch, fake } = guard();
  const [url, init] = /** @type {[string, RequestInit]} */ (request());
  await fetch(url, init);
  await fetch(url, init);
  await assert.rejects(fetch(url, init), { code: "phase_call_budget_exhausted" });
  assert.equal(fake.requests.length, 2);
});

test("the ledger enforces 20 requests and 50K output tokens across reruns and survives a lost process", async () => {
  await scratch((dir) => {
    // Given a ledger that reserves 256 tokens per request.
    const path = ledgerPath(dir, AUTH);
    for (let index = 1; index <= 20; index++) assert.equal(reserve(path, AUTH, "m", 256), index);
    // Then the 21st request is refused, with reservations counting as spent until settled.
    assert.throws(() => reserve(path, AUTH, "m", 1), { code: "live_call_budget_exhausted" });
    assert.equal(consumedTokens(readLedger(path, AUTH).requests), 20 * 256);
    // And a different authorization has its own ledger path but a wrong-owner file is rejected.
    assert.notEqual(ledgerPath(dir, "2026-10-25T00:00:00Z"), path);
    assert.throws(() => readLedger(path, "2026-10-25T00:00:00Z"), { code: "invalid_budget_ledger" });
  });
  await scratch((dir) => {
    // Given a ledger near the token ceiling.
    const path = ledgerPath(dir, AUTH);
    writeFileSync(path, JSON.stringify({ authorization: AUTH, requests: [{ sequence: 1, at: "t", model: "m", maxTokens: 49_900, state: "reserved_before_dispatch" }] }));
    // Then another 256-token reservation exceeds 50K, and settling the first frees its unused reservation.
    assert.throws(() => reserve(path, AUTH, "m", 256), { code: "live_output_budget_exhausted" });
    settle(path, AUTH, 1, 10);
    assert.equal(reserve(path, AUTH, "m", 256), 2);
  });
});

test("a leftover lock fails closed instead of bypassing the ledger", async () => {
  await scratch((dir) => {
    const path = ledgerPath(dir, AUTH);
    writeFileSync(path + ".lock", "");
    assert.throws(() => reserve(path, AUTH, "m", 1), { code: "ledger_locked" });
  });
});

/** @param {Record<string, string>} headers @param {number} [status] */
const observed = (headers, status = 200) => ({ sequence: 1, status, headers, completed: status === 200, toolCall: true });

test("billing is read from the unified rate-limit claim and defaults to no evidence", () => {
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS })), "subscription");
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-representative-claim": "seven_day_sonnet" })), "subscription");
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-representative-claim": "overage" })), "extra_usage");
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-overage-status": "in_use" })), "extra_usage");
  // The separate paid-overage header vetoes a subscription claim, and works without any quota headers.
  const inUse = { ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-overage-status": "allowed", "anthropic-ratelimit-unified-overage-in-use": "true" };
  assert.equal(billingOf(observed(inUse)), "extra_usage");
  assert.equal(billingOf(observed({ "anthropic-ratelimit-unified-overage-in-use": "true" })), "extra_usage");
  assert.equal(billingOf(observed({ "anthropic-ratelimit-unified-overage-in-use": "TRUE" })), "extra_usage");
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-overage-in-use": "false" })), "subscription");
  assert.equal(billingOf(observed({})), "unknown");
  assert.equal(billingOf(observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-representative-claim": "mystery" })), "unknown");
});

test("GO needs subscription evidence on every request and a tool round trip", () => {
  const ok = observed({ ...SUBSCRIPTION_HEADERS });
  assert.deepEqual(pick(classifyRoute([ok, { ...ok, sequence: 2 }])), ["GO", "subscription", "subscription_coverage_evidenced"]);
  assert.deepEqual(pick(classifyRoute([ok, { ...ok, sequence: 2, headers: {} }])), ["NO-GO", "unknown", "no_subscription_evidence"]);
  const plainOnly = { ...ok, toolCall: false };
  assert.deepEqual(pick(classifyRoute([plainOnly, { ...plainOnly, sequence: 2 }])), ["NO-GO", "subscription", "tool_round_trip_not_proven"]);
  const extra = observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-representative-claim": "overage" });
  assert.deepEqual(pick(classifyRoute([extra, { ...extra, sequence: 2 }])), ["NO-GO", "extra_usage", "extra_usage_billing"]);
  const paid = observed({ ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-overage-status": "allowed", "anthropic-ratelimit-unified-overage-in-use": "true" });
  const paidOnly = classifyRoute([ok, { ...paid, sequence: 2 }]);
  assert.deepEqual(pick(paidOnly), ["NO-GO", "extra_usage", "extra_usage_billing"]);
  assert.equal(paidOnly.evidence[1]?.overageInUse, "true");
  assert.equal(paidOnly.evidence[0]?.overageInUse, null);
  assert.deepEqual(pick(classifyRoute([observed({ "anthropic-ratelimit-unified-overage-in-use": "true" })])), ["NO-GO", "extra_usage", "extra_usage_billing"]);
  const refused = { sequence: 1, status: 403, headers: {}, completed: false, error: { type: "permission_error", message: "OAuth is not allowed" } };
  assert.deepEqual(pick(classifyRoute([refused])), ["NO-GO", "blocked", "subscription_token_rejected"]);
  const billingText = { ...refused, status: 400, error: { type: "invalid_request_error", message: "Third-party apps now draw from extra usage" } };
  assert.deepEqual(pick(classifyRoute([billingText])), ["NO-GO", "blocked", "extra_usage_or_billing_required"]);
});

test("the two captured live responses still classify as subscription coverage", () => {
  // Given the unified headers both live requests returned (extra usage disabled at the org level).
  const captured = {
    "anthropic-ratelimit-unified-5h-status": "allowed", "anthropic-ratelimit-unified-5h-utilization": "0.06",
    "anthropic-ratelimit-unified-7d-status": "allowed", "anthropic-ratelimit-unified-7d-utilization": "0.6",
    "anthropic-ratelimit-unified-overage-disabled-reason": "org_level_disabled",
    "anthropic-ratelimit-unified-overage-status": "rejected",
    "anthropic-ratelimit-unified-representative-claim": "five_hour", "anthropic-ratelimit-unified-status": "allowed",
  };
  // When they are classified, then no paid-usage signal is present and the decision is unchanged.
  const result = classifyRoute([observed(captured), { ...observed(captured), sequence: 2 }]);
  assert.deepEqual(pick(result), ["GO", "subscription", "subscription_coverage_evidenced"]);
  assert.deepEqual(result.evidence.map((entry) => entry.overageInUse), [null, null]);
});

/** @param {ReturnType<typeof classifyRoute>} value */
const pick = (value) => [value.decision, value.billing, value.reason];

test("credential store keeps a 0600 file in a 0700 directory and refuses foreign stores", async () => {
  await scratch(async (dir) => {
    const store = openCredentialStore(join(dir, "auth"));
    await store.modify("anthropic", async () => ({ type: "oauth", access: ACCESS, refresh: REFRESH, expires: Date.now() + 1000 }));
    assert.equal(statSync(join(dir, "auth")).mode & 0o777, 0o700);
    assert.equal(statSync(store.path).mode & 0o777, 0o600);
    assert.equal((await store.read("anthropic"))?.type, "oauth");
    assert.deepEqual(await store.list(), [{ providerId: "anthropic", type: "oauth" }]);
    await store.delete("anthropic");
    assert.equal(await store.read("anthropic"), undefined);
    for (const foreign of [".claude", ".codex", ".omo", ".pi"]) {
      assert.throws(() => openCredentialStore(join(dir, foreign, "agent")), { code: "credential_dir_foreign_store" });
    }
    assert.throws(() => openCredentialStore("relative/dir"), { code: "credential_dir_not_absolute" });
    // A group-readable credential file is refused rather than trusted.
    writeFileSync(store.path, "{}");
    chmodSync(store.path, 0o644);
    await assert.rejects(store.read("anthropic"), { code: "credential_file_not_owner_private_bounded" });
    rmSync(store.path);
    symlinkSync(join(dir, "elsewhere"), store.path);
    await assert.rejects(store.read("anthropic"), { code: "credential_file_not_owner_private_bounded" });
  });
});

/** Runs the whole probe through the pinned Pi under Bun against the fake transport. */
async function offlineRun(/** @type {import("./fake-anthropic.mjs").Step[]} */ script) {
  return scratch(async (dir) => {
    const store = openCredentialStore(join(dir, "auth"));
    await store.modify("anthropic", async () => ({ type: "oauth", access: ACCESS, refresh: REFRESH, expires: Date.now() + 3_600_000 }));
    const fake = fakeAnthropic({ script });
    /** @type {Record<string, unknown>[]} */
    const receipts = [];
    const outputDir = join(dir, "out");
    /** @type {Awaited<ReturnType<typeof runProbe>> | undefined} */
    let value;
    /** @type {unknown} */
    let error;
    try {
      value = await runProbe({ mode: "offline", store, send: fake.send, ledger: null, outputDir, probeRoot,
        receipt: (entry) => receipts.push(entry) });
    } catch (caught) { error = caught; }
    const files = existsSync(join(dir, "out")) ? readFileSync(join(outputDir, "claude-offline-result.json"), "utf8") : "";
    return { value, error, fake, receipts, files, auth: readFileSync(store.path, "utf8") };
  });
}

test("offline: the pinned Pi streams a plain reply and a tool call through the OAuth route", async () => {
  // When the probe runs against the fake transport.
  const run = await offlineRun([{ text: "PROBE_READY" }, { tool: { input: { text: "PROBE_TOOL" } }, outputTokens: 12 }]);
  assert.ok(run.value, JSON.stringify(run.error));
  // Then the offline run is never reported as a live GO, and both requests completed with subscription evidence.
  assert.equal(run.value.decision, "NOT RUN");
  assert.equal(run.value.offlineSelfCheck, "GO");
  assert.equal(run.value.liveEvidence, false);
  assert.equal(run.value.budget.requestsDispatched, 2);
  assert.equal(run.value.budget.outputTokensReported, 17);
  assert.equal(run.value.toolRoundTrip, true);
  // And Pi sent the OAuth identity: bearer token, tools, and a capped streamed body, with no API key header.
  const [plain, tool] = run.fake.requests;
  assert.equal(plain?.stream, true);
  assert.ok(Number(plain?.max_tokens) <= 256);
  assert.ok(Array.isArray(tool?.tools) && tool.tools.some((entry) => entry.name === "probe_echo"));
  // And no credential material reaches the receipts or the result file.
  const everything = JSON.stringify(run.receipts) + run.files;
  assert.ok(!everything.includes(ACCESS) && !everything.includes(REFRESH) && !everything.includes("Bearer"));
  assert.ok(run.auth.includes(ACCESS));
});

test("offline: an extra-usage claim or a billing refusal yields NO-GO evidence, never GO", async () => {
  const overage = { ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-representative-claim": "overage" };
  const extra = await offlineRun([{ headers: overage }, { headers: overage, tool: { input: { text: "x" } } }]);
  assert.equal(extra.value?.offlineSelfCheck, "NO-GO");
  assert.equal(extra.value?.billing, "extra_usage");
  const paidHeaders = { ...SUBSCRIPTION_HEADERS, "anthropic-ratelimit-unified-overage-status": "allowed", "anthropic-ratelimit-unified-overage-in-use": "true" };
  const paid = await offlineRun([{}, { headers: paidHeaders, tool: { input: { text: "x" } } }]);
  assert.equal(paid.value?.offlineSelfCheck, "NO-GO");
  assert.equal(paid.value?.billing, "extra_usage");
  assert.equal(paid.value?.reason, "extra_usage_billing");
  assert.equal(paid.value?.evidence[1]?.overageInUse, "true");
  const bare = await offlineRun([{ headers: { "anthropic-ratelimit-unified-overage-in-use": "true" } },
    { headers: { "anthropic-ratelimit-unified-overage-in-use": "true" }, tool: { input: { text: "x" } } }]);
  assert.equal(bare.value?.offlineSelfCheck, "NO-GO");
  assert.equal(bare.value?.billing, "extra_usage");
  const blocked = await offlineRun([{ status: 400, error: { type: "invalid_request_error", message: "Third-party apps now draw from extra usage" } },
    { status: 400, error: { type: "invalid_request_error", message: "Third-party apps now draw from extra usage" } }]);
  assert.equal(blocked.value?.offlineSelfCheck, "NO-GO");
  assert.equal(blocked.value?.reason, "extra_usage_or_billing_required");
});

test("offline: a missing login stops with a stable code before any request", async () => {
  await scratch(async (dir) => {
    const store = openCredentialStore(join(dir, "auth"));
    const fake = fakeAnthropic({ script: [{}] });
    await assert.rejects(runProbe({ mode: "offline", store, send: fake.send, ledger: null, outputDir: dir, probeRoot, receipt() {} }),
      { code: "login_required" });
    assert.equal(fake.requests.length, 0);
  });
});
