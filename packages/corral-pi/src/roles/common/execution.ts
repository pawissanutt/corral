// Explicit receipt-free board ceiling, not a read-only hint or a name-prefix heuristic.
// Rust ops/pm_tests.rs checks every member creates no stored receipt/request-ID obligation.
import type { CorralOp } from "../types.ts";

const NESTED_CORRAL_READ_OPS: ReadonlySet<CorralOp> = new Set([
  "read_brief", "read_next", "read_show", "read_list", "read_search", "read_route",
  "read_artifacts", "read_lessons", "inbox_read",
]);

export function nestedCorralRead(op: CorralOp): boolean {
  return NESTED_CORRAL_READ_OPS.has(op);
}

// One final execution predicate for bound and auxiliary sessions. A loadout is explicit;
// receipt-bearing operations need a top-level transcript call for replay and recovery.
export function executionRefusal(
  tool: string, nested: boolean, allowed: ReadonlySet<string>, corral: ReadonlySet<string>,
  reads: ReadonlySet<string> = new Set(),
): string | undefined {
  if (!allowed.has(tool)) return `${tool} is not in this session's loadout`;
  if (nested && corral.has(tool) && !reads.has(tool)) {
    return `${tool} requires a top-level transcript call for receipt replay/recovery; call it directly, not from another tool`;
  }
  return undefined;
}

const CHILD_READS = new Set(["read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_search"]);
/** Parent intersection with a fixed child ceiling: no shell, MCP, hooks or recursive children. */
export function exploreLoadout(parent: readonly string[]): string[] {
  return parent.filter((name) => CHILD_READS.has(name));
}
