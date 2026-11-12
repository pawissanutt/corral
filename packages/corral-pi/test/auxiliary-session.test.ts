import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import auth, { fixtureTransport } from "./fixtures/auth/index.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import { authContext, restrictProvider } from "../src/host/session.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { runAuxiliarySession } from "../src/extensions/subagent/session.ts";
import { AuxiliaryRuntime } from "../src/extensions/subagent/runtime.ts";

test("real SDK explore reads but cannot write or recurse; oracle has no tools and persists cancellation", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "corral-auxiliary-"));
  const originalFetch = globalThis.fetch;
  try {
    const transport = fixtureTransport();
    const network = guardedFetch(transport.send);
    globalThis.fetch = network.fetch;
    const models = await createModelCatalog(auth.credentials(authContext()), { providers: ["openai-codex"] });
    restrictProvider(models, "openai-codex", network.fetch as typeof globalThis.fetch);
    writeFileSync(join(cwd, "source.txt"), "fixture source");
    const settings = { explore: { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 30 },
      oracle: { model: "openai-codex/gpt-6.1-sol", timeout_secs: 30 },
      shadow: { model: "openai-codex/gpt-6-luna", timeout_secs: 60, max_per_hour: 30 },
      expert: { model: "openai-codex/gpt-6.1-sol", max_turns: 3, timeout_secs: 600, max_parallel: 1 } };
    const refs: Record<string, unknown>[] = [];
    const runtime = new AuxiliaryRuntime({ settings, generation: 4, provider: "openai-codex", model: "gpt-6.1-sol",
      sessionId: () => "parent",
      async record(payload) { refs.push(payload); return { session_id: payload.request, session_file: join(cwd, `${payload.request}.jsonl`) }; },
      execute: (invocation, signal) => runAuxiliarySession({ sdk, models, provider: "openai-codex", cwd,
        agentDir: join(cwd, "agent"), parentLoadout: ["read", "write", "bash", "explore", "oracle"], localTools: [] }, invocation, signal),
    });
    transport.script([
      { toolCalls: [{ callId: "read", name: "read", arguments: { path: "source.txt" } }] },
      { toolCalls: [{ callId: "write", name: "write", arguments: { path: "bad.txt", content: "bad" } }] },
      { toolCalls: [{ callId: "recurse", name: "explore", arguments: { prompt: "spawn" } }] },
      { text: "research finished" },
    ]);
    const result = await runtime.run("explore", "research", "read source");
    expect(result.text).toBe("research finished");
    const child = readFileSync(join(cwd, `${result.request}.jsonl`), "utf8").split("\n").filter(Boolean).map((s) => JSON.parse(s));
    const toolResults = child.filter((e) => e.message?.role === "toolResult").map((e) => e.message);
    expect(toolResults.map((r) => [r.toolName, r.isError])).toEqual([["read", false], ["write", true], ["explore", true]]);
    expect(existsSync(join(cwd, "bad.txt"))).toBeFalse();
    expect(child.some((e) => e.message?.role === "assistant" && e.message.usage?.input > 0)).toBeTrue();
    // Regression: tool-only research used all its turns and returned no findings. The final
    // allowed request must instead have no tools, without adding a thirteenth request.
    const beforeResearch = transport.requests;
    settings.explore.max_turns = 2;
    transport.script([
      { toolCalls: [{ callId: "bounded-read", name: "read", arguments: { path: "source.txt" } }] },
      { text: "source.txt:1 contains fixture source; further coverage is incomplete" },
    ]);
    const bounded = await runtime.run("explore", "bounded", "Find the source and report evidence");
    expect(transport.requests - beforeResearch).toBe(2);
    const boundedChild = readFileSync(join(cwd, `${bounded.request}.jsonl`), "utf8").split("\n").filter(Boolean).map(s => JSON.parse(s));
    expect(boundedChild.flatMap(entry => entry.message?.toolsRemoved ?? []).map(tool => tool.name)).toContain("read");
    expect(bounded.text).toContain("source.txt:1");
    expect(bounded.report).toMatchObject({ status: "partial", stopReason: "turn_limit" });
    transport.script([{ text: "source.txt:1 already answers the narrow question" }]);
    const early = await runtime.run("explore", "early", "Answer from supplied context");
    expect(early.report).toMatchObject({ status: "completed", stopReason: "completed" });
    // A provider that ignores tool removal still cannot execute tools or spend another turn.
    transport.script([
      { toolCalls: [{ callId: "evidence", name: "read", arguments: { path: "source.txt" } }] },
      { toolCalls: [{ callId: "ignored-removal", name: "read", arguments: { path: "missing.txt" } }] },
    ]);
    const beforeFallback = transport.requests;
    const fallback = await runtime.run("explore", "fallback", "Research source");
    expect(transport.requests - beforeFallback).toBe(2);
    expect(fallback.report).toMatchObject({ status: "partial", stopReason: "turn_limit" });
    expect(fallback.text).toContain("fixture source");
    expect(fallback.text).toContain("source.txt");
    expect(fallback.outcome).toBe("uncertain");
    // Time-reserve synthesis is also a single final request, even before the turn ceiling.
    settings.explore.max_turns = 12;
    settings.explore.timeout_secs = 4;
    transport.script([{ toolCalls: [{ callId: "reserve", name: "read", arguments: { path: "source.txt" } }] }]);
    const beforeReserve = transport.requests;
    const reserved = await runtime.run("explore", "reserve", "Answer from supplied context");
    expect(transport.requests - beforeReserve).toBe(1);
    expect(reserved.report?.stopReason).toBe("time_reserve");
    settings.explore.timeout_secs = 30;
    // Interrupt an actual research stream: no tool-free synthesis may follow the cancellation.
    const stopResearch = new AbortController();
    transport.script([{ text: "held research" }]);
    transport.holdNext();
    const researchHeld = transport.whenHeld();
    const beforeCancel = transport.requests;
    const interrupted = runtime.run("explore", "cancelled", "Research source", stopResearch.signal);
    await researchHeld;
    stopResearch.abort();
    expect((await interrupted).report).toMatchObject({ status: "failed", stopReason: "cancelled" });
    expect(transport.requests - beforeCancel).toBe(1);
    const cancel = new AbortController();
    transport.script([{ text: "oracle partial response" }]);
    transport.holdNext();
    const held = transport.whenHeld();
    const request = runtime.run("oracle", "consult", "review this approach", cancel.signal);
    await held;
    cancel.abort();
    const oracle = await request;
    expect(oracle.outcome).toBe("uncertain");
    const transcript = readFileSync(join(cwd, `${oracle.request}.jsonl`), "utf8").split("\n").filter(Boolean).map((s) => JSON.parse(s));
    const declarations = transcript.flatMap((e) => e.message?.toolsAdded ?? []);
    expect(declarations).toHaveLength(0);
    expect(refs.filter((r) => r.request === oracle.request && r.action === "finish")).toHaveLength(1);
    expect(refs.at(-1)?.outcome).toBe("uncertain");
    // A tool-hallucinating oracle cannot turn its single completion into a request loop.
    const before = transport.requests;
    transport.script([{ toolCalls: [{ callId: "forbidden", name: "read", arguments: { path: "source.txt" } }] }]);
    const limited = await runtime.run("oracle", "limited", "consult once");
    expect(limited.outcome).toBe("uncertain");
    expect(transport.requests - before).toBe(1);
    // The detached shadow uses the same isolated SDK path without inheriting tools/resources.
    transport.script([{ text: '{"choice":"wait","reason":"unsure"}' }]);
    const shadow = await runtime.run("shadow", "shadow", "Choose wait or escalate", undefined, undefined, ["wait"]);
    expect(shadow.outcome).toBe("completed");
    const shadowTranscript = readFileSync(join(cwd, `${shadow.request}.jsonl`), "utf8").split("\n").filter(Boolean).map(s => JSON.parse(s));
    expect(shadowTranscript.flatMap(entry => entry.message?.toolsAdded ?? [])).toHaveLength(0);
    expect(refs.filter(event => event.usage_kind === "shadow" && event.action === "finish").map(event => event.outcome)).toEqual(["completed"]);
    transport.script([
      { toolCalls: [
        { callId: "expert-read", name: "read", arguments: { path: "source.txt" } },
        { callId: "expert-write", name: "write", arguments: { path: "expert-bad.txt", content: "bad" } },
        { callId: "expert-shell", name: "bash", arguments: { command: "touch expert-bad.txt" } },
        { callId: "expert-recurse", name: "corral_expert", arguments: { question: "delegate" } },
      ] },
      { text: "source.txt:1 supports this advice." },
    ]);
    const expert = await runtime.run("expert", "consult", "Investigate source.txt");
    expect(expert).toMatchObject({ model: "openai-codex/gpt-6.1-sol", turns: 2, outcome: "completed" });
    const expertTranscript = readFileSync(join(cwd, `${expert.request}.jsonl`), "utf8").split("\n").filter(Boolean).map(s => JSON.parse(s));
    expect(expertTranscript.flatMap(entry => entry.message?.toolsAdded ?? []).map(tool => tool.name).sort()).toEqual(["find", "grep", "ls", "read"]);
    expect(expertTranscript.filter(entry => entry.message?.role === "toolResult").map(entry => [entry.message.toolName, entry.message.isError]))
      .toEqual([["read", false], ["write", true], ["bash", true], ["corral_expert", true]]);
    expect(existsSync(join(cwd, "expert-bad.txt"))).toBeFalse();
    await runtime.close();
    console.log("real SDK: allowed read succeeded; write/recursive spawn failed; oracle declared zero tools; cancellation saved once");
  } finally { globalThis.fetch = originalFetch; rmSync(cwd, { recursive: true, force: true }); }
}, 30_000);
