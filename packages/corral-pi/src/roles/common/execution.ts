import type { ToolDefinition } from "@earendil-works/pi-coding-agent";

// The same loadout gates direct and nested tool calls. Code Mode never calls itself.
export function executionRefusal(
  tool: string, nested: boolean, allowed: ReadonlySet<string>,
): string | undefined {
  if (!allowed.has(tool)) return `${tool} is not in this session's loadout`;
  if (nested && tool === "codemode") return "codemode cannot run inside another tool";
  return undefined;
}

const CHILD_READS = new Set(["read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_read", "web_search", "codemode"]);
/** Parent intersection with a fixed child ceiling: no shell, MCP, hooks or recursive children. */
export function exploreLoadout(parent: readonly string[]): string[] {
  return parent.filter((name) => CHILD_READS.has(name));
}

export type ChildHostedTools = (child: {
  kind: "slice" | "review" | "explore";
  owns: readonly string[];
}) => readonly ToolDefinition[];

/** Hosted tools are supplied afresh for the child, never as generic local tools. */
export function childToolBindings(parent: readonly string[], inherited: readonly ToolDefinition[], supplied: readonly ToolDefinition[]) {
  const replaceSearch = supplied.some(tool => tool.name === "web_search");
  const local = inherited.filter(({ name }) => name !== "image_gen" && !(name === "web_search" && replaceSearch));
  const hosted = supplied.filter(({ name }) => (name === "image_gen" || name === "web_search") && parent.includes(name));
  return { local, hosted, loadout(base: readonly string[]) {
    return [...new Set([...base.filter(name => name !== "web_search" || local.some(tool => tool.name === name)),
      ...hosted.map(tool => tool.name)])];
  } };
}
