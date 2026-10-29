import type { ExtensionAPI, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

/** Pure cached metadata: never connects, activates tools, or changes another capability. */
export function toolSearch(pi: Pick<ExtensionAPI, "getAllTools">, permitted: (name: string) => boolean) {
  return {
    name: "tool_search", label: "Search tools",
    description: "Search locally cached permitted tool names, descriptions and schemas. "
      + "Use mcp_activate to activate an MCP result; this search performs no connection or activation.",
    parameters: Type.Object({ query: Type.String({ maxLength: 4096 }), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })) }),
    async execute(_id, params: { query: string; limit?: number }) {
      const words = params.query.toLowerCase().split(/\s+/).filter(Boolean);
      const tools = pi.getAllTools().filter((tool) => permitted(tool.name) &&
        words.every((word) => `${tool.name} ${tool.description}`.toLowerCase().includes(word)))
        .slice(0, params.limit ?? 10)
        .map(({ name, description, parameters }) => ({ name, description, parameters }));
      return { content: [{ type: "text", text: JSON.stringify({ tools }) }], details: undefined };
    },
  } satisfies ToolDefinition;
}
