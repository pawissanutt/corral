// Deduplicate startup requests against Pi's transcript, including after the host restarts.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { HostIdentity } from "../types.ts";

export function reviewRequestMarker({ task, run }: HostIdentity, promptFile: string): string {
  const prompt = readFileSync(promptFile, "utf8");
  // The engine writes this heading at the end of the review brief, after user-supplied sections.
  const round = Array.from(prompt.matchAll(/^## Review round (\d+)$/gm)).at(-1)?.[1] ?? null;
  const digest = createHash("sha256").update(prompt).digest("hex");
  return `[corral review request ${JSON.stringify([task, round, run, digest])}]`;
}

/** Sends the startup prompt once; a marker can identify a request independently of its prompt path. */
export function sendOnce(pi: ExtensionAPI, text: string, marker?: string): void {
  let submitted = false;
  const suffix = marker === undefined ? undefined : `\n\n${marker}`;
  pi.on("session_start", (event, ctx) => {
    if (event.reason !== "startup" || submitted) return;
    const observed = ctx.sessionManager.getEntries().some((entry) => {
      if (entry.type !== "message" || entry.message.role !== "user") return false;
      const { content } = entry.message;
      if (typeof content === "string") return suffix === undefined ? content === text : content.endsWith(suffix);
      return content.some((part) => part.type === "text"
        && (suffix === undefined ? part.text === text : part.text.endsWith(suffix)));
    });
    submitted = true;
    if (!observed) pi.sendUserMessage(suffix === undefined ? text : text + suffix);
  });
}
