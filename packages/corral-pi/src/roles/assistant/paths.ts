// The assistant's fixed write roots (T-544 scratch-worktrees). edit and write reach only the
// session's recorded scratch worktrees, its session artifact directory and the system temp
// directory, so an honest model never edits the main checkout or a task worktree by accident.
// Presets, capability settings and configured rules can neither disable nor widen them.
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { RoleContext } from "../types.ts";

const WRITE_TOOLS: ReadonlySet<string> = new Set(["edit", "write"]);
const UNICODE_SPACES = /[\u00A0\u1680\u2000-\u200A\u202F\u205F\u3000]/g;

/** A tool path resolved as Pi's edit and write tools resolve it: `@`, `~`, file URLs, cwd. */
export function resolveToolPath(input: string, cwd: string): string {
  let path = input.replace(UNICODE_SPACES, " ");
  if (path.startsWith("@")) path = path.slice(1);
  if (path === "~") path = homedir();
  else if (path.startsWith("~/")) path = join(homedir(), path.slice(2));
  else if (path.startsWith("file://")) path = fileURLToPath(path);
  return resolve(cwd, path);
}

function present(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

/** Where a write lands once symlinks in the existing part of the path are followed. */
function real(path: string): string | undefined {
  const rest: string[] = [];
  let existing = path;
  while (!present(existing)) {
    const parent = dirname(existing);
    if (parent === existing) return undefined;
    rest.unshift(basename(existing));
    existing = parent;
  }
  try {
    return join(realpathSync(existing), ...rest);
  } catch {
    return undefined;
  }
}

function within(path: string, root: string): boolean {
  const rel = relative(root, path);
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}

function unquote(value: string): string {
  if (value.length > 1 && value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1).replaceAll("''", "'");
  if (value.startsWith("\"")) {
    try {
      return JSON.parse(value) as string;
    } catch {
      return value;
    }
  }
  return value;
}

/** The session's recorded scratch worktrees, read at each call so a new scratch counts at once. */
export function scratchRoots(workspace: string, session: string): string[] {
  let text: string;
  try {
    text = readFileSync(join(workspace, ".corral", "assistants", `${session}.md`), "utf8");
  } catch {
    return [];
  }
  const frontmatter = text.split("\n---\n", 1)[0] ?? "";
  return frontmatter.split("\n")
    .map((line) => /^(?:\s+|-\s+)path:\s+(.+?)\s*$/.exec(line)?.[1])
    .filter((value): value is string => value !== undefined)
    .map(unquote)
    .filter((path) => isAbsolute(path) && basename(dirname(path)) === ".worktrees"
      && basename(path).startsWith(`${session}-`));
}

/** Why the assistant may not edit or write `input`, or undefined when it is inside its roots. */
export function writeRefusal(
  input: string, workspace: string, session: string, temp: string = tmpdir(),
): string | undefined {
  const target = real(resolveToolPath(input, workspace));
  const roots = [
    ...scratchRoots(workspace, session),
    join(workspace, ".corral", "artifacts", "assistant", session),
    temp,
  ].map(real).filter((root): root is string => root !== undefined);
  if (target !== undefined && roots.some((root) => within(target, root))) return undefined;
  return `assistant rule: edit and write reach only this session's scratch worktrees, `
    + `.corral/artifacts/assistant/${session}/ and the temp directory, not ${JSON.stringify(input)}; `
    + "nothing was written. Make a scratch with `corral assistant scratch new -p <project>` and edit there.";
}

/** Refuses every edit and write call outside the roots, top-level or nested. */
export function installWriteGuard(pi: ExtensionAPI, context: RoleContext): void {
  const session = context.identity.assistant;
  pi.on("tool_call", (event) => {
    if (!WRITE_TOOLS.has(event.toolName)) return undefined;
    const path = (event.input as Record<string, unknown>).path;
    if (session === undefined || typeof path !== "string") {
      return { block: true, reason: "assistant rule: edit and write need a path and a bound assistant session; nothing was written." };
    }
    const reason = writeRefusal(path, context.cwd, session);
    return reason === undefined ? undefined : { block: true, reason };
  });
}
