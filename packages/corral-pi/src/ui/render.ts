// Compact transcript rendering for corral_* tools (rich UI). A call is one line: the tool's short
// name and the one argument that says what it does; a result is one line: the receipt state and
// the few result fields a person acts on. The raw receipt is shown only when the view is expanded.
// Rendering reads the call and its receipt only; it never changes what the tool sends or returns.
import type { AgentToolResult, Theme, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Text } from "@earendil-works/pi-tui";

import type { CorralReceipt } from "../tools/corral.ts";

type Style = Pick<Theme, "fg" | "bold">;
type Args = Record<string, unknown>;

const SUMMARY_MAX = 96;
const EXPANDED_MAX_LINES = 40;
const SUMMARY_ARGS = [
  "text", "title", "path", "verdict", "condition", "query", "task", "key", "summary", "reason", "section", "status",
];
const RESULT_FIELDS = [
  "key", "id", "attempt", "tier", "verdict", "status", "state", "ref", "artifact", "task", "count", "total",
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

export function renderCallText(name: string, args: Args, theme: Style): string {
  const summary = callSummary(args ?? {});
  const head = theme.fg("toolTitle", theme.bold(`corral ${shortName(name)}`));
  return summary ? `${head} ${theme.fg("muted", summary)}` : head;
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
  const shown = lines.slice(0, EXPANDED_MAX_LINES);
  if (lines.length > shown.length) shown.push(`… ${lines.length - shown.length} more lines`);
  return shown.join("\n");
}

export function renderResultText(
  result: AgentToolResult<unknown>, expanded: boolean, isError: boolean, theme: Style,
): string {
  const receipt = result.details as CorralReceipt | undefined;
  if (isError || !receipt || typeof receipt.state !== "string") {
    // A call that never produced a receipt (spawn failure, timeout, malformed output).
    return theme.fg("error", `✗ ${oneLine(textOf(result) || "corral call failed", 160)}`);
  }
  let line: string;
  switch (receipt.state) {
    case "committed": {
      const fields = resultFields(receipt.result);
      const mark = receipt.replayed ? theme.fg("muted", "↺ replayed") : theme.fg("success", "✓");
      line = fields ? `${mark} ${theme.fg("dim", fields)}` : mark;
      break;
    }
    case "rejected": {
      const why = refusal(receipt.result);
      line = theme.fg("error", why ? `✗ rejected  ${why}` : "✗ rejected");
      break;
    }
    case "reserved":
    case "uncertain":
      line = theme.fg("warning", `◷ ${receipt.state}`) +
        (refusal(receipt.result) ? ` ${theme.fg("dim", refusal(receipt.result) ?? "")}` : "");
      break;
    default:
      line = theme.fg("warning", `◷ ${receipt.state}`);
  }
  return expanded ? `${line}\n${theme.fg("dim", expandedBody(receipt))}` : line;
}

export function corralRenderers(name: string): Pick<ToolDefinition, "renderCall" | "renderResult"> {
  return {
    renderCall: (args, theme) => new Text(renderCallText(name, args as Args, theme), 0, 0),
    renderResult: (result, options, theme, context) =>
      new Text(renderResultText(result, options.expanded, context.isError, theme), 0, 0),
  };
}
