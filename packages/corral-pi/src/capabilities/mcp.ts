// Only generation-bound Corral workspace settings enter this adapter. Pi's ambient
// MCP extension stays disabled; this client never reads .pi or another auth store.
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { McpClient, StdioTransport, StreamableHttpTransport, toLlmContent, type Tool } from "@earendil-works/pi-mcp";
import { Type } from "typebox";

import type { Fetch } from "../auth/types.ts";
import {
  McpDenied, resourceAllowed, serverVisible, toolAllowed, type McpServer, type McpSettings,
} from "../host/mcp-settings.ts";
import { mcpControlTools } from "../host/mcp-tools.ts";
import { sha256Hex } from "../tools/corral.ts";

type Connection = {
  readonly settings: McpServer;
  client?: McpClient;
  connecting?: Promise<McpClient>;
  tools: readonly Tool[];
  error?: string;
};

export function mcpToolName(server: string, tool: string): string {
  // Keep provider names bounded and unambiguous even when server/tool names normalize alike.
  return `mcp_${server.slice(0, 16)}_${tool.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 24)}_${sha256Hex(
    `${server}\0${tool}`,
  ).slice(0, 12)}`;
}

export class McpAccess {
  private readonly servers = new Map<string, Connection>();
  private updated: (() => void) | undefined;
  private stopped = false;

  constructor(
    settings: McpSettings, private readonly role: string, project: string,
    private readonly cwd: string, private readonly send?: Fetch,
  ) {
    for (const [name, server] of Object.entries(settings.servers)) {
      if (serverVisible(server, role, project)) this.servers.set(name, { settings: server, tools: [] });
    }
  }

  private server(name: string): Connection {
    const server = this.servers.get(name);
    if (!server) throw new McpDenied(`server ${name} is not enabled for this project`);
    return server;
  }

  private async connection(name: string): Promise<McpClient> {
    if (this.stopped) throw new McpDenied("the MCP session is closed");
    const server = this.server(name);
    if (server.connecting) return server.connecting;
    if (server.client?.connectionState === "connected") return server.client;
    server.connecting = this.open(name, server);
    try { return await server.connecting; }
    finally { server.connecting = undefined; }
  }

  private async open(name: string, server: Connection): Promise<McpClient> {
    const { settings } = server;
    const client = new McpClient({ name: "corral-pi", version: "1", requestTimeoutMs: 15_000 });
    server.client = client;
    const transport = settings.command
      ? new StdioTransport({ command: settings.command, args: settings.args, cwd: this.cwd, maxMessageBytes: 1_048_576 })
      : new StreamableHttpTransport({
        url: settings.url ?? "", maxMessageBytes: 1_048_576,
        fetch: this.send ?? (() => { throw new McpDenied("no generation-bound HTTP MCP transport"); }),
      });
    try {
      await client.connect(transport);
      if (this.stopped) throw new McpDenied("the MCP session closed during connection");
      server.tools = client.serverCapabilities?.tools ? await client.listTools() : [];
      server.error = undefined;
      this.updated?.();
      return client;
    } catch (error) {
      await client.close();
      server.tools = [];
      server.error = error instanceof Error ? error.message : String(error);
      throw error;
    }
  }

  async refresh(name?: string): Promise<Record<string, unknown>> {
    const names = name === undefined ? [...this.servers.keys()] : [name];
    if (name !== undefined) this.server(name);
    for (const key of names) {
      const server = this.server(key);
      if (server.connecting) await server.connecting;
      await server.client?.close();
      server.client = undefined;
      // A configured but unavailable server is useful state, not a failed host launch.
      try { await this.connection(key); }
      catch (error) {
        if (!(error instanceof Error)) throw error;
        server.error = error.message;
      }
    }
    this.updated?.();
    return this.state();
  }

  names(): string[] {
    return [...this.servers].flatMap(([name, server]) => server.tools
      .filter((tool) => toolAllowed(server.settings, this.role, tool.name))
      .map((tool) => mcpToolName(name, tool.name)));
  }

  authorizedNames(): string[] {
    return [...this.servers].flatMap(([name, server]) => Object.keys(server.settings.tools)
      .filter((tool) => toolAllowed(server.settings, this.role, tool))
      .map((tool) => mcpToolName(name, tool)));
  }

  permits(name: string): boolean { return this.names().includes(name); }

  async call(serverName: string, tool: string, args: Record<string, unknown>, signal?: AbortSignal) {
    const server = this.server(serverName);
    if (!toolAllowed(server.settings, this.role, tool)) throw new McpDenied(`tool ${serverName}/${tool} is not allowed`);
    const client = await this.connection(serverName);
    if (!server.tools.some((entry) => entry.name === tool)) throw new McpDenied(`tool ${serverName}/${tool} is not advertised`);
    return client.callTool(tool, args, { signal });
  }

  async resources(serverName: string, signal?: AbortSignal) {
    const server = this.server(serverName);
    if (!server.settings.resources.length) throw new McpDenied(`resources on ${serverName} are not allowed`);
    const client = await this.connection(serverName);
    if (!client.serverCapabilities?.resources) return [];
    return (await client.listResources({ signal })).filter((resource) => resourceAllowed(server.settings, resource.uri));
  }

  async readResource(serverName: string, uri: string, signal?: AbortSignal) {
    const server = this.server(serverName);
    if (!resourceAllowed(server.settings, uri)) throw new McpDenied(`resource ${serverName}/${uri} is not allowed`);
    return (await this.connection(serverName)).readResource(uri, { signal });
  }

  definitions(deferred: boolean): ToolDefinition[] {
    const access = this;
    return [...this.servers].flatMap(([name, server]) => server.tools
      .filter((tool) => toolAllowed(server.settings, this.role, tool.name))
      .map((tool): ToolDefinition => ({
        name: mcpToolName(name, tool.name), label: `${name}: ${tool.title ?? tool.name}`,
        description: tool.description ?? `${name}/${tool.name}`,
        parameters: Type.Unsafe<Record<string, unknown>>({
          ...tool.inputSchema, type: "object", properties: tool.inputSchema.properties ?? {},
        }),
        exposure: deferred ? "deferred" : "direct",
        async execute(_id, params: Record<string, unknown>, signal) {
          const result = await access.call(name, tool.name, params, signal);
          if (result.isError) throw new McpDenied(JSON.stringify(result));
          return { content: toLlmContent(result), details: result.structuredContent };
        },
      })));
  }

  attach(pi: ExtensionAPI, deferred: boolean): void {
    const register = () => { for (const tool of this.definitions(deferred)) pi.registerTool(tool); };
    register();
    this.updated = () => {
      register();
      pi.setActiveTools(pi.getActiveTools().filter((name) => !name.startsWith("mcp_") ||
        name === "mcp_connect" || name === "mcp_activate" || name === "mcp_resources" ||
        name === "mcp_read_resource" || this.permits(name)));
    };
    for (const tool of mcpControlTools(this, pi, deferred)) pi.registerTool(tool);
  }

  state(): Record<string, unknown> {
    return {
      configured: this.servers.size > 0,
      servers: [...this.servers].map(([name, server]) => ({
        name, connected: server.client?.connectionState === "connected",
        tools: server.tools.filter((tool) => toolAllowed(server.settings, this.role, tool.name)).map((tool) => tool.name),
        ...(server.error ? { error: server.error } : {}),
      })),
    };
  }

  async close(): Promise<void> {
    this.stopped = true;
    this.updated = undefined;
    await Promise.all([...this.servers.values()].map(async (server) => {
      await server.client?.close();
      if (server.connecting) await server.connecting.catch((error: unknown) => {
        if (!(error instanceof Error)) throw error;
        server.error = error.message;
      });
    }));
  }
}
