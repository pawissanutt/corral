// Only generation-bound Corral workspace settings enter this adapter. Pi's ambient
// MCP extension stays disabled; this client never reads .pi or another auth store.
import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { McpClient, StreamableHttpTransport, toLlmContent, type Tool, type McpTransport } from "@earendil-works/pi-mcp";
import { Type } from "typebox";
import { z } from "zod";

import type { Fetch } from "../auth/types.ts";
import {
  McpDenied, resourceAllowed, serverVisible, toolAllowed, type McpServer, type McpSettings,
} from "../host/mcp-settings.ts";
import { mcpControlTools } from "../host/mcp-tools.ts";
import { sha256Hex } from "../tools/corral.ts";
import { ManagedInstance, OwnedStdio, type EvidenceSink } from "../host/managed-effects.ts";
import { ManagedRpc } from "../host/managed-rpc.ts";

type OwnedConnection = {
  readonly name: string; readonly owner: ManagedInstance; readonly client: McpClient; readonly local: boolean;
  readonly pending: Set<Promise<unknown>>; readonly sends: Set<Promise<void>>; readonly rpc: ManagedRpc;
  closing?: Promise<void>;
};

const SchemaObject = z.record(z.string(), z.unknown());

function canonicalSchema(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalSchema);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
    .map(([key, item]) => [key, canonicalSchema(item)]));
}

type Connection = {
  readonly settings: McpServer;
  client?: McpClient;
  connecting?: Promise<McpClient>;
  tools: readonly Tool[];
  current?: OwnedConnection;
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
  private closeResult: Promise<void> | undefined;
  private readonly owned: OwnedConnection[] = [];

  constructor(
    settings: McpSettings, private readonly role: string, project: string,
    private readonly cwd: string, private readonly send?: Fetch,
    private readonly evidence?: EvidenceSink,
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
    if (server.current) await this.stop(server.current);
    if (this.stopped) throw new McpDenied("the MCP session is closed");
    // Recheck after the stop await so concurrent reconnects still share one instance.
    if (server.connecting) return server.connecting;
    server.connecting = this.open(name, server);
    try { return await server.connecting; }
    finally { server.connecting = undefined; }
  }

  private async open(name: string, server: Connection): Promise<McpClient> {
    const { settings } = server;
    if (this.owned.length >= 16) throw new McpDenied("retained executor identity limit; inspect lifecycle before connecting another instance");
    const owner = new ManagedInstance(`mcp:${name}`, { name, settings, cwd: this.cwd }, this.evidence);
    await owner.prepare();
    const client = new McpClient({ name: "corral-pi", version: "1", requestTimeoutMs: 15_000 });
    const owned: OwnedConnection = { name, owner, client, local: !!settings.command, pending: new Set(), sends: new Set(), rpc: new ManagedRpc() };
    this.owned.push(owned); server.current = owned; server.client = client;
    if (!settings.command) await owner.bindHttp();
    const transport: McpTransport = settings.command
      ? new OwnedStdio({ command: settings.command, args: settings.args, cwd: this.cwd, maxMessageBytes: 1_048_576 }, owner)
      : new StreamableHttpTransport({
        url: settings.url ?? "", maxMessageBytes: 1_048_576,
        fetch: this.send ?? (() => { throw new McpDenied("no generation-bound HTTP MCP transport"); }),
      });
    try {
      const tracked: McpTransport = {
        start: () => transport.start(), close: () => transport.close(),
        onError: listener => transport.onError(listener), onClose: listener => transport.onClose(listener),
        onMessage: listener => transport.onMessage(message => {
          owned.rpc.received(message);
          listener(message);
        }),
        send: (message, options) => {
          const id = owned.rpc.prepare(message);
          const pending = (async () => {
            try {
              if (id !== undefined && "method" in message) await owner.rpcPrepared(id, message.method);
              if (this.stopped || owned.closing || options?.signal?.aborted) throw new McpDenied("MCP stopped before protocol dispatch");
            } catch (error) { if (id !== undefined) owned.rpc.notStarted(id); throw owned.rpc.fence(error); }
            if (id !== undefined) owned.rpc.dispatch(id);
            await transport.send(message, options);
          })();
          owned.sends.add(pending);
          void pending.then(() => owned.sends.delete(pending), () => owned.sends.delete(pending));
          return pending;
        },
        ...(transport instanceof StreamableHttpTransport ? { setProtocolVersion: (version: string) => transport.setProtocolVersion(version) } : {}),
      };
      await this.observe(owned, () => client.connect(tracked));
      if (this.stopped) throw new McpDenied("the MCP session closed during connection");
      server.tools = client.serverCapabilities?.tools ? await this.observe(owned, () => client.listTools()) : [];
      server.error = undefined;
      this.updated?.();
      return client;
    } catch (error) {
      // Attempt actual cleanup, but retain failed handles and unknown remote outcomes.
      try { await this.stop(owned); } catch (shutdown) { server.error = String(shutdown); }
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
      if (server.current) await this.stop(server.current);
      if (this.stopped) throw new McpDenied("the MCP session is closed");
      server.client = undefined; server.current = undefined;
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
    const owned = server.current;
    if (!owned || owned.client !== client) throw new McpDenied("MCP connection changed before dispatch");
    return this.observe(owned, () => client.callTool(tool, args, { signal }));
  }

  async resources(serverName: string, signal?: AbortSignal) {
    const server = this.server(serverName);
    if (!server.settings.resources.length) throw new McpDenied(`resources on ${serverName} are not allowed`);
    const client = await this.connection(serverName);
    if (!client.serverCapabilities?.resources) return [];
    const owned = server.current;
    if (!owned || owned.client !== client) throw new McpDenied("MCP connection changed before dispatch");
    return (await this.observe(owned, () => client.listResources({ signal }))).filter((resource) => resourceAllowed(server.settings, resource.uri));
  }

  async readResource(serverName: string, uri: string, signal?: AbortSignal) {
    const server = this.server(serverName);
    if (!resourceAllowed(server.settings, uri)) throw new McpDenied(`resource ${serverName}/${uri} is not allowed`);
    const client = await this.connection(serverName), owned = server.current;
    if (!owned || owned.client !== client) throw new McpDenied("MCP connection changed before dispatch");
    return this.observe(owned, () => client.readResource(uri, { signal }));
  }

  definitions(deferred: boolean): ToolDefinition[] {
    const access = this;
    return [...this.servers].flatMap(([name, server]) => server.tools
      .filter((tool) => toolAllowed(server.settings, this.role, tool.name))
      .map((tool): ToolDefinition => ({
        name: mcpToolName(name, tool.name), label: `${name}: ${tool.title ?? tool.name}`,
        description: tool.description ?? `${name}/${tool.name}`,
        // Pi compares declarations by JSON serialization. Equivalent remote key ordering must
        // not publish a new schema; preserve arrays and all actual semantic changes.
        parameters: Type.Unsafe<Record<string, unknown>>(SchemaObject.parse(canonicalSchema({
          ...tool.inputSchema, type: "object", properties: tool.inputSchema.properties ?? {},
        }))),
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
        ...(server.current ? { executor: { ...server.current.owner.snapshot(),
          unknown_remote: !server.current.local && !server.current.rpc.settled,
          requests: server.current.rpc.outcomes() } } : {}),
      })),
    };
  }

  private async observe<T>(owned: OwnedConnection, action: () => Promise<T>): Promise<T> {
    if (owned.closing) throw new McpDenied("MCP connection stopped before dispatch");
    const pending = owned.rpc.observe(action); owned.pending.add(pending);
    try { return await pending; }
    finally { owned.pending.delete(pending); }
  }

  private stop(owned: OwnedConnection): Promise<void> {
    return owned.closing ??= (async () => {
      await owned.client.close();
      await Promise.allSettled([...owned.pending, ...owned.sends]);
      if (!owned.local && !owned.rpc.settled) {
        await owned.owner.rpcUnknown("remote request unanswered, invalid, contradictory or unsupported async task; connection close is not execution shutdown", owned.rpc.outcomes());
        throw new Error("MCP remote request outcome unknown; connection close is not execution shutdown");
      }
      await owned.owner.stopped(owned.rpc.unknown ? "local executor terminated; earlier request outcome unknown"
        : "owned transport closed; every observed request terminal (not necessarily successful)", owned.rpc.outcomes());
    })();
  }

  close(): Promise<void> {
    this.stopped = true;
    this.updated = undefined;
    return this.closeResult ??= (async () => {
      await Promise.allSettled([...this.servers.values()].flatMap(server => server.connecting ? [server.connecting] : []));
      const results = await Promise.allSettled(this.owned.map(async owned => {
        try { await this.stop(owned); }
        catch (error) { this.server(owned.name).error = String(error); throw error; }
      }));
      const errors = results.flatMap(result => result.status === "rejected" ? [result.reason] : []);
      if (errors.length) throw new AggregateError(errors, "MCP shutdown unknown; ownership retained");
    })();
  }
}
