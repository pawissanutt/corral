import { z } from "zod";

const name = z.string().regex(/^[A-Za-z0-9_-]{1,40}$/);
const endpoint = z.string().url().refine((value) => {
  const url = new URL(value);
  return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password && !url.hash;
});

export const McpServerSchema = z.object({
  projects: z.array(z.string().min(1)).min(1),
  command: z.string().min(1).optional(),
  args: z.array(z.string()).default([]),
  url: endpoint.optional(),
  tools: z.record(z.string().min(1), z.enum(["read", "write"])).default({}),
  resources: z.array(z.string().min(1)).default([]),
}).strict().refine((server) =>
  (server.command !== undefined) !== (server.url !== undefined) &&
  (server.command !== undefined || server.args.length === 0),
{ message: "MCP needs exactly one command/args or HTTP(S) URL" });

export const McpSettingsSchema = z.object({
  servers: z.record(name, McpServerSchema),
}).strict();

export type McpSettings = z.infer<typeof McpSettingsSchema>;
export type McpServer = z.infer<typeof McpServerSchema>;

/** A project-bound role sees servers enabled for its project; a workspace-scoped assistant sees all. */
export function serverVisible(server: McpServer, role: string, project: string): boolean {
  return role === "assistant" || server.projects.includes(project);
}

export class McpDenied extends Error {
  readonly code = "mcp_denied";
  constructor(reason: string) { super(`mcp_denied: ${reason}`); }
}

// These generic execution/write proxies cannot become read operations through annotations
// or a mistaken read label. Configured servers remain trusted, not certified read-only code.
const WRITE_PROXY = /(^|[_./-])(bash|shell|exec|execute|run_command|write|edit|delete|remove|create|patch)([_./-]|$)/i;

export function toolAllowed(server: McpServer, role: string, tool: string): boolean {
  if (/(^|[_./-])corral([_./-]|$)/i.test(tool)) return false;
  const operation = server.tools[tool];
  return role === "worker" ? operation === "read" || operation === "write"
    : operation === "read" && !WRITE_PROXY.test(tool);
}

export function resourceAllowed(server: McpServer, uri: string): boolean {
  return server.resources.some((scope) => scope.endsWith("*")
    ? uri.startsWith(scope.slice(0, -1)) : scope === uri);
}
