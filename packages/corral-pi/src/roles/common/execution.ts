// Explicit receipt-free board reads, not a read-only hint or a name-prefix heuristic: their
// failures have no effect to reconcile. Rust ops/pm_tests.rs checks every member creates no stored
// receipt/request-ID obligation.
import type { CorralOp } from "../types.ts";

const NESTED_CORRAL_READ_OPS: ReadonlySet<CorralOp> = new Set([
  "read_brief", "read_next", "read_show", "read_list", "read_search", "read_route",
  "read_artifacts", "read_lessons", "inbox_read",
]);

export function nestedCorralRead(op: CorralOp): boolean {
  return NESTED_CORRAL_READ_OPS.has(op);
}

// One final execution predicate for bound and auxiliary sessions, for direct and nested calls
// alike: a loadout is explicit, and a tool usable directly is usable from Code Mode (T-683), where
// each receipted child carries its own parent-bound identity and journal for replay and recovery.
// `unjournaled` marks a receipted call nested by anything else, which has neither. Code Mode never
// calls itself.
export function executionRefusal(
  tool: string, nested: boolean, allowed: ReadonlySet<string>, unjournaled = false,
): string | undefined {
  if (!allowed.has(tool)) return `${tool} is not in this session's loadout`;
  if (nested && tool === "codemode") return "codemode cannot run inside another tool";
  if (nested && unjournaled) {
    return `${tool} is receipted: call it directly or from a codemode cell, which journals it for replay and recovery`;
  }
  return undefined;
}

const CHILD_READS = new Set(["read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_read", "web_search"]);
/** Parent intersection with a fixed child ceiling: no shell, MCP, hooks or recursive children. */
export function exploreLoadout(parent: readonly string[]): string[] {
  return parent.filter((name) => CHILD_READS.has(name));
}
