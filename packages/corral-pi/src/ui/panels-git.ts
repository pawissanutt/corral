// Explicit, bounded, read-only Git queries. No shell, optional locks, diff helpers or textconv.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { PanelFile } from "./panels-view.ts";

const execute = promisify(execFile);
async function git(cwd: string, args: readonly string[], signal?: AbortSignal): Promise<string> {
  const result = await execute("git", ["--no-optional-locks", "--literal-pathspecs", "-c", "core.fsmonitor=false", ...args], {
    cwd, encoding: "utf8", timeout: 3000, maxBuffer: 256 * 1024, signal,
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
  });
  return result.stdout;
}
export function parseStatus(output: string): PanelFile[] {
  const fields = output.split("\0"), files: PanelFile[] = [];
  for (let index = 0; index < fields.length; index++) {
    const field = fields[index];
    if (!field || field.length < 4) continue;
    const status = field.slice(0, 2), path = field.slice(3);
    files.push({ status, path });
    if (/[RC]/.test(status)) index++; // -z prints destination first, then the original path.
    if (files.length === 100) break;
  }
  return files;
}
export async function readPanelFiles(cwd: string, signal?: AbortSignal): Promise<readonly PanelFile[]> {
  return parseStatus(await git(cwd, ["status", "--porcelain=v1", "-z", "--untracked-files=normal"], signal));
}
export async function readPanelDiff(cwd: string, file: PanelFile, signal?: AbortSignal): Promise<string> {
  if (file.status === "??") return "Untracked entry: " + file.path + "\nNo committed Git diff exists for this entry yet.";
  const text = await git(cwd, ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "HEAD", "--", file.path], signal);
  return text || "No changes against HEAD for this path.";
}
