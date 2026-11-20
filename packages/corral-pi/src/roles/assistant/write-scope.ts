import { lstatSync, realpathSync, statSync } from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";

import { resolveToolPath, within } from "./paths.ts";

export type WriteScopeDecision = { action: "allow" } | { action: "ask"; path: string; canonical: string } | { action: "deny"; reason: string };

const PROTECTED = new Set([".git", ".corral", ".corral-next", ".worktrees"]);
const UNSAFE = "The write destination could not be resolved safely; nothing was written.";
const STATE = "Writes into Git or Corral state are refused; nothing was written.";

function hasProtectedComponent(path: string, ownWorktree?: string): boolean {
  const parts = path.split(sep);
  const parent = ownWorktree === undefined ? undefined : dirname(ownWorktree);
  // Only a Build's own direct .worktrees entry exempts its containing segment, never
  // another protected ancestor, a nested protected component, or the container itself.
  const exempt = ownWorktree && parent && basename(parent).toLowerCase() === ".worktrees" && within(path, ownWorktree)
    ? parent.split(sep).length - 1 : -1;
  return parts.some((part, i) => i !== exempt && PROTECTED.has(part.toLowerCase()));
}

function isMarkdownMemory(path: string, workspace: string): boolean {
  const memory = join(workspace, ".corral-next", "memory");
  return within(path, memory) && path.endsWith(".md") && !hasProtectedComponent(workspace)
    && !hasProtectedComponent(relative(memory, path));
}

function canonicalDirectory(path: string): string {
  const canonical = realpathSync.native(path);
  if (!statSync(canonical).isDirectory()) throw new Error(UNSAFE);
  return canonical;
}

/** Follow symlinks in the existing prefix, permitting missing components for a new file. */
function canonicalDestination(path: string): string {
  const missing: string[] = [];
  let ancestor = path;
  // lstat detects a dangling symlink itself; realpath then fails closed rather
  // than treating the symlink as just another missing component.
  while (!lstatSync(ancestor, { throwIfNoEntry: false })) {
    const parent = dirname(ancestor);
    if (parent === ancestor) throw new Error(UNSAFE);
    missing.push(basename(ancestor));
    ancestor = parent;
  }
  if (missing.length) return join(canonicalDirectory(ancestor), ...missing.reverse());
  const canonical = realpathSync.native(ancestor);
  if (!statSync(canonical).isFile()) throw new Error(UNSAFE);
  return canonical;
}

export function classifyWriteScope(path: string, scope: {
  cwd: string; root: string; workspace?: string; buildWorktree?: boolean;
}): WriteScopeDecision {
  try {
    const root = resolve(scope.root);
    const target = resolveToolPath(path, scope.cwd);
    const canonicalRoot = canonicalDirectory(root);
    const canonical = canonicalDestination(target);
    if (scope.workspace) {
      const workspace = resolve(scope.workspace);
      // The workspace itself may be an alias. The memory subtree must still have
      // its normal location below that workspace; don't follow memory aliases
      // to redefine which protected directory counts as memory.
      if (isMarkdownMemory(target, workspace) && isMarkdownMemory(canonical, canonicalDirectory(workspace))) {
        return { action: "allow" };
      }
    }
    if (hasProtectedComponent(target, scope.buildWorktree ? root : undefined)
      || hasProtectedComponent(canonical, scope.buildWorktree ? canonicalRoot : undefined)) {
      return { action: "deny", reason: STATE };
    }
    if (within(target, root) && within(canonical, canonicalRoot)) return { action: "allow" };
    return { action: "ask", path: target, canonical };
  } catch {
    return { action: "deny", reason: UNSAFE };
  }
}
