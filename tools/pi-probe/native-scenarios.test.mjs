import { test } from "node:test";
import assert from "node:assert/strict";
import { ProbeFailure } from "./credentials.mjs";
import { classifyFailure } from "./native-scenarios.mjs";

/** @param {string} file */
function thrownAt(file) {
  const error = new TypeError("synthetic failure");
  error.stack = `TypeError: synthetic failure\n    at run (${file}:1:1)\n    at main (/probe/native-host.mjs:9:9)`;
  return error;
}

test("harness-port failures are kept apart from upstream incompatibility", () => {
  // Given failures raised by the probe harness and by observed upstream behavior.
  let assertion;
  try { assert.equal(1, 2); } catch (error) { assertion = error; }
  const cases = [
    [new ProbeFailure("bun_lock_digest_mismatch"), "harness_port"],
    [new ProbeFailure("no_held_provider", "upstream_incompatibility"), "upstream_incompatibility"],
    [assertion, "upstream_incompatibility"],
    [thrownAt("/probe/node_modules/@earendil-works/pi-ai/dist/api/openai-codex-responses.js"), "upstream_incompatibility"],
    [thrownAt("/probe/fake-transport.mjs"), "harness_port"],
  ];
  // When each is classified, then its origin follows the raising side.
  for (const [error, origin] of cases) assert.equal(classifyFailure(error).origin, origin);
  // Live runs never copy error text.
  assert.equal(classifyFailure(thrownAt("/probe/fake-transport.mjs"), false).detail, undefined);
});
