import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fakeCodexTransport, writeSyntheticAuth } from "./fake-transport.mjs";
import { readSubscription, readOnlyStore } from "./credentials.mjs";
import { guardedFetch, cappedPayload } from "./request-guard.mjs";

/** Streams one prompt through the pinned Codex adapter, whose only destination is the fake.
 * @param {ReturnType<typeof fakeCodexTransport>} transport */
async function codex(transport) {
  const dir = mkdtempSync(join(tmpdir(), "pi-fake-transport-test-"));
  const path = join(dir, "auth.json");
  writeSyntheticAuth(path, Date.now());
  const { credential } = readSubscription(path, "codex", Date.now());
  rmSync(dir, { recursive: true, force: true });
  const sdk = await import("@earendil-works/pi-coding-agent");
  const models = await sdk.ModelRuntime.create({ credentials: readOnlyStore(credential), modelsPath: null,
    refreshOnCreate: false, allowModelNetwork: false });
  const model = models.getModel("openai-codex", "gpt-6.1-sol");
  assert.ok(model);
  const fetch = guardedFetch({ credential, ledger: null, allowance: 4, receipt() {}, send: transport.send });
  /** @param {string} text @param {AbortSignal} [signal] */
  return (text, signal) => models.streamSimple(model, { messages: [{ role: "user", content: text, timestamp: Date.now() }] },
    { maxTokens: 1024, transport: "sse", maxRetries: 0, signal, cacheRetention: "none", onPayload: cappedPayload, fetch });
}

test("the pinned adapter parses scripted SSE and maps every usage category", async () => {
  // Given one scripted response with cached, cache-write and reasoning tokens.
  const transport = fakeCodexTransport({ receipt() {},
    script: [{ text: "SCRIPTED", usage: { input: 10, cached: 4, cacheWrite: 2, output: 5, reasoning: 1 } }] });
  // When Pi streams a prompt through the fake.
  const result = await (await codex(transport))("Reply only SCRIPTED.").result();
  // Then Pi reports the scripted text, response ID and Pi-mapped usage.
  assert.equal(result.stopReason, "stop");
  assert.deepEqual(result.content.map((part) => part.type === "text" ? part.text : part.type), ["SCRIPTED"]);
  assert.equal(result.responseId, "resp_fake_1");
  assert.deepEqual([result.usage.input, result.usage.cacheRead, result.usage.cacheWrite, result.usage.output,
    result.usage.totalTokens], [10, 4, 2, 5, 21]);
});

test("a held response stays open until released, then completes", async () => {
  // Given the next response held open after it starts.
  /** @type {() => void} */
  let held = () => {};
  const opened = new Promise((resolve) => { held = () => resolve(undefined); });
  const transport = fakeCodexTransport({ receipt() {}, onHold: () => held() });
  transport.holdNext();
  const stream = (await codex(transport))("Reply only RELEASED.");
  // When the hold is observed and then released.
  await opened;
  transport.release();
  // Then the echo completes and nothing was aborted.
  const result = await stream.result();
  assert.equal(result.stopReason, "stop");
  assert.equal(transport.aborted, 0);
});

test("aborting a held response cancels the open stream as aborted", async () => {
  // Given a response held open mid-stream.
  /** @type {() => void} */
  let held = () => {};
  const opened = new Promise((resolve) => { held = () => resolve(undefined); });
  const transport = fakeCodexTransport({ receipt() {}, onHold: () => held() });
  transport.holdNext();
  const controller = new AbortController();
  const stream = (await codex(transport))("Reply only CANCELLED.", controller.signal);
  // When the caller aborts while the fake holds the stream.
  await opened;
  controller.abort();
  // Then Pi reports aborted and the fake observed the cancellation.
  assert.equal((await stream.result()).stopReason, "aborted");
  assert.equal(transport.aborted, 1);
  assert.throws(() => transport.release(), { code: "no_held_provider" });
});

test("an injected HTTP 503 ends as an error without a retry", async () => {
  // Given the next request fails at the transport.
  const transport = fakeCodexTransport({ receipt() {} });
  transport.failNext();
  // When Pi streams with zero retries.
  const result = await (await codex(transport))("Reply only FAILED.").result();
  // Then the turn is an error and exactly one request was made.
  assert.equal(result.stopReason, "error");
  assert.equal(transport.requests, 1);
  assert.equal(transport.failures, 1);
});
