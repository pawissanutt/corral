// Opt-in real-binary research, called only by the network/PID-isolated wire fixture.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFileSync, readdirSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { query, createSdkMcpServer, tool, type Options, type SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";
import { nativeSdkFailure } from "../../src/claude/events.ts";
import { HOST_TOOL_DENIAL_HOOKS, canUseTool } from "../../src/claude/tools.ts";

type Json = Record<string, unknown>;
const object = (v: unknown): v is Json => v !== null && typeof v === "object" && !Array.isArray(v);
function strip(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(strip);
  if (!object(v)) return v;
  return Object.fromEntries(Object.entries(v).filter(([k]) => k !== "cache_control").map(([k, x]) => [k, strip(x)]));
}
function markers(v: unknown): number {
  if (Array.isArray(v)) return v.reduce((n, x) => n + markers(x), 0);
  return object(v) ? Number("cache_control" in v) + Object.values(v).reduce<number>((n, x) => n + markers(x), 0) : 0;
}
function children(): { pid: string; rss: string; state: string; name: string }[] {
  return readdirSync("/proc").filter(p => /^\d+$/.test(p)).flatMap(pid => {
    try {
      const s = readFileSync(`/proc/${pid}/status`, "utf8");
      return Number(pid) !== process.pid ? [{ pid, rss: s.match(/VmRSS:\s*(.*)/)?.[1] ?? "none",
        state: s.match(/State:\s*(.*)/)?.[1] ?? "unknown", name: s.match(/Name:\s*(.*)/)?.[1] ?? "unknown" }] : [];
    } catch { return []; }
  });
}
export async function proveResident(template: Options) {
  const start = performance.now();
  const captures: Json[] = [];
  let transportError: unknown;
  let kind = "plain", activeTurn = 0, turnRequest = 0;
  let held = Promise.withResolvers<void>();
  let release = Promise.withResolvers<void>();
  const server = createServer(async (req, res) => {
    try {
      assert.equal(new URL(req.url ?? "", "http://127.0.0.1").pathname, "/v1/messages");
      let body = "";
      for await (const chunk of req) { body += String(chunk); assert(body.length < 256 * 1024); }
      const payload: unknown = JSON.parse(body); assert(object(payload)); captures.push(payload);
      turnRequest++;
      if (kind === "rate" || kind === "quota") {
        res.writeHead(429, { "content-type": "application/json", "retry-after": "0",
          "anthropic-ratelimit-unified-status": "rejected", "anthropic-ratelimit-unified-reset": "2000000000",
          ...(kind === "quota" ? { "anthropic-ratelimit-unified-representative-claim": "five_hour",
            "anthropic-ratelimit-unified-5h-utilization": "1.0", "anthropic-ratelimit-unified-5h-reset": "2000000000" } : {}) });
        res.end(JSON.stringify({ type: "error", error: { type: "rate_limit_error",
          message: kind === "quota" ? "Your usage limit has been reached" : "Rate limit exceeded" } })); return;
      }
      const events: Json[] = [{ type: "message_start", message: { id: `msg_${captures.length}`, type: "message",
        role: "assistant", model: "claude-opus-5-5", content: [], stop_reason: null,
        stop_sequence: null, usage: { input_tokens: 10 + activeTurn, output_tokens: 0 } } }];
      const calls = turnRequest === 1 && ["single", "parallel", "custom", "edit"].includes(kind);
      const names = calls ? (kind === "parallel" ? ["Read", "Read"] : [kind === "custom" ? "mcp__custom-tools__probe" : kind === "edit" ? "Edit" : "Read"]) : [];
      for (const [index, name] of names.entries()) {
        events.push({ type: "content_block_start", index, content_block: { type: "tool_use", id: `call_${activeTurn}_${index}`, name, input: {} } },
          { type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json: JSON.stringify(name === "Edit"
            ? { file_path: `${template.cwd}/fixture.txt`, old_string: "two", new_string: "three" }
            : name === "Read" ? { file_path: `${template.cwd}/fixture.txt` } : { value: "fixture" }) } },
          { type: "content_block_stop", index });
      }
      if (!calls) events.push({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
        { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "proof reply" } }, { type: "content_block_stop", index: 0 });
      res.writeHead(200, { "content-type": "text/event-stream" });
      for (const e of events) res.write(`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`);
      if (kind === "abort") { held.resolve(); await release.promise; }
      for (const e of [{ type: "message_delta", delta: { stop_reason: calls ? "tool_use" : "end_turn", stop_sequence: null }, usage: { output_tokens: 2 + activeTurn } }, { type: "message_stop" }]) {
        res.write(`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`);
      }
      res.end();
    } catch (error) { transportError = error; res.writeHead(500).end(); }
  });
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  const address = server.address(); assert(address && typeof address !== "string");
  let customExecutions = 0;
  const originalFile = readFileSync(`${template.cwd}/fixture.txt`, "utf8");
  const options: Options = { ...template, stderr: text => console.log(JSON.stringify({ proofStderr: text.slice(-3000) })), maxTurns: undefined, includePartialMessages: true,
    tools: ["Read", "Edit"], hooks: HOST_TOOL_DENIAL_HOOKS, canUseTool, permissionMode: "dontAsk",
    env: { ...template.env, ANTHROPIC_BASE_URL: `http://127.0.0.1:${address.port}`, CLAUDE_CODE_MAX_RETRIES: "0",
      DISABLE_PROMPT_CACHING: undefined, FORCE_PROMPT_CACHING_5M: "1", CLAUDE_CODE_PROMPT_CACHE_TTL: "5m" },
    mcpServers: { "custom-tools": createSdkMcpServer({ name: "custom-tools", tools: [tool("probe", "offline probe", { value: z.string() }, async () => { customExecutions++; throw new Error("custom tool executed"); })] }) } };
  function user(uuid: ReturnType<typeof randomUUID>, first: boolean): SDKUserMessage {
    return { type: "user", uuid, session_id: "offline-proof", parent_tool_use_id: null,
      message: { role: "user", content: [{ type: "text", text: `Synthetic proof ${uuid}`,
        ...(first ? { cache_control: { type: "ephemeral" as const } } : {}) }] } };
  }
  async function run(label: string, turns: string[], replay: boolean) {
    const gates = turns.map(() => Promise.withResolvers<void>()); gates[0]?.resolve();
    const ids = turns.map(() => randomUUID());
    const counts: number[] = [], events: unknown[] = [], failures: unknown[] = [];
    const before = captures.length;
    async function* inputs() {
      for (const [i, mode] of turns.entries()) {
        await gates[i]?.promise; kind = mode; activeTurn = i; turnRequest = 0;
        const id = ids[i]; assert(id); yield user(id, i === 0);
      }
    }
    const controller = new AbortController();
    const deadline = setTimeout(() => controller.abort(), 45_000);
    const stream = query({ prompt: inputs(), options: { ...options, abortController: controller,
      extraArgs: { ...options.extraArgs, ...(replay ? { "replay-user-messages": null } : {}) } } });
    let results = 0, rateLimit: unknown;
    try {
      for await (const m of stream) {
        if (m.type === "rate_limit_event") rateLimit = m.rate_limit_info;
        const failure = nativeSdkFailure(m, rateLimit, "coding");
        if (failure) failures.push({ type: m.type, error: String(failure), detail: failure });
        if (m.type === "result" || m.type === "rate_limit_event" || m.type === "assistant" && m.error ||
          m.type === "stream_event" && ["message_start", "message_delta"].includes(m.event.type)) events.push(m);
        if (m.type === "result") {
          counts.push(captures.length - before - counts.reduce((a, b) => a + b, 0));
          results++;
          console.log(JSON.stringify({ proof: label, turn: results, requests: counts.at(-1), id: ids[results - 1], result: m, idleChildren: children() }));
          gates[results]?.resolve();
          // Keep consuming after the final result: the finite input closes naturally, including late rate-limit events.
        }
      }
    } catch (error) {
      // Failed API turns can emit their terminal result and then exit nonzero; retain both signals.
      console.log(JSON.stringify({ proof: label, iteratorError: String(error) }));
      if (!turns.some(t => t === "rate" || t === "quota")) throw error;
    } finally { stream.close(); clearTimeout(deadline); }
    const bodies = captures.slice(before);
    const prefix = bodies.slice(1).map((next, i) => {
      const prev = bodies[i]; assert(prev);
      const a = strip(prev.messages), b = strip(next.messages);
      return { request: i + 2, system: JSON.stringify(strip(prev.system)) === JSON.stringify(strip(next.system)),
        tools: JSON.stringify(strip(prev.tools)) === JSON.stringify(strip(next.tools)),
        messages: Array.isArray(a) && Array.isArray(b) && JSON.stringify(a) === JSON.stringify(b.slice(0, a.length)) };
    });
    console.log(JSON.stringify({ proof: label, results, counts, events, failures, markers: bodies.map(markers), prefix,
      // Synthetic bodies only; retain exact inserted reminders and denied-tool text for the design record.
      messages: bodies.map(b => b.messages), childrenAfterClose: children() }));
    assert.equal(results, turns.length, `${label}: result count`);
    return { label, results, requests: captures.length - before };
  }
  try {
    const denial = await run("denial-with-replay", ["single", "plain", "parallel", "plain", "custom", "plain", "edit", "plain"], true);
    assert.equal(customExecutions, 0, "custom tool must stay host-owned");
    assert.equal(readFileSync(`${template.cwd}/fixture.txt`, "utf8"), originalFile, "Edit must not execute");
    const correlation = await run("without-replay", ["plain", "plain"], false);
    const failures = [];
    for (const mode of ["rate", "quota"]) {
      failures.push(await run(`${mode}-resident`, ["plain", mode], true));
      failures.push(await run(`${mode}-one-shot`, [mode], true));
    }
    kind = "abort"; held = Promise.withResolvers<void>(); release = Promise.withResolvers<void>();
    async function* input() { yield user(randomUUID(), true); await release.promise; }
    const stream = query({ prompt: input(), options });
    const consuming = (async () => { try { for await (const _ of stream) { /* drain until interrupt */ } } catch (error) { console.log("abort signal", String(error)); } })();
    await held.promise;
    console.log(JSON.stringify({ proof: "abort-before", children: children() }));
    await stream.interrupt(); stream.close(); release.resolve(); await consuming;
    // SDK close may return before waitpid; process exit is observed without a timing assertion.
    const cleanupDeadline = Date.now() + 5000;
    while (children().some(c => !c.state.startsWith("Z")) && Date.now() < cleanupDeadline) await new Promise(resolve => setTimeout(resolve, 10));
    console.log(JSON.stringify({ proof: "abort-after", children: children(),
      verdict: children().length === 0 ? "holds" : "changes the design: processes remain (including zombies)" }));
    if (transportError) throw transportError;
    return { denial, correlation, failures, abortChildren: children(), totalRequests: captures.length,
      elapsedMs: Math.round(performance.now() - start) };
  } finally {
    release.resolve(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve()));
  }
}
