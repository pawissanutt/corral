// The owner's one question for a gate-triage proposal, in the pane's existing choice dialog. Only an
// explicit choice answers: Escape, a timeout, an abort or a missing UI leave the proposal pending,
// and "Decide later" says so explicitly. No answer is ever inferred as approval or refusal.
import type { GateTriageProposal, TriageUi } from "../tools/define.ts";

export const TRIAGE_RECORD = "Record this diagnosis";
export const TRIAGE_DECLINE = "Decline";
export const TRIAGE_LATER = "Decide later";
export const TRIAGE_TIMEOUT_MS = 300_000;

/** true: record it; false: decline it; undefined: still pending. */
export async function askGateTriage(
  proposal: GateTriageProposal, context: TriageUi, signal: AbortSignal,
  refused: () => string | undefined = () => undefined,
): Promise<boolean | undefined> {
  if (!context.hasUI || signal.aborted || refused()) return undefined;
  const answer = await context.ui.select(
    `corral: record gate attribution?\n${proposal.summary}`,
    [TRIAGE_RECORD, TRIAGE_DECLINE, TRIAGE_LATER], { signal, timeout: TRIAGE_TIMEOUT_MS },
  );
  // The dialog may close as the turn or planning admission ends; that answer is not the owner's.
  if (signal.aborted || refused()) return undefined;
  return answer === TRIAGE_RECORD ? true : answer === TRIAGE_DECLINE ? false : undefined;
}
