import { lstat, opendir, realpath } from "node:fs/promises";
import { join } from "node:path";
import type { Run } from "./corral-cli.ts";

const unreadable = () => new Error("Cannot verify that Git reports all tracked changes. Inspect the Git index and try again.");
const unverified = () => new Error("Cannot verify a submodule checkout against its gitlink. Restore or initialize the submodule before preparing the candidate.");
const missing = (e: unknown) => (e as NodeJS.ErrnoException).code === "ENOENT";

/** Read the actual index, not .gitmodules or submodule.active, which can omit local checkouts. */
export async function checkIndexVisibility(run: Run, cwd: string): Promise<void> {
  let count = 0;
  async function visit(dir: string, depth: number): Promise<void> {
    const links: { path: string; oid: string }[] = [];
    let carry = Buffer.alloc(0), hidden = false, invalid = false;
    // NUL-delimited bytes: tabs and newlines in ordinary paths never become index flags.
    // Retain at most one bounded partial row plus the gitlinks we actually need to visit.
    function rows(chunk: Buffer) {
      for (let start = 0; start < chunk.length && !hidden && !invalid;) {
        const nul = chunk.indexOf(0, start);
        const part = chunk.subarray(start, nul < 0 ? chunk.length : nul);
        if (part.length + carry.length > 8192) { invalid = true; return; }
        const row = carry.length ? Buffer.concat([carry, part]) : part;
        if (nul < 0) { carry = Buffer.from(row); return; }
        carry = Buffer.alloc(0);
        const tag = row[0] ?? 0;
        hidden = tag === 83 || (tag >= 97 && tag <= 122); // S: skip-worktree; lowercase: assume-unchanged
        const tab = row.indexOf(9);
        if (!hidden && ((tag !== 72 && tag !== 77) || row[1] !== 32 || tab < 0 || tab + 1 === row.length)) invalid = true;
        if (!hidden && !invalid && row.toString("ascii", 2, 8) === "160000") {
          const match = /^160000 ([a-f0-9]{40}|[a-f0-9]{64}) 0$/.exec(row.toString("ascii", 2, tab));
          const bytes = row.subarray(tab + 1), path = bytes.toString("utf8");
          if (!match || !Buffer.from(path).equals(bytes) || path.split("/").some(p => !p || p === "." || p === ".." || p === ".git") || ++count > 256 || depth >= 16) invalid = true;
          else links.push({ path, oid: match[1]! });
        }
        start = nul + 1;
      }
    }
    const listing = await run(["--no-optional-locks", "-c", "core.sparseCheckout=false", "ls-files", "--stage", "-v", "-z"], { cwd: dir, stdoutChunk: rows }).catch(() => { throw unreadable(); });
    if (hidden) throw new Error("The Git index has assume-unchanged or skip-worktree entries. Clear those flags (or use a full checkout), then try again.");
    if (listing.code !== 0 || listing.stdout || invalid || carry.length) throw unreadable();
    if (!links.length) return;
    const root = await realpath(dir);
    for (const link of links) {
      const child = join(root, link.path);
      const stat = await lstat(child).catch(e => { if (!missing(e)) throw unverified(); });
      if (!stat) continue; // Not initialized: no files at this gitlink.
      if (!stat.isDirectory() || await realpath(child) !== child) throw unverified();
      const metadata = await lstat(join(child, ".git")).catch(e => { if (!missing(e)) throw unverified(); });
      if (!metadata) {
        const entries = await opendir(child);
        try { if (await entries.read()) throw unverified(); } finally { await entries.close(); }
        continue;
      }
      if (!metadata.isFile() && !metadata.isDirectory()) throw unverified();
      const inspect = async (args: string[]) => {
        const reply = await run(["--no-optional-locks", ...args], { cwd: child });
        if (reply.code !== 0) throw unverified();
        return reply.stdout;
      };
      if ((await inspect(["rev-parse", "--show-toplevel"])).replace(/\n$/, "") !== child ||
          (await inspect(["rev-parse", "--verify", "HEAD"])).trim() !== link.oid) throw unverified();
      await visit(child, depth + 1);
      if ((await inspect(["status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"])).length) throw unverified();
    }
  }
  await visit(cwd, 0);
}
