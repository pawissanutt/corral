// The planning roles' kickoff: the dispatch prompt is sent once per planning request, marked
// with the request's identity and the prompt's digest, so a relaunched host never resends a
// request its session already observed while a new round (a rewritten prompt) is sent.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

function promptDigest(path: string): string {
  try {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
  } catch {
    return "unreadable";
  }
}

export function requestMarker(kind: string, ids: readonly (string | undefined)[], promptFile: string): string {
  return `[corral ${kind} request ${JSON.stringify([...ids, promptDigest(promptFile)])}]`;
}

/** Sends `text` with its marker on the first startup unless the session already holds the marker. */
export function sendOnce(pi: ExtensionAPI, text: string | undefined, marker: string): void {
  let submitted = false;
  pi.on("session_start", (event, ctx) => {
    if (event.reason !== "startup" || submitted || text === undefined) return;
    const observed = ctx.sessionManager.getEntries().some((entry) =>
      entry.type === "message" && entry.message.role === "user"
      && (typeof entry.message.content === "string"
        ? entry.message.content.includes(marker)
        : entry.message.content.some((part) => part.type === "text" && part.text.includes(marker))));
    submitted = true;
    if (!observed) pi.sendUserMessage(`${text}\n\n${marker}`);
  });
}
