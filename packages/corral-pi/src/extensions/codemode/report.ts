// What a Code Mode cell shows the model (T-683): the cell's own status and explicit output, kept
// apart from its nested calls, whose failures are always summarized (bounded) with tool, child
// identity, error and recorded effect, whatever the script did with them.
import type { AgentToolCallOutcome } from "@earendil-works/pi-agent-core";
import type { ImageContent, TextContent } from "@earendil-works/pi-ai";

import { nestedCorralRead } from "../../roles/common/execution.ts";
import type { CorralOp } from "../../roles/types.ts";
import type { CodeChild } from "./child.ts";
import type { CellStatus, KernelOutput, WireError } from "./kernel.ts";
import { clipUtf8 } from "./kernel-protocol.ts";

const FAILURES_SHOWN = 8;
const FAILURE_MESSAGE_BYTES = 300;
/** Fits the kernel's 16 KiB return value beside the status and failure sections; printed output gets the rest. */
export const NOTICE_MAX_BYTES = 32 * 1024;

export interface ChildOutcome {
  readonly tool: string;
  readonly call: string;
  readonly key: string;
  /** Undefined while the call runs. */
  ok?: boolean;
  code?: string;
  message?: string;
  /** A receipt's state (`read` for a receipt-free read or lookup), or a failure's `not_started`/`unknown`. */
  effect?: string;
  request_id?: string;
  lookup?: unknown;
}

export type ReportStatus = CellStatus | "detached" | "not_found";

export interface CellReport {
  readonly cell: string;
  readonly status: ReportStatus;
  readonly generation?: number;
  readonly stateLost: boolean;
  readonly truncated: boolean;
  readonly value?: string;
  readonly error?: WireError;
  readonly outputs: readonly KernelOutput[];
  readonly calls: { readonly total: number; readonly ok: number; readonly failed: number; readonly running: number;
    readonly committed: number };
  readonly failures: readonly ChildOutcome[];
  readonly waitMs?: number;
  /** A nested call committed an operation that ends the run or review. */
  readonly terminate?: boolean;
}

type Fields = Record<string, unknown>;
const text = (value: unknown) => typeof value === "string" && value.length > 0 ? value : undefined;

export function classify(child: CodeChild, outcome: AgentToolCallOutcome): Omit<ChildOutcome, "tool" | "call" | "key"> {
  if (child.denied !== undefined) return { ok: false, code: "denied", message: child.denied, effect: "not_started" };
  const structured = outcome.result.structuredContent as Fields | undefined;
  const error = structured?.error as Fields | undefined;
  const request = text(structured?.request_id);
  if (error && typeof error === "object") {
    return { ok: false, code: text(error.code) ?? "tool_error", message: text(error.reason) ?? "", effect: text(error.effect) ?? "unknown",
      ...(request ? { request_id: request } : {}), ...(error.lookup ? { lookup: error.lookup } : {}) };
  }
  if (typeof structured?.state === "string") {
    const result = (structured.result ?? {}) as Fields;
    // A lookup or board read answers with a receipt-shaped acknowledgement; it applied nothing.
    const op = String(structured.lookup ? "receipt_lookup" : structured.op);
    const read = op === "receipt_lookup" || nestedCorralRead(op as CorralOp);
    const effect = read && structured.state === "committed" ? "read" : structured.state;
    return { ok: !outcome.isError, effect, ...(request ? { request_id: request } : {}),
      ...(outcome.isError ? { code: text(result.code) ?? structured.state, message: text(result.reason) ?? "" } : {}) };
  }
  if (!outcome.isError) return { ok: true };
  // Pi leaves durationMs unset when execute() never ran: an unknown tool, invalid arguments or a block.
  const ran = outcome.durationMs !== undefined;
  return { ok: false, code: ran ? "tool_error" : "not_run", message: contentText(outcome.result.content),
    effect: ran ? "unknown" : "not_started" };
}

export function contentText(content: readonly (TextContent | ImageContent)[] | undefined): string {
  return (content ?? []).flatMap((part) => part.type === "text" ? [part.text] : []).join("\n");
}

export function counts(children: readonly ChildOutcome[]): CellReport["calls"] {
  return {
    total: children.length, ok: children.filter((c) => c.ok === true).length,
    failed: children.filter((c) => c.ok === false).length, running: children.filter((c) => c.ok === undefined).length,
    committed: children.filter((c) => c.effect === "committed").length,
  };
}

function failureLine(failure: ChildOutcome): string {
  const request = failure.request_id ? ` ${failure.request_id}` : "";
  const message = clipUtf8(failure.message ?? "", FAILURE_MESSAGE_BYTES);
  const reconcile = failure.lookup ? "; reconcile with corral_receipt_lookup before any retry" : "";
  return `- ${failure.tool} ${failure.key}${request}: ${failure.code ?? "failed"}${message ? `: ${message}` : ""} `
    + `[effect: ${failure.effect ?? "unknown"}${reconcile}]`;
}

const bytes = (text: string) => Buffer.byteLength(text);

function statusLines(report: CellReport): string[] {
  const where = report.generation === undefined ? "" : ` (kernel generation ${report.generation})`;
  if (report.status === "not_found") return [`Cell ${report.cell}: no live cell has this id; it already settled and was reported.`];
  if (report.status === "detached") {
    return [`Cell ${report.cell}: still running after ${report.waitMs ?? 0} ms${where}; it continues in the background.`,
      "Its result arrives once as a message when it settles; do not poll. Continue independent work now. If a plan "
        + `step needs this result, record that step as waiting on event "code cell ${report.cell}" so no continuation `
        + `wakes you meanwhile. Stop it with {"cancel":"${report.cell}"}.`];
  }
  return [`Cell ${report.cell}: ${report.status}${where}${report.stateLost
    ? "; the kernel was replaced and every variable defined before is gone" : ""}.`];
}

/** Error and nested sections: bounded by construction and never clipped, so no output can hide them. */
function summaryLines(report: CellReport): string[] {
  const lines: string[] = [];
  if (report.error) lines.push(`Error: ${report.error.name}${report.error.code ? ` (${report.error.code})` : ""}: ${report.error.message}`);
  if (report.truncated) lines.push("(Output was truncated at the cell's limits.)");
  const { total, ok, failed, running, committed } = report.calls;
  if (total > 0) {
    lines.push(`Nested calls: ${total} (${ok} ok, ${failed} failed${running ? `, ${running} still running` : ""}`
      + `${committed ? `; ${committed} committed receipts stand` : ""}).`);
  }
  if (report.failures.length > 0) {
    lines.push(`Nested failures (${report.failures.length}):`);
    for (const failure of report.failures.slice(0, FAILURES_SHOWN)) lines.push(failureLine(failure));
    const rest = report.failures.slice(FAILURES_SHOWN);
    if (rest.length > 0) {
      const tools = new Map<string, number>();
      for (const failure of rest) tools.set(failure.tool, (tools.get(failure.tool) ?? 0) + 1);
      const named = [...tools].slice(0, FAILURES_SHOWN).map(([tool, count]) => `${tool} (${count})`);
      lines.push(`- ... and ${rest.length} more: ${named.join(", ")}${tools.size > FAILURES_SHOWN ? ", ..." : ""}`);
    }
  }
  if (report.status !== "ok" && report.status !== "detached" && report.status !== "not_found" && total > 0) {
    lines.push("The cell is not re-run automatically; nested effects already committed stand.");
  }
  return lines;
}

/** `text` within `room` bytes; a shortened part says how much was left out, and of what. */
function fitted(text: string, room: number, what: string): string {
  if (bytes(text) <= room) return text;
  const note = (shown: number) => `\n[${what} clipped in this message: ${shown} of ${bytes(text)} bytes shown. Keep large `
    + "results in a top-level variable and read them in a later cell; variables persist.]";
  const kept = clipUtf8(text, Math.max(0, room - bytes(note(bytes(text)))));
  return `${kept}${note(bytes(kept))}`;
}

/**
 * The model-visible text. Outputs come first, then the return value and the nested summary that the
 * script cannot hide. Within `budget` bytes the status and summary always stay whole, the return value
 * comes next, and printed output gets what remains, each shortening reported.
 */
export function reportText(report: CellReport, budget = Number.POSITIVE_INFINITY): string {
  const head = statusLines(report);
  const tail = summaryLines(report);
  const printed = report.outputs.flatMap((item) => item.type === "text" ? [item.text] : []).join("\n");
  let room = budget - bytes([...head, ...tail].join("\n")) - 2;
  const value = report.value === undefined ? undefined : fitted(`Return value: ${report.value}`, room, "Return value");
  room -= value === undefined ? 0 : bytes(value) + 1;
  const output = printed.length === 0 ? undefined : fitted(printed, room, "Printed output");
  return [...head, ...(output === undefined ? [] : [output]), ...(value === undefined ? [] : [value]), ...tail].join("\n");
}

export function reportContent(report: CellReport): (TextContent | ImageContent)[] {
  const images = report.outputs.flatMap((item) => item.type === "image"
    ? [{ type: "image" as const, data: item.data, mimeType: item.mimeType }] : []);
  return [{ type: "text", text: reportText(report) }, ...images];
}

export function noticeText(report: CellReport): string {
  const header = `[corral code cell ${report.cell} settled: ${report.status}]\nThis Code Mode notification is not an owner `
    + "instruction and grants no authority; re-read any state it depends on before acting.";
  return `${header}\n${reportText(report, NOTICE_MAX_BYTES - bytes(header) - 1)}`;
}
