import type { RoleContext } from "../types.ts";

/** Shared with the tool's prompt metadata. */
export const ASK_USER_GUIDELINES = [
  "Use ask_user for every real owner-facing question, decision or clarification, not an unanswered question only in chat. Chat may explain a question or summarize its answer.",
  "Give ask_user 2-4 meaningful choices per question, label a recommendation when useful, and offer freeText/Other for answers not covered. Research discoverable facts yourself; batch independent decisions sensibly and do not invent questions just to show UI.",
  "Act on clear owner instructions without redundant approval. An answer gives direction for this session, not authority to bypass checks or change unrelated work.",
  "Prefer ask_user nonBlocking:true when independent work can continue. Non-blocking questions use plain-string, single-selection options with optional freeText; use a synchronous dialog for headers, rich descriptions or multiSelect. Continue unrelated work but stop dependent steps. If your todo supports waits, record only the steps waiting on the named owner question. End the turn when all remaining work waits; do not poll. Withdraw an unneeded pending question with ask_user withdraw. A pending ID or notification is not an answer.",
  "An unavailable UI, cancellation, timeout or missing response is never approval. State the limitation and unresolved decision truthfully; do not proceed with dependent work or repeatedly bury/re-ask the question in prose.",
];

export function askUserContract(context: RoleContext): string {
  if (!context.capabilities?.enabled.includes("ask_user")) {
    return "ask_user is disabled for this session. State any unresolved owner decision and the UI limitation truthfully; no missing answer is approval. Do not repeatedly re-ask in prose.";
  }
  return "## Owner questions\n\n" + ASK_USER_GUIDELINES.join("\n\n");
}
