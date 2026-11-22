import type { Run } from "./corral-cli.ts";

/** Reject flags that let status or diff hide changes. cwd is the worktree root. */
export async function checkIndexVisibility(run: Run, cwd: string): Promise<void> {
  const cursor: { state: "tag" | "space" | "firstPathByte" | "path" } = { state: "tag" };
  let hidden = false, malformed = false;
  // Only keep enough state to parse `<tag> <path>\0`, even across chunk boundaries.
  // Paths are neither decoded nor retained (newlines, non-UTF-8 bytes, etc. are harmless).
  function stdoutChunk(chunk: Buffer) {
    for (let offset = 0; offset < chunk.length && !hidden && !malformed;) {
      if (cursor.state === "tag") {
        const tag = chunk[offset++]!;
        // -v lowercases the usual tag for assume-unchanged. S means skip-worktree.
        hidden = tag === 0x53 || (tag >= 0x61 && tag <= 0x7a);
        malformed = !hidden && tag !== 0x48 && tag !== 0x4d; // H (cached), M (unmerged)
        cursor.state = "space";
      } else if (cursor.state === "space") {
        malformed = chunk[offset++] !== 0x20;
        cursor.state = "firstPathByte";
      } else if (cursor.state === "firstPathByte") {
        malformed = chunk[offset++] === 0;
        cursor.state = "path";
      } else {
        const end = chunk.indexOf(0, offset);
        if (end < 0) return;
        offset = end + 1;
        cursor.state = "tag";
      }
    }
  }
  const unreadable = () => new Error("Cannot verify that Git reports all tracked changes. Inspect the Git index and try again.");
  let result: Awaited<ReturnType<Run>>;
  try {
    // Don't apply sparse checkout's in-memory clearing of skip-worktree on present paths.
    result = await run(["--no-optional-locks", "-c", "core.sparseCheckout=false", "ls-files", "-v", "-z"], { cwd, stdoutChunk });
  } catch { throw unreadable(); }
  if (hidden) throw new Error("The Git index has assume-unchanged or skip-worktree entries. Clear those flags (or use a full checkout), then try again.");
  if (result.code !== 0 || result.stdout || malformed || cursor.state !== "tag") throw unreadable();
}
