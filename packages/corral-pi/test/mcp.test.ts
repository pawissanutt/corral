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
