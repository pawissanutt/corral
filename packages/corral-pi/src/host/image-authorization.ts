import { classifyWriteScope, type WriteScopeDecision } from "../roles/assistant/write-scope.ts";
import { resolveToolPath } from "../roles/assistant/paths.ts";
import type { ToolApprovals } from "./tool-approvals.ts";

/** Classify the exact path pinned by the publisher, never a separately resolved destination. */
export function authorizeImageDestination(
  path: string, prompt: string, preparedPath: string, identity: () => string,
  scope: Parameters<typeof classifyWriteScope>[1], approvals?: ToolApprovals,
  classify: typeof classifyWriteScope = classifyWriteScope,
): () => void {
  if (resolveToolPath(preparedPath, scope.cwd) !== preparedPath) {
    throw new Error("image_generation_unavailable: unsupported destination spelling");
  }
  const decisionForPreparedPath = (): WriteScopeDecision => {
    const decision = classify(preparedPath, scope);
    if (decision.action === "ask" && decision.canonical !== preparedPath) {
      throw new Error("Image destination changed since preparation; nothing was written.");
    }
    return decision;
  };
  const decision = decisionForPreparedPath();
  const reason = approvals ? approvals.write("image_gen", { path, prompt }, decision, decision.action === "ask" ? identity() : undefined)
    : decision.action === "deny" ? decision.reason
    : decision.action === "ask" ? "An owner approval interface is unavailable; nothing was written." : undefined;
  if (reason) throw new Error(reason);
  return () => {
    const current = decisionForPreparedPath();
    if (JSON.stringify(current) !== JSON.stringify(decision)) throw new Error("Image destination changed since authorization; nothing was written.");
  };
}
