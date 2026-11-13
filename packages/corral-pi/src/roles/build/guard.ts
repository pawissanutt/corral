import { relative, sep } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { assistantShellRefusal } from "../assistant/guard.ts";
import { real, resolveToolPath, within } from "../assistant/paths.ts";
import type { RoleContext } from "../types.ts";

// Cover ordinary Git invocations, including -C/-c and explicit repository paths. These
// fixed accident guards are deliberately conservative, not a shell security boundary.
const GIT = /(?:^|[\s/;&|()`'"])git\s+(?:(?:-C|-c|--git-dir|--work-tree|--namespace)\s+(?:"[^"]*"|'[^']*'|[^\s;&|]+)\s+|--(?:git-dir|work-tree|namespace)=[^\s;&|]+\s+|--(?:no-pager|paginate|bare)\s+)*/g;

export function buildShellRefusal(command: string): string | undefined {
  const fixed = assistantShellRefusal(command);
  if (fixed) return fixed;
  for (const match of command.matchAll(GIT)) {
    const args = command.slice(match.index! + match[0].length);
    if (/^worktree\s+remove\b/.test(args)) {
      return "build rule: do not remove a worktree from under running children; nothing ran. The owner controls session cleanup.";
    }
    // The command-only role hook has no session identity: refuse all branch deletion,
    // rather than risk deleting the session branch through another checkout.
    if (/^branch\b[^\n;&|]*\s(?:-[a-zA-Z]*[dD][a-zA-Z]*|--delete)(?=\s|$|['"])/.test(args)) {
      return "build rule: do not delete branches from under running children; nothing ran. The owner controls session cleanup.";
    }
  }
  return undefined;
}

export type BuildWriteClassification = { allowed: true } | { allowed: false; reason: string };
const PROTECTED: readonly string[] = [".git", ".corral", ".worktrees"];

/** Check both the tool's lexical destination and where its existing symlinks lead. */
export function classifyBuildWrite(path: string, worktree: string): BuildWriteClassification {
  const root = resolveToolPath(worktree, ".");
  const target = resolveToolPath(path, root);
  if (!within(target, root)) return { allowed: false, reason: "build rule: writes must stay inside the session worktree; nothing was written." };
  if (relative(root, target).split(sep).some((part) => PROTECTED.includes(part))) {
    return { allowed: false, reason: "build rule: writes into .git, .corral or .worktrees are refused; nothing was written." };
  }
  const canonicalRoot = real(root);
  const canonicalTarget = real(target);
  if (!canonicalRoot || !canonicalTarget) {
    return { allowed: false, reason: "build rule: the write destination could not be resolved safely; nothing was written." };
  }
  if (!within(canonicalTarget, canonicalRoot)) {
    return { allowed: false, reason: "build rule: the write destination resolves outside the session worktree; nothing was written." };
  }
  if (relative(canonicalRoot, canonicalTarget).split(sep).some((part) => PROTECTED.includes(part))) {
    return { allowed: false, reason: "build rule: the write destination resolves into .git, .corral or .worktrees; nothing was written." };
  }
  return { allowed: true };
}

export function installBuildGuard(pi: ExtensionAPI, context: RoleContext): void {
  pi.on("tool_call", async (event) => {
    if (event.toolName !== "edit" && event.toolName !== "write") return undefined;
    if (!context.build) return { block: true, reason: "build rule: no build session is bound, so every edit and write is refused; nothing was written." };
    const path = (event.input as Record<string, unknown>).path;
    if (typeof path !== "string") return { block: true, reason: "build rule: edit and write require a target path; nothing was written." };
    try {
      const result = classifyBuildWrite(resolveToolPath(path, context.cwd), context.build.session.worktree);
      return result.allowed ? undefined : { block: true, reason: result.reason };
    } catch {
      return { block: true, reason: "build rule: the write destination could not be resolved safely; nothing was written." };
    }
  });
}
