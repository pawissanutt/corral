import { posix } from "node:path";
import { LIVE_CHILD_STATES, type ChildKind, type ChildState } from "../../build/types.ts";

export interface WriterWaveNode {
  readonly id: string;
  readonly kind: ChildKind;
  readonly owns: readonly string[];
  readonly after?: readonly string[];
}

// Match the fleet's literal file-or-directory semantics, not globs or a filesystem sandbox.
function roots(node: WriterWaveNode): string[] {
  if (!node.owns.length) return ["."];
  return node.owns.map(raw => {
    const path = posix.normalize(raw).replace(/\/$/, "");
    if (raw.includes("\0") || raw.includes("\\") || posix.isAbsolute(raw) || /^[A-Za-z]:/.test(path) || path === ".." || path.startsWith("../")) {
      throw new Error(`Child ${node.id} owns must be repository-relative: ${JSON.stringify(raw)}`);
    }
    return path;
  });
}

/** Admit a complete wave after the caller has resolved kinds and validated IDs and dependencies. */
export function validateWriterWave(existing: readonly (WriterWaveNode & { readonly state: ChildState })[], incoming: readonly WriterWaveNode[]): void {
  const writers = incoming.filter(node => node.kind === "slice");
  if (!writers.length) return;
  const graph = new Map([...existing, ...incoming].map(node => [node.id, node.after ?? []]));
  const cache = new Map<string, Set<string>>();
  const ancestors = (id: string): Set<string> => {
    const cached = cache.get(id);
    if (cached) return cached;
    const seen = new Set<string>(), stack = [...graph.get(id) ?? []];
    while (stack.length) {
      const next = stack.pop()!;
      if (seen.has(next)) continue;
      seen.add(next);
      for (const parent of graph.get(next) ?? []) stack.push(parent);
    }
    cache.set(id, seen);
    return seen;
  };
  const peers = existing.filter(node => node.kind === "slice" && LIVE_CHILD_STATES.includes(node.state))
    .map(node => ({ id: node.id, paths: roots(node) }));
  for (const writer of writers) {
    const paths = roots(writer);
    for (const peer of peers) {
      if (ancestors(writer.id).has(peer.id) || ancestors(peer.id).has(writer.id)) continue;
      for (const a of paths) for (const b of peer.paths) {
        if (a === "." || b === "." || a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`)) {
          throw new Error(`Writer ownership conflict: ${writer.id} (${a}) overlaps ${peer.id} (${b}); use disjoint owns or order them with after`);
        }
      }
    }
    peers.push({ id: writer.id, paths });
  }
}
