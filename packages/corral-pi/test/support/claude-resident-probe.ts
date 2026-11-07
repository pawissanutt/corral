// Called only inside the wire fixture's private network/PID namespaces. This is a timing probe,
// not a resident production provider: the fake endpoint does no model inference or real caching.
import assert from "node:assert/strict";
import { query, type Options, type SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";

function user(turn: number): SDKUserMessage {
  return { type: "user", message: { role: "user", content: `Synthetic timing turn ${turn}: reply briefly.` },
    parent_tool_use_id: null, session_id: "offline-timing" };
}
async function* single(turn: number) { yield user(turn); }

export async function measureResident(template: Options) {
  const abortController = new AbortController();
  const deadline = setTimeout(() => abortController.abort(), 20_000);
  const options: Options = { ...template, tools: [], mcpServers: {}, maxTurns: 1, abortController };
  try {
    const oneShotStart = performance.now();
    for (let turn = 0; turn < 3; turn++) {
      const stream = query({ prompt: single(turn), options });
      let results = 0;
      try {
        for await (const message of stream) if (message.type === "result") {
          assert(message.subtype === "success" && !message.is_error, "one-shot timing result failed");
          results++;
        }
      } finally { stream.close(); }
      assert.equal(results, 1);
    }
    const oneShotMs = performance.now() - oneShotStart;
    const gates = Array.from({ length: 3 }, () => Promise.withResolvers<void>());
    gates[0]?.resolve();
    async function* inputs() {
      for (const [turn, gate] of gates.entries()) {
        await gate.promise;
        yield user(turn);
      }
    }
    const residentStart = performance.now();
    const stream = query({ prompt: inputs(), options });
    let results = 0;
    try {
      for await (const message of stream) if (message.type === "result") {
        assert(message.subtype === "success" && !message.is_error, "resident timing result failed");
        results++;
        gates[results]?.resolve();
        if (results === gates.length) break;
      }
    } finally { stream.close(); }
    assert.equal(results, 3);
    return { model: "synthetic endpoint; no inference", requestsPerMode: 3,
      oneShotQueries: 3, residentQueries: 1, oneShotMs: Math.round(oneShotMs),
      residentMs: Math.round(performance.now() - residentStart),
      scope: "SDK process/query overhead only; not provider latency, cache-hit rate or production continuity" };
  } finally { clearTimeout(deadline); }
}
