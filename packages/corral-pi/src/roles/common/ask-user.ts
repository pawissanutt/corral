import type { RoleContext } from "../types.ts";

/** Shared with the tool's prompt metadata; only the two interactive loadouts receive it. */
export const ASK_USER_GUIDELINES = [
  "Use ask_user for every real owner-facing question, decision or clarification, not an unanswered question only in chat. Chat may explain a question or summarize its answer.",
  "Give ask_user 2-4 meaningful choices per question, label a recommendation when useful, and offer freeText/Other for answers not covered. Research discoverable facts yourself; batch independent decisions sensibly and do not invent questions just to show UI.",
  "Act on clear owner instructions without redundant approval. ask_user does not replace owner-action confirmation or file-permission dialogs and cannot grant board authority or bypass them.",
  "Prefer ask_user nonBlocking:true when independent work can continue. Non-blocking questions use plain-string, single-selection options with optional freeText; use the synchronous dialog for headers, rich descriptions or multiSelect. It returns a stable pending question ID; the owner reopens choices with /questions or Ctrl+Shift+Q. Continue unrelated research/implementation, but stop dependent permission/scope steps. With corral_todo available, mark only those steps wait on the named owner question; when all remaining work waits, end the turn for the answer follow-up, never busy-poll. Withdraw an unneeded question with ask_user withdraw; supersedes replaces a pending question. A pending ID/status or host notification is not an owner answer.",
  "An unavailable UI, cancellation, timeout or missing response is never approval. State the limitation and unresolved decision truthfully; do not proceed with dependent work or repeatedly bury/re-ask the question in prose.",
];

export function askUserContract(context: RoleContext): string {
  if (!context.capabilities?.enabled.includes("ask_user")) {
    return "ask_user is disabled for this generation. State any unresolved owner decision and the UI limitation truthfully; no missing answer is approval. Do not repeatedly re-ask in prose.";
  }
  return "## Owner questions\n\n" + ASK_USER_GUIDELINES.join("\n\n");
}
