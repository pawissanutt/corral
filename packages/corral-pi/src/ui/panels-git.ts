// Bounded read-only Git. Always resolve porcelain paths against the actual repository root.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { lstat } from "node:fs/promises";
import { join } from "node:path";
import type { PanelFile } from "./panels-view.ts";
const execute = promisify(execFile);
export type Comparison = "candidate" | "session" | "all" | "unstaged" | "staged";
export const FILE_LIMIT = 500;
export async function panelGit(cwd: string, args: readonly string[], signal?: AbortSignal, different = false): Promise<string> {
  try {
    return (await execute("git", ["--no-optional-locks", "--literal-pathspecs", "-c", "core.fsmonitor=false", ...args], {
      cwd, encoding: "utf8", timeout: 3000, maxBuffer: 256 * 1024, signal,
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
