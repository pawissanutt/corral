// Model-facing text only. Canonical receipts and script return values are never rewritten.
import { z } from "zod";
import { CORRAL_OPS } from "../roles/types.ts";
import type { CorralReceipt } from "./corral.ts";

export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** Keep prose/code literal; nested data retains an unambiguous JSON representation. */
export function renderFields(value: unknown): string {
  if (typeof value === "string") return value;
  if (!record(value)) return JSON.stringify(value) ?? "";
  return Object.entries(value).filter(([, item]) => item !== undefined).map(([key, item]) => {
    const label = /^[a-zA-Z_][a-zA-Z_0-9]*$/.test(key) ? key : JSON.stringify(key);
    return `${label}: ${typeof item === "string" ? item.includes("\n") ? `\n${item}` : item : JSON.stringify(item)}`;
  }).join("\n");
}

function remaining(value: Record<string, unknown>, keys: readonly string[]): string {
  return renderFields(Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key))));
}

/** Compact only the known ledger structure; preserve additional fields and unfamiliar shapes. */
function renderProgress(value: unknown): string {
  if (!record(value) || !Array.isArray(value.phases)) return renderFields(value);
  const lines = [remaining(value, ["phases"])];
  for (const phase of value.phases) {
    if (!record(phase) || typeof phase.id !== "string" || typeof phase.label !== "string" || !Array.isArray(phase.steps)) {
      return renderFields(value);
    }
    lines.push(`${phase.id}: ${phase.label}`, remaining(phase, ["id", "label", "steps"]));
    for (const step of phase.steps) {
      if (!record(step) || typeof step.id !== "string" || typeof step.label !== "string" || !record(step.status)
        || typeof step.status.state !== "string") return renderFields(value);
      lines.push(`- ${step.id} [${step.status.state}]: ${step.label}`,
        remaining(step.status, ["state"]), remaining(step, ["id", "label", "status", ...(Array.isArray(step.criteria)
          && step.criteria.length === 0 ? ["criteria"] : [])]));
    }
  }
  return lines.filter(Boolean).join("\n");
}

export function renderReceipt(receipt: CorralReceipt): string {
  // A future protocol/operation must not silently lose fields through an older presentation.
  if (receipt.version !== 1 || !CORRAL_OPS.some(op => op === receipt.op)) return JSON.stringify(receipt);
  const envelope = remaining(receipt, ["version", "op", "state", "request_id", "result", "replayed",
    "identity", "payload_sha256", "at", "committed_at"]);
  const result = receipt.op === "todo" && record(receipt.result)
    ? Object.entries(receipt.result).map(([key, item]) => key === "progress"
      ? `progress:\n${renderProgress(item)}` : renderFields({ [key]: item })).join("\n")
    : renderFields(receipt.result);
  return [`${receipt.op}: ${receipt.state}${receipt.replayed ? " (replayed)" : ""}`,
    `request_id: ${receipt.request_id}`, envelope, result].filter(Boolean).join("\n");
}

const CanonicalReceipt = z.object({ version: z.literal(1), request_id: z.string().nullable(), op: z.string(),
  state: z.enum(["committed", "rejected", "reserved", "queued", "uncertain", "abandoned"]),
  result: z.unknown(), replayed: z.boolean().optional() }).passthrough();

/** Details establish visibility only when their entire deterministic rendering is still present. */
export function visibleReceipt(text: string, details: unknown): CorralReceipt | undefined {
  const canonical = CanonicalReceipt.safeParse(details);
  return canonical.success && text === renderReceipt(canonical.data) ? canonical.data : undefined;
}

export function renderFailure(op: string, requestId: string, error: Record<string, unknown>): string {
  return `${op}: host error\nrequest_id: ${requestId}\n${renderFields(error)}`;
}
