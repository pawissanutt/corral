// The same loadout gates direct and nested tool calls. Code Mode never calls itself.
export function executionRefusal(
  tool: string, nested: boolean, allowed: ReadonlySet<string>,
): string | undefined {
  if (!allowed.has(tool)) return `${tool} is not in this session's loadout`;
  if (nested && tool === "codemode") return "codemode cannot run inside another tool";
  return undefined;
}

const CHILD_READS = new Set(["read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_read", "web_search"]);
/** Parent intersection with a fixed child ceiling: no shell, MCP, hooks or recursive children. */
export function exploreLoadout(parent: readonly string[]): string[] {
  return parent.filter((name) => CHILD_READS.has(name));
}
