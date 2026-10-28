// Pi-only tools (corral_*, lsp, ...) for Claude Code's planner (T-493), ported from senpi's
// custom-tools.js and custom-tools-schema.js (MIT). The MCP handler is deliberately unusable: the
// host receives the streamed tool call and executes it through Pi.
import type { McpSdkServerConfigWithInstance } from "@anthropic-ai/claude-agent-sdk";
import type { Tool } from "@earendil-works/pi-ai";
import { z } from "zod";

import { loadSdk } from "./sdk.ts";
import { CUSTOM_TOOLS_MCP_SERVER_NAME, TOOL_EXECUTION_DENIED_MESSAGE } from "./tools.ts";

interface JsonSchema {
  type?: string | string[];
  const?: unknown;
  enum?: unknown[];
  anyOf?: JsonSchema[];
  oneOf?: JsonSchema[];
  items?: JsonSchema;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  description?: unknown;
}

type Literal = string | number | boolean;

function union(types: z.ZodType[]): z.ZodType {
  const [first, second, ...rest] = types;
  if (!first) return z.unknown();
  if (!second) return first;
  return z.union([first, second, ...rest]);
}

function schemaToZod(schema: JsonSchema): z.ZodType {
  let converted: z.ZodType | undefined;
  if (schema.const !== undefined) {
    converted = schema.const === null ? z.null() : z.literal(schema.const as Literal);
  } else if (schema.enum && schema.enum.length > 0) {
    const values = schema.enum.filter((value): value is Literal => ["string", "number", "boolean"].includes(typeof value));
    if (values.length > 0) converted = union(values.map((value) => z.literal(value)));
  }
  if (converted === undefined) {
    const variants = schema.anyOf ?? schema.oneOf;
    if (variants && variants.length > 0) {
      converted = union(variants.map(schemaToZod));
    } else {
      switch (Array.isArray(schema.type) ? schema.type[0] : schema.type) {
        case "string": converted = z.string(); break;
        case "number": converted = z.number(); break;
        case "integer": converted = z.number().int(); break;
        case "boolean": converted = z.boolean(); break;
        case "null": converted = z.null(); break;
        case "array": converted = z.array(schema.items ? schemaToZod(schema.items) : z.unknown()); break;
        case "object": converted = z.object(jsonSchemaToZodShape(schema)); break;
        default: converted = z.unknown();
      }
    }
  }
  return typeof schema.description === "string" ? converted.describe(schema.description) : converted;
}

export function jsonSchemaToZodShape(schema: unknown): Record<string, z.ZodType> {
  const object = (schema ?? {}) as JsonSchema;
  const required = new Set(object.required ?? []);
  const shape: Record<string, z.ZodType> = {};
  for (const [key, value] of Object.entries(object.properties ?? {})) {
    const converted = schemaToZod(value);
    shape[key] = required.has(key) ? converted : converted.optional();
  }
  return shape;
}

export async function denyCustomToolExecution() {
  return { content: [{ type: "text" as const, text: TOOL_EXECUTION_DENIED_MESSAGE }], isError: true };
}

export async function buildCustomToolServers(
  tools: readonly Tool[],
): Promise<Record<string, McpSdkServerConfigWithInstance> | undefined> {
  if (tools.length === 0) return undefined;
  const sdk = await loadSdk();
  const server = sdk.createSdkMcpServer({
    name: CUSTOM_TOOLS_MCP_SERVER_NAME,
    version: "1.0.0",
    tools: tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: jsonSchemaToZodShape(tool.parameters),
      handler: denyCustomToolExecution,
    })),
  });
  return { [CUSTOM_TOOLS_MCP_SERVER_NAME]: server };
}
