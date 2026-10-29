import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { McpAccess } from "../capabilities/mcp.ts";
import { McpDenied } from "./mcp-settings.ts";

export const MCP_CONTROLS = ["mcp_connect", "mcp_resources", "mcp_read_resource"] as const;

export function mcpControlTools(access: McpAccess, pi: ExtensionAPI, deferred: boolean): ToolDefinition[] {
  const result = (value: unknown) => ({ content: [{ type: "text" as const, text: JSON.stringify(value) }], details: undefined });
  return [
    {
      name: "mcp_connect", label: "Connect MCP", description: "Refresh configured project MCP servers and their approved tool schemas. "
        + "No servers means MCP is unconfigured; configure agent.capabilities.<role>.mcp in the Corral workspace.",
      parameters: Type.Object({ server: Type.Optional(Type.String()) }),
      async execute(_id, params: { server?: string }) { return result(await access.refresh(params.server)); },
    },
    {
      name: "mcp_resources", label: "List MCP resources", description: "List resources within the configured server/URI policy.",
      parameters: Type.Object({ server: Type.String() }),
      async execute(_id, params: { server: string }, signal) { return result(await access.resources(params.server, signal)); },
    },
    {
      name: "mcp_read_resource", label: "Read MCP resource", description: "Read a resource allowed by the configured server/URI policy.",
      parameters: Type.Object({ server: Type.String(), uri: Type.String() }),
      async execute(_id, params: { server: string; uri: string }, signal) {
        return result(await access.readResource(params.server, params.uri, signal));
      },
    },
    ...(deferred ? [{
      name: "mcp_activate", label: "Activate MCP tool",
      description: "Refresh a configured server, then activate approved tool names returned by tool_search. Does not enable codemode.",
      parameters: Type.Object({ server: Type.String(), tools: Type.Array(Type.String(), { minItems: 1 }) }),
      async execute(_id: string, params: { server: string; tools: string[] }) {
        await access.refresh(params.server);
        for (const name of params.tools) if (!access.permits(name)) throw new McpDenied(`activation of ${name} is not allowed`);
        pi.setActiveTools([...new Set([...pi.getActiveTools(), ...params.tools])]);
        return result({ activated: params.tools });
      },
    }] : []),
  ];
}
