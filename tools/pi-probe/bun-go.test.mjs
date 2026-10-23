import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { goRecord } from "./go-record.mjs";
import { CRITERIA } from "./native-scenarios.mjs";
import { readBunLock, readPins } from "./runtime.mjs";

const probeRoot = dirname(fileURLToPath(import.meta.url));
const pins = readPins(probeRoot);
const lock = readBunLock(join(probeRoot, "bun.lock"));
/** @param {string} name */
const json = (name) => JSON.parse(readFileSync(join(probeRoot, name), "utf8"));

test("bun-go.json binds the reviewed lock, the exact Pi pins and six offline PASS results", () => {
  // Given the committed record, pins and lock.
  const record = json("bun-go.json");
  // Then a lock or pin change without a fresh re-proof fails here.
  assert.equal(record.verdict, "GO");
  assert.equal(record.lock.sha256, lock.sha256);
  assert.equal(pins.bunLockSha256, lock.sha256);
  assert.equal(json("lock-diff.json").bun.sha256, lock.sha256);
  assert.deepEqual([record.pi.package, record.pi.version, record.pi.integrity, record.pi.upstreamCommit],
    [pins.package, pins.version, pins.integrity, pins.upstreamCommit]);
  assert.equal(record.bun.range, pins.bunRange);
  assert.ok(record.bun.tested.includes(pins.bun));
  assert.deepEqual(Object.keys(record.results), [...CRITERIA]);
  assert.ok(CRITERIA.every((name) => record.results[name] === "PASS"));
  assert.equal(record.offline.providerRequests, 0);
  assert.equal(record.install.ok, true);
  assert.equal(record.live.status, "NOT RUN");
});

test("a failed criterion, a provider request or any recorded failure makes the record NO-GO", () => {
  // Given an otherwise passing offline result.
  const passing = { mode: "offline_fake_transport", decision: "GO", providerRequests: 0,
    criteria: Object.fromEntries(CRITERIA.map((name) => [name, "PASS"])),
    runtime: { bun: pins.bun, bunLockSha256: lock.sha256 },
    install: { ok: true, lockUnchanged: true, lockSha256: lock.sha256, peers: [] },
    harnessPortFailures: [], upstreamIncompatibilities: [] };
  assert.equal(goRecord(passing, null).verdict, "GO");
  const failure = { step: "x", origin: "harness_port", code: "x" };
  // When any single condition fails, then the verdict is NO-GO and live stays a separate column.
  for (const variant of [
    { criteria: { ...passing.criteria, cancellation: "FAIL" } },
    { criteria: { ...passing.criteria, busy_input: "NOT_RUN" } },
    { providerRequests: 1 },
    { harnessPortFailures: [failure] },
    { upstreamIncompatibilities: [{ ...failure, origin: "upstream_incompatibility" }] },
    { install: { ...passing.install, ok: false } },
    { runtime: { ...passing.runtime, bunLockSha256: "0".repeat(64) } },
  ]) {
    const record = goRecord({ ...passing, ...variant }, null);
    assert.equal(record.verdict, "NO-GO", JSON.stringify(variant));
    assert.equal(record.live.status, "NOT RUN");
  }
});
