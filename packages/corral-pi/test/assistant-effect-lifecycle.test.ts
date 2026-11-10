// One offline close journey, real disposable executors; no provider or live owner store.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PlanningPolicy } from "../src/host/planning-policy.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { loadModules } from "../src/host/modules.ts";
import { McpSettingsSchema } from "../src/host/mcp-settings.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { processGroup, startTimeOf } from "../src/extensions/jobs/runtime.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { EventLog } from "../src/bridge/events.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

test("assistant close verifies MCP/LSP exit before settling their admissions exactly once", async () => {
  const assets = buildFixtureInstall(), work = mkdtempSync(join(tmpdir(), "assistant-effects-"));
  const cwd = join(work, "ws"); mkdirSync(cwd);
  const peers = ["mcp", "lsp"].map(kind => ({ kind, pid: join(work, `${kind}.pid`), script: join(work, `${kind}.ts`) }));
  for (const peer of peers) {
    const source = fileURLToPath(new URL(peer.kind === "mcp" ? "./fixtures/mcp-stdio.ts"
      : "../src/extensions/lsp/testing/fake-server.ts", import.meta.url));
    writeFileSync(peer.script, `import { writeFileSync } from "node:fs"; writeFileSync(${JSON.stringify(peer.pid)}, String(process.pid)); await import(${JSON.stringify(source)});`);
  }
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  const owned: { pid: number; start: string | undefined }[] = [];
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant"), transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("missing offline transport");
    const network = guardedFetch(transport, undefined, "openai-codex"); globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "assistant" as const, assistant: "A-1", generation: 1 };
    const corral = writeFakeCorral(work); corral.snapshot({ session: { id: "A-1", scratch: [] } });
    const client = new CorralClient({ corral: corral.path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    const mcp = McpSettingsSchema.parse({ servers: { docs: { projects: ["demo"], command: process.execPath,
      args: [peers[0]!.script], tools: { lookup: "read" } } } });
    writeFileSync(join(cwd, "fixture.ts"), "const def = 1;\n");
    writeFileSync(join(cwd, "notes.txt"), "not a supported LSP target\n");
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", client,
      events: new EventLog(1), network, modules, ui: "plain",
      capabilities: { catalog: 9, digest: "b".repeat(64), preset: "standard", enabled: ["todo", "mcp.enabled", "lsp"], mcp },
      guidance: { projectText: false, settings: { lsp: { languages: { fixture: { command: process.execPath,
        args: [peers[1]!.script], extensions: { ".ts": "typescript" } } } } } as never },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
    await host.session.bindExtensions({}); await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const transitions = () => corral.calls().flatMap(c => c.envelope.payload.kind === "assistant_boundary"
      ? [c.envelope.payload.transition as { action: string; tool: string; id: string }] : []);
    fixtureTransport().script([{ toolCalls: [
      { callId: "missing", name: "lsp_definition", arguments: { path: "missing.ts", line: 1, column: 1 } },
      { callId: "unsupported", name: "lsp_diagnostics", arguments: { path: "notes.txt" } },
    ] }, { text: "targets refused" }]);
    await host.session.prompt("Inspect rejected targets", { source: "interactive" });
    expect(existsSync(peers[1]!.pid)).toBeFalse();
    const toolStarts = transitions().filter(t => t.action === "start" && t.tool.startsWith("lsp_"));
    expect(toolStarts.map(t => t.tool).sort()).toEqual(["lsp_definition", "lsp_diagnostics"]);
    expect(transitions().filter(t => t.action === "settle" && toolStarts.some(start => start.id === t.id))).toHaveLength(2);
    const rejected = host.session.messages.filter(m => m.role === "toolResult" && ["lsp_definition", "lsp_diagnostics"].includes(m.toolName));
    expect(rejected).toHaveLength(2);
    expect(rejected.every(m => m.role === "toolResult" && m.isError)).toBeTrue();
    console.log("rejected targets: no LSP process or manager admission; both per-tool admissions settled");
    // Exercise the actual no-executor session_shutdown path before a real LSP start.
    const firstPid = Number(readFileSync(peers[0]!.pid, "utf8"));
    const firstMcp = { pid: firstPid, start: startTimeOf(firstPid) }; owned.push(firstMcp);
    expect(firstMcp.start).toBeDefined();
    await host.session.reload();
    expect(processGroup(firstMcp.pid, firstMcp.start).members).toBe(0);
    expect(transitions().filter(t => t.action === "start" && t.tool === "lsp_startup")).toHaveLength(0);
    expect(host.session.sessionManager.getEntries().filter(e => e.type === "custom" && e.customType === "corral_effect_lifecycle"
      && (e.data as { admission: string }).admission.endsWith(":lsp"))).toHaveLength(0);
    const afterReload = transitions().length;
    console.log("empty LSP shutdown/reload: no lifetime admission, seal or settlement; owned MCP exited");
    fixtureTransport().script([{ toolCalls: [{ callId: "lsp", name: "lsp_definition", arguments: { path: "fixture.ts", line: 1, column: 7 } }] }, { text: "done" }]);
    await host.session.prompt("Inspect fixture", { source: "interactive" });
    for (const peer of peers) {
      expect(existsSync(peer.pid)).toBeTrue();
      const pid = Number(readFileSync(peer.pid, "utf8")); owned.push({ pid, start: startTimeOf(pid) });
      expect(owned.at(-1)!.start).toBeDefined();
    }
    const resourceIds = transitions().filter(t => t.action === "start" && ["mcp_connect", "lsp_startup"].includes(t.tool)).map(t => t.id);
    const retiredMcp = transitions().slice(0, afterReload).find(t => t.action === "settle" && t.id.endsWith(":mcp"))?.id;
    expect(retiredMcp).toBeDefined();
    const currentIds = resourceIds.filter(id => id !== retiredMcp);
    expect(currentIds).toHaveLength(2);
    const call = client.call.bind(client), attempts: string[] = [];
    const refused = currentIds.find(id => id.endsWith(":mcp"));
    client.call = async (...args) => {
      const transition = args[1].payload?.transition as { action?: string; id?: string } | undefined;
      if (transition?.action === "settle" && transition.id && resourceIds.includes(transition.id)) {
        const child = owned[transition.id.endsWith(":mcp") ? 1 : 2]!;
        expect(processGroup(child.pid, child.start).members).toBe(0);
        attempts.push(transition.id);
        if (transition.id === refused) throw new Error("fixture settlement CAS conflict; terminal proof retained");
      }
      return call(...args);
    };
    await expect(host.close()).rejects.toThrow("managed resource shutdown/settlement incomplete");
    await expect(host.close()).rejects.toThrow("managed resource shutdown/settlement incomplete");
    expect(attempts.sort()).toEqual(currentIds.sort()); // no shutdown or settlement retry
    const settled = transitions().filter(t => t.action === "settle" && resourceIds.includes(t.id));
    expect(settled.map(t => t.id)).toEqual(resourceIds.filter(id => id !== refused));
    const journal = host.session.sessionManager.getEntries().filter(e => e.type === "custom" && e.customType === "corral_effect_lifecycle");
    const terminals = journal.flatMap(e => {
      const data = (e as { data: { admission: string; event: { action: string } } }).data;
      return data.event.action === "terminal" ? [data.admission] : [];
    });
    expect(terminals.sort()).toEqual([...resourceIds].sort()); // exactly one terminal per original admission
    const recovered = new PlanningPolicy({ read: async () => JSON.parse(readFileSync(join(work, "planning-boundary.json"), "utf8")),
      record: async () => { throw new Error("resumed policy must not synthesize settlement"); }, fence() {} });
    await recovered.refresh(); expect(recovered.restricted).toBeTrue();
    for (const child of owned) expect(processGroup(child.pid, child.start).members).toBe(0);
  } finally {
    // Fixture-only ownership cleanup, including the pre-fix close leak. Never guess a PID.
    try { await host?.close(); } catch { /* retained CAS/unknown outcome, never retried as repair */ }
    for (const child of owned) if (startTimeOf(child.pid) === child.start) {
      try { process.kill(child.pid, "SIGKILL"); } catch { /* already exited */ }
    }
    host?.session.dispose();
    globalThis.fetch = originalFetch; assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 20_000);
