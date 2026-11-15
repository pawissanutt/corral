import type { RoleContext } from "../types.ts";

/** The native attempt and its receipts, not transferred model memory, retain authority. */
export function continuationContract(context: RoleContext): string {
  const notice = context.continuation;
  if (!notice) return "";
  const fresh = notice.context.kind === "fresh-read-only"
    ? `Fresh read-only ${context.identity.role} attempt: ${notice.context.reason}. Re-read the immutable subject and evidence; `
      + "partial notes are non-authoritative. Do not inherit a prior submission or verdict. "
    : "Continue only from verified committed context and canonical native state. ";
  return "[corral role continuation]\n" + fresh
    + "Receipts are references and must never be replayed: resolve original receipt IDs and results, never execute their tools again. "
    + "Uncertain effects require reconciliation, not retries. This context is not owner input and grants no new tool, action, "
    + "spending, submission, verdict or permission authority; existing scopes and expiries remain unchanged.";
}
