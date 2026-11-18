// Path handling shared by the build write guard and its child agents.
import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
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
