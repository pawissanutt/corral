// Which presenter draws which tool, and the one renderer resolver the rich UI registers with Pi. Presenter modules
// are asked in order; the first that returns a presenter for a tool name wins, and the fallback presenter draws any
// tool without one. Build cards keep their own renderers. Nested rows (Code Mode children) use `presenterFor` and
// `rowLine` directly with the same deps.
import type { ExtensionAPI, ToolRenderers } from "@earendil-works/pi-coding-agent";

import type { CodeRuntime } from "../extensions/codemode/runtime.ts";
import type { JobInspection } from "../extensions/jobs/runtime.ts";
import type { CorralToolSpec } from "../roles/types.ts";
import type { PanelView } from "./panels.ts";
import type { TodoLedger } from "./todo.ts";
import { buildPresenters } from "./build/build-presenters.ts";
import { codeCellRenderers } from "./code-cell.ts";
import { corralPresenters } from "./corral-presenter.ts";
import { groupingFor } from "./tool-group.ts";
import { filePresenters } from "./tool-lines-files.ts";
import { jobPresenters } from "./tool-lines-jobs.ts";
import { localPresenters } from "./tool-lines-local.ts";
import {
  baseState, firstLine, lineCount, numberArg, outputLines, recordOf, resultText, rowRenderers, shortPath, stringArg, tailLines, textLines,
} from "./tool-row.ts";
import type { BodyLine, CallInput, Presenter, RowDeps, RowModel } from "./tool-row.ts";

/** What a job row needs to know about a job this host owns (`lines`: finished output lines, when known). */
export type JobFacts = Pick<JobInspection, "job" | "command" | "state" | "startedAt" | "endedAt" | "exitCode" | "signal" | "tail" | "lines">;

/** Host state presenters may read. Getters are read at render time, so a row follows live state without a new tool event. */
export interface ToolRenderDeps extends RowDeps {
  /** The session's working directory: paths inside it are shown relative to it. */
  readonly cwd: string;
  /** Live facts of an owned job by id; undefined when this host does not know it (another generation, pruned). */
  job(id: string): JobFacts | undefined;
  /** The generation's Code Mode runtime, when Code Mode is enabled. */
  code(): CodeRuntime | undefined;
  /** The host's todo ledger, when `corral_todo` is loaded. */
  ledger(): TodoLedger | undefined;
  /** The role's corral tool specs (their names are the corral operations). */
  readonly corralSpecs: readonly CorralToolSpec[];
  /** Opens a workbench view; `select` names a row (`job:<id>` in Agents, a path in Changes). Absent without panels. */
  readonly openView?: (view: PanelView, select?: string) => void;
}

/**
 * A group of presenters, e.g. the job tools. Returns undefined for tool names it does not draw. `label` is the tool's
 * registered label when the resolver knows it (MCP tools read as `server: title`).
 */
export interface PresenterModule { presenter(name: string, deps: ToolRenderDeps, label?: string): Presenter | undefined }

/** Presenter modules in the order they are asked; jobs come before corral so `corral_job_monitor` reads as a job row. */
export const PRESENTER_MODULES: readonly PresenterModule[] = [filePresenters, jobPresenters, corralPresenters, localPresenters, buildPresenters];

/** Tools that draw their own cards: their registered renderers stay. */
const OWN_CARDS: Readonly<Record<string, true>> = { task: true, build_finish: true, board_plan: true };

/** The presenter for tool `name`: the first module's, else the fallback. `label` is the tool's registered label. */
export function presenterFor(name: string, deps: ToolRenderDeps, label?: string): Presenter {
  for (const module of PRESENTER_MODULES) {
    const presenter = module.presenter(name, deps, label);
    if (presenter) return presenter;
  }
  return fallbackPresenter(name, label);
}

/**
 * Registers the one renderer resolver of the rich UI: every tool call is drawn as a kit row, except the build cards,
 * whose registered renderers stay, and Code Mode cells, which draw a group of child rows. Kit rows fold into group
 * rows by kind (tool-group.ts); build cards and Code Mode cells never fold. Code Mode's import of `presenterFor`
 * back from this module is used only at render time, so the cycle is safe.
 */
export function installToolRenderers(pi: Pick<ExtensionAPI, "registerToolRenderer">, deps: ToolRenderDeps): void {
  pi.registerToolRenderer((name, next) => {
    if (OWN_CARDS[name] === true) return next();
    if (name === "codemode") return codeCellRenderers(deps);
    return rowRenderers(presenterFor(name, deps, labelOf(next())), deps, name, groupingFor(name));
  });
}

function labelOf(renderers: ToolRenderers | undefined): string | undefined {
  return renderers && "label" in renderers && typeof renderers.label === "string" ? renderers.label : undefined;
}

// ---------------------------------------------------------------------------------------------------------------
// Fallback: any tool without a presenter

/** The verb of a tool without a presenter: an MCP tool's registered label (`server: title`), else its name in words. */
function fallbackVerb(name: string, label: string | undefined): string {
  if (name.startsWith("mcp_") && label) return label;
  return name.replace(/^mcp_/, "").replaceAll("_", " ");
}
const TARGET_KEYS = ["command", "path", "file_path", "pattern", "query", "url", "prompt", "question", "job", "content_id",
  "document", "server", "uri", "fact", "id", "text", "title", "summary"];

/** The one argument that says what the call does: a path (with its line), the first question, or the first known key. */
export function fallbackTarget(args: Readonly<Record<string, unknown>>, cwd: string): { readonly text: string; readonly path: boolean } | undefined {
  const questions = args["questions"];
  if (Array.isArray(questions) && questions.length) {
    const head: unknown = questions[0], first = recordOf(head);
    const text = (first && (stringArg(first, "header") ?? stringArg(first, "question"))) ?? (typeof head === "string" ? head : "");
    return { text: text + (questions.length > 1 ? ` + ${questions.length - 1} more` : ""), path: false };
  }
  const path = stringArg(args, "path") ?? stringArg(args, "file_path"), pattern = stringArg(args, "pattern");
  if (pattern) return { text: `"${pattern}"${path ? ` in ${shortPath(path, cwd)}` : ""}`, path: false };
  if (path) {
    const line = numberArg(args, "line"), column = numberArg(args, "column");
    return { text: shortPath(path, cwd) + (line === undefined ? "" : `:${line}${column === undefined ? "" : `:${column}`}`), path: true };
  }
  for (const key of TARGET_KEYS) {
    const value = args[key];
    if (typeof value === "string" && value.trim()) return { text: value, path: false };
  }
  const loose = Object.values(args).find((value): value is string => typeof value === "string" && value.trim() !== "");
  return loose === undefined ? undefined : { text: loose, path: false };
}

const SUMMARY_KEYS = ["status", "state", "title", "total", "count", "matched", "saved", "configured", "next_offset"];
/** A JSON result in words: an error's message, or a few scalar fields as `key value`. */
export function jsonSummary(value: unknown): { readonly error?: string; readonly words: string } {
  if (Array.isArray(value)) return { words: `${value.length} ${value.length === 1 ? "item" : "items"}` };
  const record = recordOf(value);
  if (!record) return { words: String(value) };
  const error = record["error"], errorRecord = recordOf(error);
  if (typeof error === "string" || errorRecord) {
    const message = typeof error === "string" ? error : [errorRecord?.["code"], errorRecord?.["message"]].filter(part => typeof part === "string").join(": ");
    return { error: message || "error", words: message || "error" };
  }
  const parts = SUMMARY_KEYS.flatMap(key => {
    const item = record[key];
    if (key === "configured" && item === false) return ["not configured"];
    if (key === "next_offset") return typeof item === "number" ? ["more"] : [];
    return typeof item === "string" || typeof item === "number" || typeof item === "boolean" ? [`${key.replaceAll("_", " ")} ${String(item)}`] : [];
  }).slice(0, 3);
  for (const [key, item] of Object.entries(record)) {
    if (parts.length >= 3) break;
    if (Array.isArray(item) && !SUMMARY_KEYS.includes(key)) parts.push(`${item.length} ${key.replaceAll("_", " ")}`);
  }
  return { words: parts.join(" · ") || `${Object.keys(record).length} fields` };
}
/** `text` parsed as JSON when it is one JSON object or array, else undefined. */
export function parsedJson(text: string): { readonly value: unknown } | undefined {
  const trimmed = text.trim();
  if (!/^[[{]/.test(trimmed)) return undefined;
  try { return { value: JSON.parse(trimmed) }; } catch { return undefined; }
}

/**
 * Any tool: readable verb, the argument that says what it does, the result as facts. JSON is summarised, never dumped;
 * text output stays behind the expand hint, so a collapsed row never prints what the tool returned.
 */
export function fallbackPresenter(name: string, label?: string): Presenter {
  const verb = fallbackVerb(name, label);
  return (call: CallInput): RowModel => {
    const state = baseState(call), target = fallbackTarget(call.args, call.cwd);
    const base = { verb, ...(target ? { target: target.text, targetClip: target.path ? "middle" as const : "end" as const } : {}) };
    if (state === "writing" || state === "running") return { ...base, state };
    const text = resultText(call.result);
    const json = parsedJson(text), summary = json ? jsonSummary(json.value) : undefined;
    const full = json ? JSON.stringify(json.value, null, 2) : text;
    const expanded = (): readonly BodyLine[] => textLines(full);
    if (state === "failed" || summary?.error !== undefined) {
      const reason = summary?.error ?? firstLine(text);
      const rest = json ? [] : text.split("\n").filter(line => line.trim() !== "").slice(1);
      const more = call.totals ? (call.totals.lines === undefined ? undefined : call.totals.lines - 1 - Math.min(3, rest.length)) : rest.length - 3;
      return { ...base, state: "failed", outcome: reason || "failed", ...(rest.length ? { body: tailLines(rest.join("\n"), 3) } : {}),
        ...(more !== undefined && more > 0 ? { hiddenHint: `${more} more lines` } : {}), ...(rest.length > 3 || call.totals ? { expanded } : {}) };
    }
    const lines = json ? lineCount(full) : outputLines(call, text);
    const outcome = summary?.words ?? (lines === undefined ? "text" : lines ? `${lines} ${lines === 1 ? "line" : "lines"}` : "no output");
    return { ...base, state, outcome, ...(full.trim() ? { expanded } : {}) };
  };
}
