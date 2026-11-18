// `/corral-export`: the current branch, attached to its task as offline HTML.
import { existsSync, mkdirSync, renameSync } from "node:fs";
import { join } from "node:path";

import type { AgentSession } from "@earendil-works/pi-coding-agent";

import { type CorralClient, sha256Hex } from "../tools/corral.ts";

export const EXPORT_COMMAND = "corral-export";

export interface SessionExport {
  readonly path: string;
  readonly artifact: unknown;
}

export async function exportSnapshot(
  session: AgentSession, sessionId: string, task: string, dir: string, client: CorralClient,
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
  const artifact = await client.artifact(task, path, "log", `Pi session ${sessionId} transcript at entry ${leaf} (offline HTML)`);
  return { path, artifact };
}
