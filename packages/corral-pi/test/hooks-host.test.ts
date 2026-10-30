// Hooks and bash rules in the real worker host (T-572): deny wins, ask runs only after the
// owner's bound permission answer, allowed commands work, and hooks wrap only top-level local
// calls with durable session records, never corral operations, denied calls or nested calls.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { PERMISSION_REF, commandDigest } from "../src/capabilities/bash-rules.ts";
import { HOOK_ENTRY } from "../src/capabilities/hooks.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const record = (tool: string) => ({ name: "mark", argv: ["sh", "-c", `printf '%s\\n' "\${CORRAL_HOOK_TOOL:-${tool}}" >> hooks.log`] });
const capabilities: CapabilitySet = {
  catalog: 2, digest: "b".repeat(64), preset: "full", enabled: ["todo", "hooks", "bash_rules"],
};
const guidance = { projectText: false, settings: {
  hooks: { timeout_ms: 10_000, pre_tool: [record("tool")], post_tool: [], turn_end: [{ ...record("turn_end"), name: "end" }] },
  bash_rules: { allow: [], ask: ["touch asked*"], deny: ["rm *"], default: "allow" },
} };

test("bash rules and hooks run around worker tools with durable hook attribution", async () => {
  // Given the production worker with full-preset hooks and bash rules, and a nesting tool.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-hooks-host-"));
  const cwd = join(work, "cwd");
  mkdirSync(cwd);
  writeFileSync(join(cwd, "notes.txt"), "keep\n");
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-572", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(work);
    const client = new CorralClient({
      corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    const nest: ToolDefinition = {
      name: "fixture_nest", label: "Nest", description: "Runs nested calls",
      parameters: Type.Object({}),
      async execute(_id, _params, signal, _update, ctx) {
        const denied = await ctx.executeTool("bash", { command: "rm notes.txt" }, { signal });
        const flagged = await ctx.executeTool("bash", { command: "bash -lc 'rm notes.txt'" }, { signal });
        const asked = await ctx.executeTool("bash", { command: "sh -ec 'touch asked.txt'" }, { signal });
        const read = await ctx.executeTool("read", { path: join(cwd, "notes.txt") }, { signal });
        const errors = [denied.isError, flagged.isError, asked.isError, read.isError];
        return { content: [{ type: "text", text: JSON.stringify(errors) }], details: undefined };
      },
    };
    const role = workerRole(PACKAGE_ROOT);
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, guidance, client, events, network,
      modules: { ...modules, role: {
        ...role, localToolNames: [...role.localToolNames ?? [], nest.name],
        localTools: (context: RoleContext) => [...role.localTools?.(context) ?? [], nest],
      } },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const live = host;
    const turn = async (stage: string, origin: string, text: string, script: Parameters<ReturnType<typeof fixtureTransport>["script"]>[0]) => {
      fixtureTransport().script(script);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await live.submit({ text, busyPolicy: "prompt", idempotencyKey: stage, origin }, `rq-${stage}`)).disposition)
        .toBe("started");
      await ended;
      await live.drained();
    };
    const result = (callId: string) => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      const text = message.content.map((part) => (part.type === "text" ? part.text : "")).join("");
      return { isError: message.isError, text };
    };
    const kinds = () => events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events.map((e) => e.kind);

    // When one turn reads, runs denied, ask and allowed commands, calls corral and nests calls.
    await turn("t1", "test", "work", [
      { toolCalls: [{ callId: "t1-read", name: "read", arguments: { path: join(cwd, "notes.txt") } }] },
      { toolCalls: [{ callId: "t1-deny", name: "bash", arguments: { command: "echo hi && rm notes.txt" } }] },
      { toolCalls: [{ callId: "t1-ask", name: "bash", arguments: { command: "touch asked.txt" } }] },
      { toolCalls: [{ callId: "t1-deny-flags", name: "bash", arguments: { command: "sh -ec 'rm notes.txt'" } }] },
      { toolCalls: [{ callId: "t1-ask-flags", name: "bash", arguments: { command: "bash -lc 'touch asked.txt'" } }] },
      { toolCalls: [{ callId: "t1-allow", name: "bash", arguments: { command: "echo ok > allowed.txt" } }] },
      { toolCalls: [{ callId: "t1-todo", name: "corral_todo", arguments: { op: "start", step: "hooks" } }] },
      { toolCalls: [{ callId: "t1-nest", name: "fixture_nest", arguments: {} }] },
      { text: "Finished" },
    ]);
    // Then deny wins (also nested), ask runs nothing yet, and the allowed command works.
    expect(result("t1-deny")).toEqual({ isError: true, text: "bash rule \"rm *\" denies \"rm notes.txt\"; nothing ran" });
    expect(result("t1-nest").text).toBe("[true,true,true,false]");
    expect(result("t1-deny-flags")).toEqual({ isError: true, text: "bash rule \"rm *\" denies \"rm notes.txt\"; nothing ran" });
    expect(result("t1-ask-flags").isError).toBeTrue();
    expect(result("t1-ask-flags").text).toContain(`${PERMISSION_REF}${commandDigest("bash -lc 'touch asked.txt'")}`);
    expect(existsSync(join(cwd, "notes.txt"))).toBeTrue();
    const ref = `${PERMISSION_REF}${commandDigest("touch asked.txt")}`;
    expect(result("t1-ask").isError).toBeTrue();
    expect(result("t1-ask").text).toContain(ref);
    expect(existsSync(join(cwd, "asked.txt"))).toBeFalse();
    expect(readFileSync(join(cwd, "allowed.txt"), "utf8")).toBe("ok\n");
    // And hooks ran only around top-level local calls that passed the checks, plus the turn end.
    expect(readFileSync(join(cwd, "hooks.log"), "utf8")).toBe("read\nbash\nfixture_nest\nturn_end\n");
    const entries = readFileSync(live.session.sessionFile ?? "", "utf8").trim().split("\n")
      .map((line) => JSON.parse(line) as { type: string; customType?: string; data?: Record<string, unknown> })
      .filter((entry) => entry.type === "custom" && entry.customType === HOOK_ENTRY).map((entry) => entry.data ?? {});
    const byInvocation = new Map<string, string[]>();
    for (const entry of entries) byInvocation.set(String(entry.invocation), [...byInvocation.get(String(entry.invocation)) ?? [], String(entry.state)]);
    expect([...byInvocation.values()].every((states) => states.join(",") === "prepared,started,completed")).toBeTrue();
    expect([...new Set(entries.map((entry) => entry.tool_call_id === undefined ? `turn ${entry.turn}` : String(entry.tool_call_id).split("|")[0]))])
      .toEqual(["t1-read", "t1-allow", "t1-nest", "turn 1"]);
    expect(kinds().filter((kind) => kind === "bash_rule_denied")).toHaveLength(4);
    expect(kinds().filter((kind) => kind === "bash_permission_required")).toHaveLength(3);

    // When the model asks the bound permission question and the owner's answer is delivered.
    await turn("t2", "test", "ask", [
      { toolCalls: [{ callId: "t2-ask", name: "corral_ask", arguments: {
        kind: "permission", text: "May I run touch asked.txt?", options: ["allow", "deny"], context_refs: [ref] } }] },
      { text: "Asked" },
    ]);
    expect(kinds()).toContain("bash_permission_asked");
    await turn("t3", "corral-inbox", "Answer to T-572/R1/Q1 (revision 2) by Owner:\nyes\nChosen option: allow", [
      { toolCalls: [{ callId: "t3-run", name: "bash", arguments: { command: "touch asked.txt" } }] },
      { toolCalls: [{ callId: "t3-again", name: "bash", arguments: { command: "touch asked.txt" } }] },
      { text: "Done" },
    ]);
    // Then the identical command runs once after the answer and the next run asks again.
    expect(result("t3-run").isError).toBeFalse();
    expect(existsSync(join(cwd, "asked.txt"))).toBeTrue();
    expect(result("t3-again").isError).toBeTrue();
    expect(kinds().filter((kind) => kind === "bash_permission_used")).toHaveLength(1);
    // And corral received the ask and todo but never saw a hook.
    expect(corral.calls().map(({ envelope }) => envelope.op).filter((op) => op !== "event" && op !== "hello"))
      .toEqual(["todo", "ask"]);
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
