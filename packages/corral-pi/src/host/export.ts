// `/corral-export` (T-564, plan T-460@P3 menu 6): the bound session's current branch as one
// offline HTML file, attached to the container through the role's `artifact` operation. Nothing is
// uploaded. A snapshot is the session and its branch's last transcript entry; corral's own
// `custom` bookkeeping entries (such as a bridge delivery record) are not transcript. Exporting it
// again reuses the file and the request ID, so corral replays the first receipt.
import { existsSync, mkdirSync, renameSync } from "node:fs";
import { join } from "node:path";

import type { AgentSession } from "@earendil-works/pi-coding-agent";

import { type CorralClient, type CorralReceipt, sha256Hex, toolRequestId } from "../tools/corral.ts";

export const EXPORT_COMMAND = "corral-export";

export interface SessionExport {
  readonly path: string;
  readonly receipt: CorralReceipt;
}

export async function exportSnapshot(
  session: AgentSession, sessionId: string, dir: string, client: CorralClient,
): Promise<SessionExport> {
  const leaf = session.sessionManager.getBranch().findLast((entry) => entry.type !== "custom")?.id ?? "empty";
  const key = sha256Hex(`${sessionId}\0${leaf}`).slice(0, 16);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const path = join(dir, `session-${key}.html`);
  if (!existsSync(path)) {
    const staged = `${path}.${process.pid}.partial`;
    // A rich pane's corral theme exists only in memory, which the exporter cannot load by name.
    await session.exportToHtml(staged, { themeName: "dark" });
    renameSync(staged, path);
  }
  const receipt = await client.call("artifact", {
    requestId: toolRequestId(sessionId, `${EXPORT_COMMAND}:${leaf}`),
    payload: { path, kind: "log", title: `Pi session ${sessionId} transcript at entry ${leaf} (offline HTML)` },
  });
  return { path, receipt };
}
