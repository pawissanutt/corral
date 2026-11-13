// Bounded read-only Git. Always resolve porcelain paths against the actual repository root.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { constants } from "node:fs";
import { lstat, open } from "node:fs/promises";
import { join } from "node:path";
import type { PanelFile } from "./panels-view.ts";
const execute = promisify(execFile);
export type Comparison = "candidate" | "session" | "all" | "unstaged" | "staged";
export const FILE_LIMIT = 500;
/** Bytes one Git read (or one untracked file's count) may take. */
const READ_LIMIT = 256 * 1024;
/** Untracked files counted per read, and the bytes all of them may take together; the rest keep no count. */
const UNTRACKED_COUNTED = 100, UNTRACKED_BYTES = 4 * 1024 * 1024;
export async function panelGit(cwd: string, args: readonly string[], signal?: AbortSignal, different = false): Promise<string> {
  try {
    return (await execute("git", ["--no-optional-locks", "--literal-pathspecs", "-c", "core.fsmonitor=false", ...args], {
      cwd, encoding: "utf8", timeout: 3000, maxBuffer: READ_LIMIT, signal,
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
    })).stdout;
  } catch (error) {
    if (different && error instanceof Error && "code" in error && error.code === 1
      && "stdout" in error && typeof error.stdout === "string") return error.stdout;
    throw error;
  }
}
export function gitFailure(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/not a git repository/i.test(message)) return "Not a Git repository";
  if (error instanceof Error && "code" in error && error.code === "ENOENT") return "Git is not installed";
  // execFile prefixes the whole command line; Git's own fatal/error line is the readable part.
  const lines = message.split("\n").map(line => line.trim()).filter(Boolean);
  return lines.find(line => /^(fatal|error):/i.test(line)) ?? lines.at(-1) ?? "Git unavailable";
}
export async function repositoryRoot(cwd: string, signal?: AbortSignal): Promise<string> {
  return (await panelGit(cwd, ["rev-parse", "--show-toplevel"], signal)).trimEnd();
}
export function parseStatus(output: string): PanelFile[] {
  const fields = output.split("\0"), files: PanelFile[] = [];
  for (let index = 0; index < fields.length; index++) {
    const field = fields[index]; if (!field || field.length < 4) continue;
    const status = field.slice(0, 2), path = field.slice(3);
    const originalPath = /[RC]/.test(status) ? fields[++index] : undefined;
    files.push({ status, path, ...(originalPath ? { originalPath } : {}) });
    if (files.length === FILE_LIMIT) break;
  }
  return files;
}
export function comparisonArgs(comparison: Comparison): string[] {
  switch (comparison) {
    case "candidate": throw new Error("Candidate comparison requires captured base and HEAD");
    case "session": throw new Error("Session comparison requires its recorded base");
    case "all": return ["HEAD"];
    case "unstaged": return [];
    case "staged": return ["--cached"];
  }
}
export function relevantFile(file: PanelFile, comparison: Comparison): boolean {
  switch (comparison) {
    case "candidate":
    case "session":
    case "all": return true;
    case "unstaged": return file.status === "??" || file.status[1] !== " ";
    case "staged": return file.status !== "??" && file.status[0] !== " ";
  }
}
export function parseNumstat(text: string): ReadonlyMap<string, { added: number | null; removed: number | null }> {
  const fields = text.split("\0"), rows = new Map<string, { added: number | null; removed: number | null }>();
  for (let i = 0; i < fields.length; i++) {
    const record = fields[i]; if (!record) continue;
    const match = /^(\d+|-)\t(\d+|-)\t([\s\S]*)$/.exec(record); if (!match) continue;
    let path = match[3];
    if (path === "") { i++; path = fields[++i]; }
    if (path === undefined) continue;
    rows.set(path, { added: match[1] === "-" ? null : Number(match[1]), removed: match[2] === "-" ? null : Number(match[2]) });
  }
  return rows;
}
/**
 * Added-line counts of untracked files, as numstat gives them against the empty file: one per newline plus an
 * unterminated last line; `null` for binary content (a NUL byte in the first 8000 bytes, as Git decides). Files are
 * read directly, without a Git process: only regular files (never a symlink, which is not followed), each at most
 * READ_LIMIT bytes, the first UNTRACKED_COUNTED of them within UNTRACKED_BYTES in list order. The rest get no count.
 */
export async function untrackedStats(root: string, files: readonly PanelFile[], signal?: AbortSignal): Promise<Map<string, { added: number | null; removed: number | null }>> {
  const rows = new Map<string, { added: number | null; removed: number | null }>();
  let counted = 0, bytes = 0;
  for (const file of files) {
    if (file.status !== "??") continue;
    if (signal?.aborted || counted >= UNTRACKED_COUNTED) break;
    const path = join(root, file.path);
    const link = await lstat(path).catch(() => undefined);
    if (!link?.isFile() || link.size > READ_LIMIT || bytes + link.size > UNTRACKED_BYTES) continue;
    const handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0)).catch(() => undefined);
    if (!handle) continue;
    try {
      // The path may have changed since lstat: count only the same regular file, still within the limit.
      const stat = await handle.stat();
      if (!stat.isFile() || stat.ino !== link.ino || stat.size > READ_LIMIT) continue;
      const buffer = Buffer.alloc(stat.size), { bytesRead } = await handle.read(buffer, 0, stat.size, 0);
      const text = buffer.subarray(0, bytesRead);
      counted++; bytes += bytesRead;
      if (text.subarray(0, 8000).includes(0)) { rows.set(file.path, { added: null, removed: null }); continue; }
      let lines = 0; for (const byte of text) if (byte === 0x0a) lines++;
      if (bytesRead && text[bytesRead - 1] !== 0x0a) lines++;
      rows.set(file.path, { added: lines, removed: 0 });
    } catch { continue; } finally { await handle.close().catch(() => {}); }
  }
  return rows;
}
export async function readPanelFiles(cwd: string, signal?: AbortSignal): Promise<readonly PanelFile[]> {
  const root = await repositoryRoot(cwd, signal);
  return parseStatus(await panelGit(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"], signal));
}
export async function readPanelDiff(cwd: string, file: PanelFile, signal?: AbortSignal, comparison: Comparison = "all", head?: string): Promise<string> {
  // A session diff is the working tree against its recorded base, which arrives as `head`.
  if (comparison === "session") comparison = "all";
  const root = await repositoryRoot(cwd, signal);
  if (file.status === "??") {
    if ((await lstat(join(root, file.path))).isDirectory()) return "Directory entry; choose an individual file.";
    return (await panelGit(root, ["diff", "--no-index", "--no-color", "--no-ext-diff", "--no-textconv", "--", process.platform === "win32" ? "NUL" : "/dev/null", file.path], signal, true)) || "Empty new file.";
  }
  if (head !== undefined && !/^[a-f0-9]{40,64}$/.test(head)) throw new Error("Invalid captured Git base");
  let base = head && comparison === "all" ? [head] : head && comparison === "staged" ? ["--cached", head] : comparisonArgs(comparison);
  if (comparison === "all" && !head) {
    try { await panelGit(root, ["rev-parse", "--verify", "HEAD"], signal); }
    catch (error) {
      if (signal?.aborted) throw error;
      // Before the first commit, an added working file is compared to the empty file.
      if ((await lstat(join(root, file.path)).catch(() => null))?.isFile()) {
        return panelGit(root, ["diff", "--no-index", "--no-color", "--no-ext-diff", "--no-textconv", "--", process.platform === "win32" ? "NUL" : "/dev/null", file.path], signal, true);
      }
      base = ["--cached"];
    }
  }
  const paths = file.originalPath ? [file.originalPath, file.path] : [file.path];
  return (await panelGit(root, ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--find-renames", ...base, "--", ...paths], signal)) || "No changes in this comparison.";
}
