import { FILE_LIMIT, panelGit } from "./panels-git.ts";
import type { PanelFile } from "./panels-view.ts";

/** `git diff --name-status -z`, never human tables or status of the working tree. */
export function parseChangedPaths(text: string): PanelFile[] {
  const fields = text.split("\0"), result: PanelFile[] = [];
  for (let i = 0; i < fields.length && result.length < FILE_LIMIT; ) {
    const status = fields[i++]; if (!status) break;
    const first = fields[i++]; if (first === undefined) throw new Error("Incomplete changed-path record");
    if (/^[RC]\d*$/.test(status)) {
      const path = fields[i++]; if (path === undefined) throw new Error("Incomplete rename record");
      result.push({ path, originalPath: first, status: status[0] ?? status });
    } else result.push({ path: first, status });
  }
  return result;
}
export function readCandidateDiff(root: string, file: PanelFile, base: string | undefined, head: string | undefined, signal?: AbortSignal): Promise<string> {
  if (!base || !head || !/^[a-f0-9]{40,64}$/.test(base) || !/^[a-f0-9]{40,64}$/.test(head)) throw new Error("Candidate identity unavailable");
  const paths = file.originalPath ? [file.originalPath, file.path] : [file.path];
  return panelGit(root, ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--find-renames", base, head, "--", ...paths], signal);
}
