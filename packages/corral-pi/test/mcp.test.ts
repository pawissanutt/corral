import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";
import { McpAccess, mcpToolName } from "../src/capabilities/mcp.ts";
import { McpSettingsSchema, resourceAllowed, toolAllowed } from "../src/host/mcp-settings.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { mcpServer } from "./support/mcp-server.ts";

test("configured policy, not MCP annotations, controls tools and resources across reconnect", async () => {
  // Given a real HTTP server advertising a read and misleadingly annotated write/proxy tools.
  const peer = await mcpServer();
  const raw = globalThis.fetch.bind(globalThis);
  const settings = McpSettingsSchema.parse({ servers: {
    docs: { projects: ["demo"], url: peer.url, tools: { lookup: "read", shell: "read", corral_done: "read" }, resources: ["fixture://public/*"] },
    other: { projects: ["other"], url: peer.url, tools: { lookup: "read" } },
  } });
  const network = guardedFetch(raw, undefined, "openai-codex", undefined, undefined, {
    origins: [new URL(peer.url).origin], send: raw,
  });
  const access = new McpAccess(settings, "reviewer", "demo", process.cwd(), network.mcp);
  try {
    for (const revision of [1, 2]) {
      if (revision === 2) peer.revise();
      // When connecting/reconnecting, calling an approved identity and attempting bypasses.
      await access.refresh();
      const schema = JSON.stringify(access.definitions(false)[0]?.parameters);
      peer.reorder();
      await access.refresh();
      expect(JSON.stringify(access.definitions(false)[0]?.parameters)).toBe(schema);
      const result = await access.call("docs", "lookup", { key: "note", ...(revision === 2 ? { version: 2 } : {}) });
      const before = peer.calls.length;
      await expect(access.call("docs", "write_file", {})).rejects.toThrow("mcp_denied");
      await expect(access.call("docs", "shell", {})).rejects.toThrow("mcp_denied");
      await expect(access.call("docs", "corral_done", {})).rejects.toThrow("mcp_denied");
      await expect(access.call("docs", "fresh_write", {})).rejects.toThrow("mcp_denied");
      await expect(access.call("other", "lookup", {})).rejects.toThrow("mcp_denied");
      await expect(access.readResource("docs", "fixture://private/secret")).rejects.toThrow("mcp_denied");
      // Then denial happens before I/O, cached schemas refresh without any digest approval.
      expect(peer.calls).toHaveLength(before);
      expect(result.content).toEqual([{ type: "text", text: JSON.stringify({
        tool: "lookup", revision, args: { key: "note", ...(revision === 2 ? { version: 2 } : {}) },
      }) }]);
      expect(access.names()).toEqual([mcpToolName("docs", "lookup")]);
      expect(JSON.parse(JSON.stringify(access.definitions(false)[0]?.parameters)).required).toEqual(revision === 1 ? ["key"] : ["key", "version"]);
      expect((await access.resources("docs")).map((resource) => resource.uri)).toEqual(["fixture://public/note"]);
      expect((await access.readResource("docs", "fixture://public/note")).contents).toHaveLength(1);
    }
  } finally { await access.close(); await peer.close(); }
});

test("empty project MCP is a useful unconfigured state without connecting", async () => {
  // Given no configured servers for the current project.
  const access = new McpAccess({ servers: {} }, "worker", "demo", process.cwd());
  try {
    // When discovery runs, then no fabricated server or tool is exposed.
    expect(await access.refresh()).toEqual({ configured: false, servers: [] });
    expect(access.names()).toEqual([]);
  } finally { await access.close(); }
});

test("configured stdio MCP connects and closes without the in-process network guard", async () => {
  // Given a real disposable stdio peer using the same pinned client transport.
  const settings = McpSettingsSchema.parse({ servers: { docs: {
    projects: ["demo"], command: process.execPath,
    args: [fileURLToPath(new URL("./fixtures/mcp-stdio.ts", import.meta.url))], tools: { lookup: "read" },
  } } });
  const access = new McpAccess(settings, "worker", "demo", process.cwd());
  try {
    // When connecting and invoking an approved tool, then no HTTP transport is needed.
    await access.refresh();
    expect((await access.call("docs", "lookup", {})).content).toEqual([{ type: "text", text: "stdio fixture" }]);
  } finally { await access.close(); }
  // Closing awaits the pinned transport's owned process-group cleanup.
  await expect(access.call("docs", "lookup", {})).rejects.toThrow("MCP session is closed");
});

test("MCP close joins startup but never converts an unanswered remote request into shutdown proof", async () => {
  const peer = await mcpServer(), raw = globalThis.fetch.bind(globalThis);
  const started = Promise.withResolvers<void>(), release = Promise.withResolvers<void>();
  let holdInitialize = true, loseReply = false, dispatched = 0;
  let reply: "normal" | "task" | "malformed" | "rpc_error" = "normal";
  const settings = McpSettingsSchema.parse({ servers: { docs: { projects: ["demo"], url: peer.url, tools: { lookup: "read" } } } });
  const facts: import("../src/host/managed-effects.ts").Lifecycle[] = [];
  const send: typeof raw = async (input, init) => {
    const message = typeof init?.body === "string" ? JSON.parse(init.body) as { method?: string; id?: number | string } : {};
    if (message.method === "initialize" && holdInitialize) { started.resolve(); await release.promise; }
    if (message.id !== undefined) dispatched++;
    const result = await raw(input, init);
    if (message.method === "tools/call" && loseReply) throw new Error("fixture disconnected after remote dispatch; no reply observed");
    if (message.method === "tools/call" && reply !== "normal") {
      const payload = reply === "task" ? { result: { task: { taskId: "async-1", status: "working" } } }
        : reply === "malformed" ? { result: { content: "not an array" } }
        : { error: { code: -32602, message: "fixture rejected arguments" } };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: message.id, ...payload }), { headers: { "content-type": "application/json" } });
    }
    return result;
  };
  const access = new McpAccess(settings, "assistant", "demo", process.cwd(), send, async event => { facts.push(event); });
  try {
    const opening = access.refresh(); await started.promise;
    const closing = access.close(); release.resolve(); await opening; await closing;
    expect(facts.filter(f => f.action === "terminal")).toHaveLength(1);
    await access.close(); expect(facts.filter(f => f.action === "terminal")).toHaveLength(1);
    await expect(access.refresh()).rejects.toThrow("MCP session is closed");
    holdInitialize = false;
    // Identity publication is an async pre-dispatch fence. Closing during it must join
    // that work and positively record not-started, never send the queued request later.
    const preparing = Promise.withResolvers<void>(), permit = Promise.withResolvers<void>();
    const fenced: import("../src/host/managed-effects.ts").Lifecycle[] = [];
    const before = dispatched;
    const guarded = new McpAccess(settings, "assistant", "demo", process.cwd(), send, async event => {
      fenced.push(event);
      if (event.action === "rpc_prepared") { preparing.resolve(); await permit.promise; }
    });
    const queued = guarded.refresh(); await preparing.promise;
    const stopped = guarded.close(); permit.resolve(); await queued; await stopped;
    expect(dispatched).toBe(before);
    const terminal = fenced.find(event => event.action === "terminal");
    expect(terminal?.action === "terminal" && terminal.terminal.requests[0]?.state).toBe("not_started");
    loseReply = true;
    const unknown: import("../src/host/managed-effects.ts").Lifecycle[] = [];
    const next = new McpAccess(settings, "assistant", "demo", process.cwd(), send, async event => { unknown.push(event); });
    await next.refresh();
    await expect(next.call("docs", "lookup", {})).rejects.toThrow("disconnected");
    await expect(next.close()).rejects.toThrow("ownership retained");
    await expect(next.close()).rejects.toThrow("ownership retained");
    expect(unknown.some(f => f.action === "terminal")).toBeFalse();
    loseReply = false;
    for (const mode of ["task", "malformed", "rpc_error"] as const) {
      reply = mode;
      const outcomes: import("../src/host/managed-effects.ts").Lifecycle[] = [];
      const variant = new McpAccess(settings, "assistant", "demo", process.cwd(), send, async event => { outcomes.push(event); });
      await variant.refresh();
      if (mode === "task") await variant.call("docs", "lookup", {});
      else await expect(variant.call("docs", "lookup", {})).rejects.toThrow();
      if (mode === "rpc_error") { await variant.close(); expect(outcomes.filter(f => f.action === "terminal")).toHaveLength(1); }
      else {
        await expect(variant.close()).rejects.toThrow("ownership retained");
        expect(outcomes.some(f => f.action === "terminal")).toBeFalse();
        const retained = outcomes.find(event => event.action === "rpc_unknown");
        expect(retained?.action === "rpc_unknown" && retained.requests.some(request => request.method === "tools/call" && request.state === "unknown")).toBeTrue();
        if (mode === "task") expect(retained?.action === "rpc_unknown" && retained.requests.find(request => request.method === "tools/call")?.remote_task_id).toBe("async-1");
      }
    }
  } finally { release.resolve(); await peer.close(); }
});

test("MCP HTTP origins cannot borrow provider routes or follow redirects", async () => {
  // Given a purpose-specific configured local origin and a wire that records requests.
  const requests: string[] = [];
  const network = guardedFetch(async () => new Response("provider"), undefined, "openai-codex", undefined, undefined, {
    origins: ["http://127.0.0.1:9876"],
    send: async (input, init) => { requests.push(String(input)); expect(init?.redirect).toBe("error"); return new Response("mcp"); },
  });
  const send = network.mcp;
  if (!send) throw new Error("MCP transport missing");
  // When the MCP transport reaches its origin and attempts unrelated/credential-bearing routes.
  expect(await (await send("http://127.0.0.1:9876/mcp", { method: "POST" })).text()).toBe("mcp");
  for (const url of ["https://chatgpt.com/backend-api/codex/responses", "http://127.0.0.1:9877/mcp"]) {
    await expect(send(url, { method: "POST" })).rejects.toThrow("network_refused");
  }
  await expect(send("http://127.0.0.1:9876/mcp", { headers: { authorization: "provider-token" } })).rejects.toThrow("network_refused");
  await expect(network.fetch("http://127.0.0.1:9876/mcp", { method: "POST" })).rejects.toThrow("network_refused");
  // Then only the approved request reached the wire.
  expect(requests).toEqual(["http://127.0.0.1:9876/mcp"]);
});

test("workspace MCP schema and role ceiling exclude invalid transports and write proxies", () => {
  // Given the same policy for every role, without trusting server annotations.
  const server = McpSettingsSchema.parse({ servers: { docs: {
    projects: ["demo"], command: "fixture", tools: { lookup: "read", write_file: "write", shell: "read", corral_done: "write" },
    resources: ["fixture://public/*"],
  } } }).servers.docs;
  if (!server) throw new Error("missing fixture server");
  // When evaluating each role, then only worker gets configured writes/shell, never Corral proxy.
  expect(toolAllowed(server, "worker", "write_file")).toBeTrue();
  expect(toolAllowed(server, "worker", "shell")).toBeTrue();
  for (const role of ["worker", "reviewer", "pm", "planner", "plan_reviewer"]) {
    expect(toolAllowed(server, role, "corral_done")).toBeFalse();
    expect(toolAllowed(server, role, "lookup")).toBeTrue();
    if (role !== "worker") {
      expect(toolAllowed(server, role, "write_file")).toBeFalse();
      expect(toolAllowed(server, role, "shell")).toBeFalse();
    }
  }
  expect(resourceAllowed(server, "fixture://private/secret")).toBeFalse();
  for (const invalid of [
    { projects: [], command: "fixture" }, { projects: ["demo"] },
    { projects: ["demo"], command: "fixture", url: "http://localhost/" },
    { projects: ["demo"], url: "https://user:pass@example.org/" },
    { projects: ["demo"], url: "ftp://example.org/" },
    { projects: ["demo"], url: "http://localhost/", args: ["forbidden"] },
    { projects: ["demo"], command: "fixture", unexpected: true },
  ]) expect(McpSettingsSchema.safeParse({ servers: { docs: invalid } }).success).toBeFalse();
});
