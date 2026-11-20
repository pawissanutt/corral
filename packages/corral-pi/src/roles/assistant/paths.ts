// Path handling shared by the build write guard and its child agents.
import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
const UNICODE_SPACES = /[\u00A0\u2000-\u200A\u202F\u205F\u3000]/g;

function normalizePiPath(path: string): string {
  if (process.platform === "win32" && !path.includes("\\")) {
    const mount = /^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i.exec(path);
    if (mount) path = `${mount[1]!.toUpperCase()}:\\${mount[2]?.replaceAll("/", "\\") ?? ""}`;
  }
  if (path === "~") return homedir();
  if (path.startsWith("~/") || (process.platform === "win32" && path.startsWith("~\\"))) {
    return join(homedir(), path.slice(2));
  }
  return path.startsWith("file://") ? fileURLToPath(path) : path;
}

/** Mirror Pi's private resolveToCwd for edit/write; checked against the SDK in paths.test.ts. */
export function resolveToolPath(input: string, cwd: string): string {
  const normalized = input.replace(UNICODE_SPACES, " ");
  const path = normalizePiPath(normalized.startsWith("@") ? normalized.slice(1) : normalized);
  const base = normalizePiPath(cwd);
  return isAbsolute(path) ? resolve(path) : resolve(base, path);
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
export function real(path: string): string | undefined {
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

export function within(path: string, root: string): boolean {
  const rel = relative(root, path);
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}
