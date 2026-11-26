import { realpathSync } from "node:fs";
import { basename, dirname, isAbsolute } from "node:path";
import { z } from "zod";
import { real } from "../roles/assistant/paths.ts";
import { engineRun, json } from "../build/corral-cli.ts";
import { findWorkspaceRoot } from "./config.ts";
import { TASK_ID } from "./worktree-access.ts";
import { tryWorktreeLock } from "./worktree-files.ts";

const WriterLock = z.union([
  z.object({ lock: z.null() }),
  z.object({ lock: z.string().refine(isAbsolute), lockDevice: z.string(), lockInode: z.string() }),
]);

/** A Code/Build host bound in a registered checkout participates for its entire process lifetime. */
export function taskWriterLock(cwd: string, corral: string | undefined, workspace: string | undefined) {
  return lockTask(realpathSync(cwd), corral, workspace);
}

/** File tools in other Corral hosts also participate, including a destination reached by alias.
 * A task run already protects its own checkout through engine liveness; no extra lookup there. */
export async function taskDestinationLock(path: string, corral: string | undefined, workspace: string | undefined, own?: string) {
  const canonical = real(path) ?? path;
  const lock = await lockTask(canonical, corral, workspace, own);
  if ((real(path) ?? path) !== canonical) {
    lock?.close();
    throw new Error("File destination changed during task writer admission.");
  }
  return lock;
}

async function lockTask(path: string, corral: string | undefined, workspace: string | undefined, own?: string) {
  for (let checkout = path; checkout !== dirname(checkout); checkout = dirname(checkout)) {
    const task = basename(checkout).toUpperCase();
    if (!TASK_ID.test(task) || basename(dirname(checkout)).toLowerCase() !== ".worktrees") continue;
    if (checkout === own) return;
    // A configured board can own repositories outside its directory. Do not let an
    // unrelated nearer database's null response override explicit board binding.
    const roots = new Set<string>();
    if (workspace) roots.add(workspace);
    for (let parent = dirname(dirname(checkout)); ;) {
      const root = findWorkspaceRoot({}, parent);
      if (!root) break;
      roots.add(root);
      if (root === dirname(root)) break;
      parent = dirname(root);
    }
    let failure: unknown;
    for (const root of roots) {
      let receipt: z.infer<typeof WriterLock>;
      try {
        receipt = WriterLock.parse(await json(engineRun(corral || "corral-next"),
          ["--root", root, "--json", "takeover-writer-lock", task, checkout], root, 15_000));
      } catch (error) { failure ??= error; continue; }
      if (receipt.lock === null) continue;
      const lock = tryWorktreeLock(receipt.lock, false);
      try {
        if (lock.device !== receipt.lockDevice || lock.inode !== receipt.lockInode) throw new Error("Task worktree writer lock changed during admission.");
        return lock;
      } catch (error) {
        lock.close();
        throw error;
      }
    }
    throw new Error(`Cannot verify which Corral workspace owns ${task}; open the checkout from its registered board.`, { cause: failure });
  }
}
