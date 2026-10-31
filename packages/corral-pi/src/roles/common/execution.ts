// One final execution predicate for the bound session and auxiliary sessions. A loadout is
// explicit, not inferred from a tool's read-only hint. Corral operations are top-level only.
export function executionRefusal(
  tool: string, nested: boolean, allowed: ReadonlySet<string>, corral: ReadonlySet<string>,
): string | undefined {
  if (!allowed.has(tool)) return `${tool} is not in this session's loadout`;
  if (nested && corral.has(tool)) return `${tool} is a corral operation; call it directly, not from another tool`;
  return undefined;
}

const CHILD_READS = new Set(["read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_search"]);
/** Parent intersection with a fixed child ceiling: no shell, MCP, hooks or recursive children. */
export function exploreLoadout(parent: readonly string[]): string[] {
  return parent.filter((name) => CHILD_READS.has(name));
}
