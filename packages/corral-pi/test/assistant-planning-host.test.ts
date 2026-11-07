// One offline host journey: entry during a pending file dialog and retained Code Mode
// callback, then direct/nested/dynamic/user-shell attempts and a generation-runtime reload.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { EventLog } from "../src/bridge/events.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { loadModules } from "../src/host/modules.ts";
import { McpSettingsSchema } from "../src/host/mcp-settings.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { mcpServer } from "./support/mcp-server.ts";
import { nextEvent } from "./support/wait.ts";

test("planning fences pending grants, retained children, MCP, direct shell and reload before effects", async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "assistant-planning-"));
  const cwd = join(work, "ws"), temp = join(work, "temp");
  mkdirSync(cwd); mkdirSync(temp);
  const product = join(cwd, "product.txt"), escape = join(temp, "escape.txt");
  writeFileSync(product, "unchanged\n");
  const originalFetch = globalThis.fetch, originalTemp = process.env.TMPDIR;
  const dialog = Promise.withResolvers<void>(), releaseDialog = Promise.withResolvers<string>();
  const child = Promise.withResolvers<void>(), releaseChild = Promise.withResolvers<void>();
  const peer = await mcpServer();
  let host: HostSession | undefined, forbidden = 0;
  let activateLate: () => void = () => { throw new Error("extension not bound"); };
  try {
    restrictEnvironment(process.env); process.env.TMPDIR = temp;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("missing offline transport");
    const mcp = McpSettingsSchema.parse({ servers: { docs: { projects: ["demo"], url: peer.url,
      tools: { lookup: "read" }, resources: ["fixture://public/*"] } } });
    const network = guardedFetch(transport, undefined, "openai-codex", undefined, undefined,
      { origins: [new URL(peer.url).origin], send: originalFetch.bind(globalThis) });
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "assistant" as const, assistant: "A-1", generation: 1 };
    const events = new EventLog(1), corral = writeFakeCorral(work);
    corral.snapshot({ session: { id: "A-1", scratch: [] } });
    const client = new CorralClient({ corral: corral.path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    const reader: ToolDefinition = { name: "read", label: "Read barrier", description: "Fixture read-only inspection",
      parameters: Type.Object({ path: Type.String() }), async execute(_id, params: { path: string }) {
        if (params.path === "barrier") { child.resolve(); await releaseChild.promise; }
        return { content: [{ type: "text", text: "inspection" }], details: undefined };
      } };
    const unknown: ToolDefinition = { name: "fixture_unknown", label: "Unknown effect", description: "Must never execute",
      parameters: Type.Object({}), async execute() { forbidden++; return { content: [], details: undefined }; } };
    const late = { ...unknown, name: "fixture_late", exposure: "hidden" as const };
    const role = assistantRole(PACKAGE_ROOT);
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", client, events, network,
      capabilities: { catalog: 9, digest: "b".repeat(64), preset: "standard", enabled: ["todo", "codemode", "write_permissions", "mcp.enabled", "tool_search"], mcp },
      guidance: { projectText: false, settings: { write_permissions: { allow: [], deny: [] } } as never },
      modules: { ...modules, role: { ...role, localToolNames: [reader.name, unknown.name, late.name], localTools: () => [reader, unknown, late],
        extend: (pi, context) => {
          role.extend?.(pi, context);
          activateLate = () => {
            pi.registerTool({ ...late, exposure: "direct" });
            pi.setActiveTools([...pi.getActiveTools(), late.name]); // deliberately defeat hiding
          };
        },
      } },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")), ui: "plain" });
    await host.session.bindExtensions({ uiContext: { notify() {}, select: async () => {
      dialog.resolve(); return releaseDialog.promise;
    } } as unknown as ExtensionUIContext });
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    expect(peer.calls.filter(c => c.method === "initialize")).toHaveLength(1);
    fixtureTransport().script([
      { toolCalls: [{ callId: "retained", name: "codemode", arguments: { wait_ms: 0, code:
        `const retained = tools.write; await new Promise(resolve => setTimeout(async () => { await tools.read({path:"barrier"}); try { await retained({path:${JSON.stringify(escape)},content:"forbidden"}); } catch(e) { print(e.message); } resolve(); }, 0));` } }] },
      { toolCalls: [{ callId: "grant", name: "write", arguments: { path: product, content: "forbidden" } }] },
      { text: "entry settled" },
    ]);
    const running = host.session.prompt("Explicitly plan this without implementing", { source: "interactive" });
    await Promise.all([dialog.promise, child.promise]);
    const entered = await host.enterPlanning("A-1/g1/i1");
    expect(entered.phase).toBe("planning");
    const retainedDenied = nextEvent(events, e => e.kind === "code_call" && e.tool === "write" && e.phase === "settled");
    releaseDialog.resolve("Allow once"); releaseChild.resolve();
    await running; await host.drained();
    expect((await retainedDenied).effect).toBe("not_started");
    await host.codeNoticed();
    activateLate();
    expect(readFileSync(product, "utf8")).toBe("unchanged\n");
    expect(existsSync(escape)).toBeFalse();
    expect(corral.calls().filter(c => c.envelope.op === "write_permission" && c.envelope.payload.action === "decide")).toHaveLength(0);
    const calls = peer.calls.length;
    const attempts = [
      ["write", { path: escape, content: "forbidden" }],
      ["edit", { path: product, edits: [{ oldText: "unchanged", newText: "forbidden" }] }],
      ["bash", { command: `printf forbidden > '${escape}'` }],
      ["job_start", { command: `printf forbidden > '${escape}'` }],
      ["mcp_connect", { server: "docs" }], ["mcp_activate", { server: "docs", tools: [] }],
      ["mcp_resources", { server: "docs" }], ["mcp_read_resource", { server: "docs", uri: "fixture://public/note" }],
      ["fixture_unknown", {}], ["fixture_late", {}], ["corral_owner_action", { kind: "approve", target: "T-1", input: "A-1/g1/i1" }],
    ] as const;
    const check = async (prefix: string) => {
      fixtureTransport().script([
        ...attempts.map(([name, args], i) => ({ toolCalls: [{ callId: `${prefix}-${i}`, name, arguments: args }] })),
        { toolCalls: [{ callId: `${prefix}-nested`, name: "codemode", arguments: { code:
          `const names = ["write","bash","mcp_connect","fixture_unknown"]; for (const n of names) { try { await tools[n](n === "write" ? {path:${JSON.stringify(escape)},content:"forbidden"} : n === "bash" ? {command:"echo forbidden"} : n === "mcp_connect" ? {server:"docs"} : {}); } catch (e) { print(e.message); } }` } }] },
        { text: "read-only discussion" },
      ]);
      await host!.session.prompt("fix this part of the design", { source: "interactive" }); await host!.drained();
      for (let i = 0; i < attempts.length; i++) {
        const message = host!.session.messages.find(m => m.role === "toolResult" && m.toolCallId.split("|")[0] === `${prefix}-${i}`);
        expect(message?.role === "toolResult" && message.isError).toBeTrue();
      }
      for (const excludeFromContext of [false, true]) {
        const outcome = await host!.session.extensionRunner.emitUserBash({ type: "user_bash", command: `echo forbidden > '${escape}'`, cwd, excludeFromContext });
        expect(outcome?.result?.exitCode).toBe(1);
      }
      expect(readFileSync(product, "utf8")).toBe("unchanged\n"); expect(existsSync(escape)).toBeFalse();
      expect(forbidden).toBe(0); expect(peer.calls).toHaveLength(calls);
    };
    await check("planning");
    await host.session.reload();
    await check("reload");
  } finally {
    releaseDialog.resolve("Deny"); releaseChild.resolve();
    await host?.close(); await peer.close();
    globalThis.fetch = originalFetch; process.env.TMPDIR = originalTemp;
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 30_000);
