// Top-level browser targets: an exact configured dev origin, or an HTML file inside the canonical
// worktree. Only the top-level document is checked; page subrequests are not intercepted.
import { realpathSync, statSync } from "node:fs";
import { extname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export class BrowserError extends Error {
  constructor(readonly code: string, message: string) {
    super(`${code}: ${message}`);
  }
}

const HTML = new Set([".html", ".htm"]);

export function inside(root: string, path: string): boolean {
  const rel = relative(root, path);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel));
}

/** A worktree HTML file as its canonical file URL; symlinks may not leave the worktree. */
function worktreeFile(path: string, cwd: string): URL {
  const root = realpathSync(cwd);
  let real: string;
  try {
    real = realpathSync(resolve(cwd, path));
  } catch {
    throw new BrowserError("browser_target_refused", `${path} does not exist in the worktree`);
  }
  if (!inside(root, real)) throw new BrowserError("browser_target_refused", `${path} is outside the worktree`);
  if (!statSync(real).isFile() || !HTML.has(extname(real).toLowerCase())) {
    throw new BrowserError("browser_target_refused", `${path} is not an .html or .htm file`);
  }
  return pathToFileURL(real);
}

/**
 * Resolves a requested target: an `http(s)` URL whose origin is configured, a `file:` URL, or a
 * worktree-relative/absolute path to an HTML file. Everything else is refused.
 */
export function browserTarget(input: string, origins: readonly string[], cwd: string): URL {
  const url = URL.parse(input);
  if (url === null) return worktreeFile(input, cwd);
  if (url.protocol === "file:") return worktreeFile(fileURLToPath(url), cwd);
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new BrowserError("browser_target_refused", "use a configured http(s) dev origin without credentials, or a worktree HTML file");
  }
  if (!origins.includes(url.origin)) {
    throw new BrowserError("browser_target_refused", `${url.origin} is not a configured browser origin`);
  }
  return url;
}

/** The document the browser ended on must still be an allowed target (redirects, scripted navigation). */
export function checkLanded(href: string, origins: readonly string[], cwd: string): void {
  try {
    browserTarget(href, origins, cwd);
  } catch (error) {
    const reason = error instanceof BrowserError ? error.message : String(error);
    throw new BrowserError("browser_target_refused", `the page left its allowed target and landed on ${href} (${reason})`);
  }
}
