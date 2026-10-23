import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { USAGE_SCRIPT, DANGLING_CALLS, expectedUsage } from "./fixture-sessions.mjs";

/** @param {string} name */
const fixture = (name) => readFileSync(new URL("./fixtures/" + name, import.meta.url), "utf8");
const categories = ["input", "output", "cacheRead", "cacheWrite", "reasoning", "totalTokens"];

/** @param {string} name @returns {Promise<Array<Record<string, any>>>} */
async function entries(name) {
  const sdk = await import("@earendil-works/pi-coding-agent");
  return /** @type {Array<Record<string, any>>} */ (sdk.parseSessionEntries(fixture(name)));
}

test("the usage session's per-category totals equal the scripted synthetic usage", async () => {
  // Given the committed session and totals derived from the fake transport script alone.
  const expected = JSON.parse(fixture("session-usage.expected.json"));
  assert.deepEqual(expected.responses.map((/** @type {Record<string, unknown>} */ { responseId, text, ...usage }) => usage),
    USAGE_SCRIPT.map((step) => expectedUsage(/** @type {import("./fake-transport.mjs").FakeUsage} */ (step.usage))));
  // When upstream Pi parses the session file.
  const parsed = await entries("session-usage.jsonl");
  const assistants = parsed.filter((entry) => entry.type === "message" && entry.message.role === "assistant")
    .map((entry) => entry.message);
  // Then each response and the per-category sums match, keyed by unique response IDs.
  assert.equal(parsed[0].type, "session");
  assert.equal(parsed[0].id, expected.sessionId);
  assert.deepEqual(assistants.map((message) => message.responseId), expected.responses.map((/** @type {{responseId: string}} */ item) => item.responseId));
  for (const key of categories) {
    assert.equal(assistants.reduce((sum, message) => sum + message.usage[key], 0), expected.totals[key], key);
  }
});

test("the dangling session ends in one corral call and one upstream edit call without results", async () => {
  // Given the committed crash-point snapshot.
  const expected = JSON.parse(fixture("session-dangling.expected.json"));
  const sdk = await import("@earendil-works/pi-coding-agent");
  const parsed = await entries("session-dangling.jsonl");
  // When upstream Pi rebuilds the conversation from it.
  const context = sdk.buildSessionContext(/** @type {any} */ (parsed.filter((entry) => entry.type !== "session")));
  const last = /** @type {Record<string, any>} */ (context.messages.at(-1));
  const calls = last.content.filter((/** @type {{type: string}} */ part) => part.type === "toolCall");
  // Then the last turn holds exactly the two calls, and no tool result exists for either.
  assert.equal(parsed[0].id, expected.sessionId);
  assert.equal(last.role, "assistant");
  assert.deepEqual(calls.map((/** @type {{id: string, name: string}} */ call) => [call.id, call.name]),
    expected.calls.map((/** @type {{toolCallId: string, name: string}} */ call) => [call.toolCallId, call.name]));
  assert.deepEqual(expected.calls.map((/** @type {{class: string}} */ call) => call.class), DANGLING_CALLS.map((call) => call.class));
  assert.equal(context.messages.filter((message) => message.role === "toolResult").length, 0);
});
