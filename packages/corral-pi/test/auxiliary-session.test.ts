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
      oracle: { model: "openai-codex/gpt-6.1-sol", timeout_secs: 30 } };
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
    await runtime.close();
    console.log("real SDK: allowed read succeeded; write/recursive spawn failed; oracle declared zero tools; cancellation saved once");
  } finally { globalThis.fetch = originalFetch; rmSync(cwd, { recursive: true, force: true }); }
}, 30_000);
