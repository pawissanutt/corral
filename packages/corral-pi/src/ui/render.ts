// Compact transcript rendering for corral_* tools (rich UI), as one tool line: the state glyph (the spinner
// while the call runs, then the receipt's), the tool's short name in accent, the one argument that says what it
// does, and after a dot the few result fields a person acts on. The raw receipt is shown only when the view is
// expanded. Rendering reads the call and its receipt only; it never changes what the tool sends or returns.
import type { AgentToolResult, Theme, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Container, Text, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Component } from "@earendil-works/pi-tui";

import type { CorralReceipt } from "../tools/corral.ts";
import { spinner } from "./frame.ts";

type Style = Pick<Theme, "fg">;
type Args = Record<string, unknown>;

const SUMMARY_MAX = 96;
const EXPANDED_MAX_LINES = 40;
const SUMMARY_ARGS = [
  "text", "title", "path", "verdict", "condition", "query", "task", "key", "summary", "reason", "section", "status",
];
const RESULT_FIELDS = [
  "key", "id", "attempt", "tier", "verdict", "status", "state", "ref", "artifact", "task", "count", "total", "revision",
];

export function shortName(name: string): string {
  return name.replace(/^corral_/, "").replaceAll("_", " ");
}

function oneLine(value: string, max = SUMMARY_MAX): string {
  const flat = value.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

export function callSummary(args: Args): string | undefined {
  const kind = typeof args.kind === "string" ? args.kind : undefined;
  for (const key of SUMMARY_ARGS) {
    const value = args[key];
    if (typeof value === "string" && value.trim() !== "") {
      return kind && key !== "kind" ? `${kind}: ${oneLine(value)}` : oneLine(value);
    }
  }
  // ProgressUpdate, CheckpointUpdate and EvidenceObservation arrive as one object argument.
  for (const value of Object.values(args)) {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      const nested = callSummary(value as Args);
      if (nested) return nested;
    }
  }
  return kind;
}

/** `corral ask` in accent, then what the call does. */
export function renderCallText(name: string, args: Args, theme: Style): string {
  const summary = name === "corral_todo" && typeof args.op === "string"
    ? oneLine([args.op, args.step ?? args.phase].filter((value) => typeof value === "string").join(" "))
    : name === "corral_show" ? showSummary(args ?? {}) : callSummary(args ?? {});
  const head = theme.fg("toolTitle", `corral ${shortName(name)}`);
  return summary ? `${head} ${theme.fg("text", summary)}` : head;
}

function showSummary(args: Args): string {
  const section = typeof args.section === "string" ? args.section : "overview";
  const label = typeof args.key === "string" ? `Question ${args.key}`
    : section.charAt(0).toUpperCase() + section.slice(1);
  const range = args.offset !== undefined || args.limit !== undefined
    ? ` · from ${args.offset ?? 0}${args.limit !== undefined ? `, ≤${args.limit} chars` : ""}` : "";
  return `${args.task ?? "task"} · ${label}${range}`;
}

function scalar(value: unknown): string | undefined {
  if (typeof value === "string" && value !== "") return oneLine(value, 48);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return undefined;
}

export function resultFields(result: unknown, limit = 3): string | undefined {
  if (typeof result === "string") return oneLine(result);
  if (!result || typeof result !== "object" || Array.isArray(result)) return undefined;
  const record = result as Record<string, unknown>;
  if (typeof record.content_json === "string" && typeof record.total_chars === "number") {
    const range = `${record.offset ?? 0}–${record.end_offset ?? record.content_json.length}/${record.total_chars} chars`;
    return `${record.section ?? "read"} · ${range} · ${record.next_offset == null ? "complete" : `next=${record.next_offset}`}`
      + (record.revision != null ? ` · rev=${record.revision}` : "")
      + (typeof record.diagnostic === "string" ? ` · ${oneLine(record.diagnostic)}` : "");
  }
  const parts: string[] = [];
  for (const key of RESULT_FIELDS) {
    const value = scalar(record[key]);
    if (value !== undefined) parts.push(`${key}=${value}`);
    if (parts.length >= limit) break;
  }
  return parts.length > 0 ? parts.join(" · ") : undefined;
}

function refusal(result: unknown): string | undefined {
  if (!result || typeof result !== "object") return typeof result === "string" ? oneLine(result) : undefined;
  const record = result as Record<string, unknown>;
  const code = scalar(record.code);
  const reason = typeof record.reason === "string" ? oneLine(record.reason)
    : typeof record.message === "string" ? oneLine(record.message) : undefined;
  return [code, reason].filter(Boolean).join(": ") || undefined;
}

function textOf(result: AgentToolResult<unknown>): string {
  const first = result.content.find((block) => block.type === "text");
  return first && first.type === "text" ? first.text : "";
}

function expandedBody(receipt: CorralReceipt): string {
  const lines = JSON.stringify(receipt.result ?? null, null, 2).split("\n");
  const shown = typeof (receipt.result as Args | null)?.content_json === "string"
    ? lines : lines.slice(0, EXPANDED_MAX_LINES);
  if (lines.length > shown.length) shown.push(`… ${lines.length - shown.length} more lines`);
  return shown.join("\n");
}

/** A receipt's outcome: its glyph, words in the glyph's tone, and detail drawn dim. */
interface Outcome { readonly glyph: string; readonly tone: "success" | "muted" | "error" | "warning"; readonly words?: string; readonly detail?: string }
function outcome(result: AgentToolResult<unknown>, isError: boolean): Outcome {
  const receipt = result.details as CorralReceipt | undefined;
  if (isError || !receipt || typeof receipt.state !== "string") {
    // A call that never produced a receipt (spawn failure, timeout, malformed output).
    return { glyph: "✗", tone: "error", words: oneLine(textOf(result) || "corral call failed", 160) };
  }
  switch (receipt.state) {
    case "committed": {
      const fields = resultFields(receipt.result), detail = fields ? { detail: fields } : {};
      return receipt.replayed ? { glyph: "↺", tone: "muted", words: "replayed", ...detail } : { glyph: "✓", tone: "success", ...detail };
    }
    case "rejected": {
      const why = refusal(receipt.result);
      return { glyph: "✗", tone: "error", words: why ? `rejected  ${why}` : "rejected" };
    }
    case "reserved":
    case "uncertain": {
      const why = refusal(receipt.result);
      return { glyph: "◷", tone: "warning", words: receipt.state, ...(why ? { detail: why } : {}) };
    }
    default:
      return { glyph: "◷", tone: "warning", words: receipt.state };
  }
}
/** The raw receipt, for the expanded view; undefined for a call without a receipt. */
function receiptBody(result: AgentToolResult<unknown>, isError: boolean): string | undefined {
  const receipt = result.details as CorralReceipt | undefined;
  return isError || !receipt || typeof receipt.state !== "string" ? undefined : expandedBody(receipt);
}

/** The receipt as one line (`✓ key=Q-3 · tier=quick`), and the raw receipt under it when `expanded`. */
export function renderResultText(
  result: AgentToolResult<unknown>, expanded: boolean, isError: boolean, theme: Style,
): string {
  const shown = outcome(result, isError), body = expanded ? receiptBody(result, isError) : undefined;
  const line = theme.fg(shown.tone, shown.words ? `${shown.glyph} ${shown.words}` : shown.glyph)
    + (shown.detail ? ` ${theme.fg("dim", shown.detail)}` : "");
  return body === undefined ? line : `${line}\n${theme.fg("dim", body)}`;
}

/** One line drawn at render time, `inset` cells in from both sides and clipped with `…`; nothing when `text` is empty. */
class ToolLine implements Component {
  constructor(private readonly text: () => string, private readonly inset = 0) {}
  render(width: number): string[] {
    const line = this.text();
    if (!line) return [];
    const pad = Math.max(0, Math.min(this.inset, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
    return [" ".repeat(pad) + (visibleWidth(line) > room ? truncateToWidth(line, room, "…") : line)];
  }
  invalidate(): void {}
}

/**
 * The tool draws its own frame (`renderShell: "self"`): Pi puts one blank row above the line and no box around it,
 * and the line keeps the output padding free itself, so it starts in the column of the rest of the transcript.
 */
export function corralRenderers(name: string): Pick<ToolDefinition, "renderShell" | "renderCall" | "renderResult"> {
  return {
    renderShell: "self",
    // While the call runs its line carries the spinner; once the receipt arrives the result draws the line instead.
    renderCall: (args, theme, context) => new ToolLine(() => context.isPartial
      ? `${theme.fg("accent", spinner(Date.now()))} ${renderCallText(name, args as Args, theme)}` : "", context.outputPad),
    renderResult: (result, options, theme, context) => {
      if (options.isPartial) return new ToolLine(() => "");
      const shown = outcome(result, context.isError);
      const after = [shown.words ? theme.fg(shown.tone, shown.words) : "", shown.detail ? theme.fg("dim", shown.detail) : ""].filter(Boolean).join(" ");
      const line = new ToolLine(() => `${theme.fg(shown.tone, shown.glyph)} ${renderCallText(name, (context.args ?? {}) as Args, theme)}`
        + (after ? `${theme.fg("muted", " · ")}${after}` : ""), context.outputPad);
      const body = options.expanded ? receiptBody(result, context.isError) : undefined;
      if (body === undefined) return line;
      const both = new Container();
      both.addChild(line);
      both.addChild(new Text(theme.fg("dim", body), Math.max(0, context.outputPad), 0));
      return both;
    },
  };
}
