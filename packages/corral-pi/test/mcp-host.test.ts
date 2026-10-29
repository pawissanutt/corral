import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { mcpToolName } from "../src/capabilities/mcp.ts";
import { EventLog } from "../src/bridge/events.ts";
import { McpSettingsSchema } from "../src/host/mcp-settings.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment, BUILTIN_EXTENSIONS } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { mcpServer } from "./support/mcp-server.ts";
import { nextEvent } from "./support/wait.ts";

test("workspace MCP policy holds directly, deferred and nested across reconnect and reload without codemode", async () => {
  // Given a real MCP peer, private worktrees and real host sessions under three role bindings.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-mcp-host-"));
  const raw = globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  try {
    restrictEnvironment(process.env);
    for (const role of ["worker", "reviewer", "pm"] as const) {
      for (const search of [false, true]) {
        const peer = await mcpServer();
        const cwd = join(work, `${role}-${search}`);
        const agentDir = join(cwd, "agent");
        mkdirSync(join(cwd, ".pi"), { recursive: true });
        mkdirSync(agentDir);
        const ambient = JSON.stringify({ mcpServers: { ambient: { url: peer.url } } });
        writeFileSync(join(cwd, ".pi/mcp.json"), ambient);
        writeFileSync(join(agentDir, "mcp.json"), ambient);
        const settings = McpSettingsSchema.parse({ servers: {
          docs: { projects: ["demo"], url: peer.url, tools: { lookup: "read" }, resources: ["fixture://public/*"] },
          foreign: { projects: ["another-project"], url: peer.url, tools: { write_file: "write" } },
        } });
        const capabilities: CapabilitySet = {
          catalog: 3, digest: "a".repeat(64), preset: "standard",
          enabled: ["mcp.enabled", ...(search ? ["tool_search"] : [])], mcp: settings,
        };
        const modules = await loadModules(assets.dir, role);
        const transport = modules.auth.transport?.(authContext());
        if (!transport) throw new Error("missing fake provider transport");
        const network = guardedFetch(transport, undefined, "openai-codex", undefined, undefined, {
          origins: [new URL(peer.url).origin], send: raw.bind(globalThis),
        });
        globalThis.fetch = network.fetch;
        const identity = { store: "st-demo", project: "demo", role, generation: 1, task: "T-567", run: "R1" };
        const events = new EventLog(1);
        const corral = writeFakeCorral(cwd);
        let host: HostSession | undefined;
        const client = new CorralClient({
          corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
        });
        const nested: ToolDefinition = {
          name: "fixture_mcp", label: "Nested MCP", description: "Exercise the shared nested execution check",
          parameters: Type.Object({ name: Type.String(), args: Type.Unsafe<Record<string, unknown>>({ type: "object" }) }),
          async execute(_id, params: { name: string; args: Record<string, unknown> }, signal, _update, ctx) {
            const outcome = await ctx.executeTool(params.name, params.args, { signal });
            return { content: [{ type: "text", text: JSON.stringify({ isError: outcome.isError }) }], details: undefined };
          },
        };
        try {
          host = await HostSession.create({
            sdk, identity, cwd, agentDir, model: "gpt-6.1-sol", capabilities, client, events, network,
            modules: { ...modules, role: {
              ...modules.role, localToolNames: [nested.name], localTools: () => [nested],
            } },
            sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
          });
          await host.session.bindExtensions({});
          await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
          const lookup = mcpToolName("docs", "lookup");
          const write = mcpToolName("docs", "write_file");
          expect(peer.calls.filter(({ method }) => method === "initialize")).toHaveLength(1);
          expect(host.session.getActiveToolNames().includes(lookup)).toBe(!search);
          expect(host.loadout.includes("tool_search")).toBe(search);
          expect(host.loadout).not.toContain("codemode");
          for (const builtin of ["mcp", "codemode", "tool-search"]) expect(BUILTIN_EXTENSIONS).toContain(builtin);
          if (search) {
            const metadata = host.session.getAllTools()
              .filter((tool) => ["mcp_connect", "mcp_activate", "mcp_resources", "mcp_read_resource", "tool_search"].includes(tool.name))
              .map(({ name, description, parameters }) => ({ name, description, parameters }));
            const bytes = Buffer.byteLength(JSON.stringify(metadata));
            console.log(`MCP_STATIC_COST ${JSON.stringify({
              estimator: "utf8-json-bytes/4-v1", role, default_empty_server_tools: metadata.length,
              tool_bytes_delta: bytes, tool_tokens_estimate_delta: Math.ceil(bytes / 4), prompt_bytes_delta: 0,
            })}`);
          }
          for (const stage of ["startup", "reconnect", "reload"]) {
            if (stage === "reconnect") peer.revise();
            if (stage === "reload") await host.session.reload();
            const args = { key: "note", ...(stage === "startup" ? {} : { version: 2 }) };
            // When model calls use direct/deferred/nested paths and request forbidden identities.
            const steps = [
              ...(stage === "reconnect" ? [{ toolCalls: [{ callId: `${stage}-connect`, name: "mcp_connect", arguments: { server: "docs" } }] }] : []),
              ...(search ? [
                { toolCalls: [{ callId: `${stage}-search`, name: "tool_search", arguments: { query: "lookup" } }] },
                { toolCalls: [{ callId: `${stage}-activate`, name: "mcp_activate", arguments: { server: "docs", tools: [lookup] } }] },
                { toolCalls: [{ callId: `${stage}-deny-activate`, name: "mcp_activate", arguments: { server: "docs", tools: [write] } }] },
              ] : []),
              { toolCalls: [{ callId: `${stage}-read`, name: lookup, arguments: args }] },
              { toolCalls: [{ callId: `${stage}-nested`, name: nested.name, arguments: { name: lookup, args } }] },
              { toolCalls: [{ callId: `${stage}-denied`, name: write, arguments: {} }] },
              { toolCalls: [{ callId: `${stage}-nested-denied`, name: nested.name, arguments: { name: write, args: {} } }] },
              { toolCalls: [{ callId: `${stage}-resources`, name: "mcp_resources", arguments: { server: "docs" } }] },
              { toolCalls: [{ callId: `${stage}-private`, name: "mcp_read_resource", arguments: { server: "docs", uri: "fixture://private/secret" } }] },
              { toolCalls: [{ callId: `${stage}-ambient`, name: "mcp_connect", arguments: { server: "ambient" } }] },
              { text: "Finished" },
            ];
            fixtureTransport().script(steps);
            const ended = nextEvent(events, (event) => event.kind === "turn_ended");
            expect((await host.submit({ text: stage, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" },
              `rq-${stage}`)).disposition).toBe("started");
            await ended;
            await host.drained();
            // Then only approved reads reached the wire, resources were filtered and search stayed cached.
            const result = (suffix: string) => host?.session.messages.find((message) =>
              message.role === "toolResult" && message.toolCallId.split("|")[0] === `${stage}-${suffix}`);
            for (const suffix of ["read", "nested", "resources"]) {
              const message = result(suffix);
              expect(message?.role).toBe("toolResult");
              const isError = message?.role === "toolResult" ? message.isError : undefined;
              expect({ role, search, stage, suffix, isError, details: isError ? message : undefined })
                .toEqual({ role, search, stage, suffix, isError: false, details: undefined });
            }
            for (const suffix of ["denied", "private", "ambient", ...(search ? ["deny-activate"] : [])]) {
              const message = result(suffix);
              const isError = message?.role === "toolResult" && message.isError;
              expect({ role, search, stage, suffix, isError, details: isError ? undefined : message })
                .toEqual({ role, search, stage, suffix, isError: true, details: undefined });
            }
            const denied = result("nested-denied");
            expect(denied?.role === "toolResult" && denied.content).toEqual([{ type: "text", text: '{"isError":true}' }]);
            if (search) {
              const message = result("search");
              const content = message?.role === "toolResult" ? message.content[0] : undefined;
              const matches = JSON.parse(content?.type === "text" ? content.text : "null");
              const names = matches.tools.map((tool: { name: string }) => tool.name);
              expect(names).toContain(lookup);
              expect(names).not.toContain(write);
              expect(names).not.toContain("fixture_forbidden");
            }
            expect(peer.calls.filter(({ method, params }) => method === "tools/call" && params.name !== "lookup")).toEqual([]);
            expect(host.loadout).not.toContain(write);
            expect(host.loadout).not.toContain("codemode");
          }
          expect(peer.calls.filter(({ method }) => method === "tools/call")).toHaveLength(6);
          expect(host.state().capabilities).toEqual(capabilities);
        } finally { await host?.close(); await peer.close(); }
        globalThis.fetch = raw;
      }
    }
  } finally { globalThis.fetch = raw; assets.cleanup(); rmSync(work, { recursive: true, force: true }); }
}, 120_000);
