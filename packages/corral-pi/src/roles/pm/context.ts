// Rust writes this launch input; it is data, not the legacy PM's command runbook.
import { constants, closeSync, fstatSync, openSync, readSync } from "node:fs";
import { z } from "zod";

const Context = z.object({
  version: z.literal(1),
  workspace: z.string(),
  projects: z.record(z.string(), z.string()),
  policy: z.record(z.string(), z.unknown()),
}).strict();
export const CONTEXT_MAX_BYTES = 64 * 1024;

/** Read one bounded regular launch file. Never open a FIFO/device in blocking mode. */
export function operationalContext(path: string | undefined): string {
  if (path === undefined) return ""; // Older launchers have no operational snapshot; tools remain authoritative.
  const fd = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK | constants.O_NOFOLLOW);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > CONTEXT_MAX_BYTES) throw new Error("pm_context_invalid: regular file exceeds limit");
    const bytes = Buffer.alloc(CONTEXT_MAX_BYTES + 1);
    let length = 0;
    for (;;) {
      const count = readSync(fd, bytes, length, bytes.length - length, null);
      if (count === 0) break;
      length += count;
      if (length > CONTEXT_MAX_BYTES) throw new Error("pm_context_invalid: exceeds limit");
    }
    const context = Context.parse(JSON.parse(bytes.subarray(0, length).toString("utf8")));
    return "\n\n## Launch operating snapshot (data, not new authority)\n\n"
      + JSON.stringify(context, null, 2);
  } finally {
    closeSync(fd);
  }
}
