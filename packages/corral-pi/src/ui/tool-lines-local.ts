// Rows for the local tools that are neither files, commands, corral operations nor Code Mode: language server
// queries, web and browser tools, owner questions, research helpers, tool search and MCP. Each reads as its verb, the
// one argument that says what it does, and the result as facts; one-line JSON results are parsed here, never shown.
// Only types come from `tool-rendering.ts`, which imports this module's presenters: no load-order cycle.
import {
  baseState, firstLine, formatElapsed, lineCount, numberArg, outputBytes, outputLines, recordOf, resultText, shortPath, stringArg, textLines,
} from "./tool-row.ts";
import type { BodyLine, CallInput, Presenter, RowModel } from "./tool-row.ts";
import type { PresenterModule, ToolRenderDeps } from "./tool-rendering.ts";

type Base = Omit<RowModel, "state">;
type Json = Readonly<Record<string, unknown>>;

/** Every registered local tool this module draws (MCP server tools, named `mcp_<server>_<tool>_<hash>`, too). */
export const LOCAL_TOOL_NAMES = [
  "lsp_diagnostics", "lsp_definition", "lsp_references", "web_fetch", "web_read", "web_search", "browser_navigate",
  "browser_screenshot", "ask_user", "ask_owner", "explore", "oracle", "tool_search", "mcp_connect", "mcp_resources",
  "mcp_read_resource", "mcp_activate", "read_review_snapshot",
] as const;

function jsonOf(text: string): unknown {
  const trimmed = text.trim();
  if (!/^[[{]/.test(trimmed)) return undefined;
  try { return JSON.parse(trimmed); } catch { return undefined; }
}
function list(value: unknown): Json[] {
  return Array.isArray(value) ? value.flatMap(item => { const entry = recordOf(item); return entry ? [entry] : []; }) : [];
}
function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}
/** `12 lines` from the real total (a clipped preview without one says only `text`). */
function linesWords(call: CallInput, output: string): string {
  const lines = outputLines(call, output);
  return lines === undefined ? "text" : lines === 0 ? "no output" : plural(lines, "line");
}
/** `512 B`, `9.3 KB`, `1.1 MB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
function text(value: Json | undefined, key: string): string | undefined {
  const item = value?.[key];
  return typeof item === "string" ? item : undefined;
}
function count(value: Json | undefined, key: string): number | undefined {
  const item = value?.[key];
  return typeof item === "number" && Number.isFinite(item) ? item : undefined;
}
/** A URL as the owner recognises it: without its scheme, and a `file://` URL as a path relative to the session. */
function site(url: string, cwd: string): string {
  if (url.startsWith("file://")) return shortPath(decodeURIComponent(url.slice("file://".length)), cwd);
  return url.replace(/^https?:\/\//, "").replace(/\/$/, "");
}
/** The reason a call failed: a JSON `{ error: { code, message } }`, an MCP result's first text block, else the first line. */
function reasonOf(output: string): string {
  const value = recordOf(jsonOf(output)), error = value?.["error"], errorRecord = recordOf(error);
  if (typeof error === "string") return error;
  if (errorRecord) return text(errorRecord, "message") ?? text(errorRecord, "code") ?? "error";
  const block = list(value?.["content"]).find(part => part["type"] === "text");
  if (block && typeof block["text"] === "string") return firstLine(block["text"]);
  return firstLine(output);
}
/** Writing and running rows carry only the base; a failed call reads its reason; `ok` draws the rest. */
function settle(call: CallInput, base: Base, ok: (output: string) => RowModel): RowModel {
  const state = baseState(call);
  if (state === "writing" || state === "running") return { ...base, state };
  const output = resultText(call.result);
  if (state === "failed") return { ...base, state: "failed", outcome: reasonOf(output) || "failed" };
  const error = recordOf(jsonOf(output))?.["error"];
  if (error !== undefined && error !== null) return { ...base, state: "failed", outcome: reasonOf(output) };
  return ok(output);
}
/** The JSON pretty-printed, for the expanded view. */
function pretty(value: unknown): () => readonly BodyLine[] {
  return () => textLines(JSON.stringify(value, null, 2));
}

// ---------------------------------------------------------------------------------------------------------------
// Language server

function position(call: CallInput): string | undefined {
  const path = stringArg(call.args, "path") ?? stringArg(call.args, "filePath");
  if (!path) return undefined;
  const line = numberArg(call.args, "line"), column = numberArg(call.args, "column");
  return shortPath(path, call.cwd) + (line === undefined ? "" : `:${line}${column === undefined ? "" : `:${column}`}`);
}
function lspBase(verb: string, call: CallInput): Base {
  const where = position(call);
  return { verb, ...(where ? { target: where, targetClip: "middle" as const } : {}) };
}
/** A language server refusal (`lsp_timeout: ...`) in words; a timeout always says so. */
function lspFailure(base: Base, output: string): RowModel {
  const match = /^(lsp_\w+):\s*([\s\S]*)$/.exec(output.trim()), code = match?.[1], reason = firstLine(match?.[2] ?? output);
  if (code === "lsp_timeout") {
    const ms = /after (\d+) ms/.exec(reason)?.[1], server = /^(\S+) is still indexing/.exec(reason)?.[1];
    return { ...base, state: "failed", outcome: `timeout after ${ms ? formatElapsed(Number(ms)) : "the deadline"}${server ? ` · ${server} still indexing` : ""}` };
  }
  return { ...base, state: "failed", outcome: reason || "failed" };
}
const SEVERITY_ORDER = ["error", "warning", "information", "hint", "unknown"];
function diagnosticsPresenter(): Presenter {
  return call => {
    const base = lspBase("diagnostics", call), state = baseState(call);
    if (state === "writing" || state === "running") return { ...base, state };
    const output = resultText(call.result);
    if (state === "failed") return lspFailure(base, output);
    const value = recordOf(jsonOf(output));
    if (!value) return { ...base, state: "ok", outcome: lineCount(output) ? "result in details" : "no diagnostics", expanded: () => textLines(output) };
    const items = list(value["diagnostics"]), total = count(value, "total") ?? items.length, server = text(value, "server");
    const right = server ? { right: server } : {};
    if (total === 0) return { ...base, ...right, state: "ok", outcome: value["complete"] === false ? "no diagnostics yet · server not settled" : "no diagnostics" };
    const bySeverity = new Map<string, number>();
    for (const item of items) bySeverity.set(text(item, "severity") ?? "unknown", (bySeverity.get(text(item, "severity") ?? "unknown") ?? 0) + 1);
    const words = SEVERITY_ORDER.filter(severity => bySeverity.has(severity))
      .map(severity => plural(bySeverity.get(severity)!, severity === "information" ? "note" : severity === "unknown" ? "other" : severity));
    if (total > items.length) words.push(`${total} in all`);
    const errors = bySeverity.get("error") ?? 0, warnings = bySeverity.get("warning") ?? 0;
    const lineOf = (item: Json): BodyLine => ({
      text: `${count(item, "line") ?? "?"}:${count(item, "column") ?? "?"}  ${text(item, "severity") ?? "unknown"}${text(item, "code") ? ` ${text(item, "code")}` : ""}  ${firstLine(text(item, "message") ?? "")}`,
      tone: text(item, "severity") === "error" ? "error" : text(item, "severity") === "warning" ? "warning" : "muted",
    });
    const serious = items.filter(item => text(item, "severity") === "error" || text(item, "severity") === "warning");
    const shown = errors || warnings ? serious.slice(0, 3) : [];
    return {
      ...base, ...right, state: errors ? "failed" : "ok", outcome: words.join(" · "),
      ...(!errors && warnings ? { outcomeTone: "warning" as const } : {}),
      ...(shown.length ? { body: shown.map(lineOf) } : {}),
      ...(items.length > shown.length ? { hiddenHint: `${items.length - shown.length} more`, expanded: () => items.map(lineOf) } : {}),
    };
  };
}
function locationsPresenter(kind: "definition" | "references"): Presenter {
  return call => {
    const base = lspBase(kind, call), state = baseState(call);
    if (state === "writing" || state === "running") return { ...base, state };
    const output = resultText(call.result);
    if (state === "failed") return lspFailure(base, output);
    const value = recordOf(jsonOf(output)), locations = list(value?.["locations"]);
    const total = count(value, "total") ?? locations.length;
    const where = (item: Json) => `${shortPath(text(item, "path") ?? "?", call.cwd)}:${count(item, "line") ?? "?"}`;
    const lines = (): readonly BodyLine[] => locations.map(item => ({ text: `${where(item)}:${count(item, "column") ?? "?"}  ${text(item, "preview") ?? ""}` }));
    const first = locations[0];
    if (!first) return { ...base, state: "ok", outcome: kind === "definition" ? "no definition found" : "no references" };
    if (kind === "definition") {
      return { ...base, state: "ok", outcome: total === 1 ? `→ ${where(first)}` : `${total} locations · first ${where(first)}`,
        ...(text(first, "preview") ? { detail: text(first, "preview")! } : {}), ...(total > 1 ? { hiddenHint: plural(total, "location"), expanded: lines } : {}) };
    }
    const files = new Set(locations.map(item => text(item, "path"))).size;
    return { ...base, state: "ok", outcome: `${plural(total, "reference")} in ${plural(files, "file")}`, detail: `first ${where(first)}`,
      hiddenHint: plural(locations.length, "location"), expanded: lines };
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Web and browser

function webFetchPresenter(): Presenter {
  return call => {
    const url = stringArg(call.args, "url");
    return settle(call, { verb: "fetch", ...(url ? { target: site(url, call.cwd) } : {}) }, output => {
      const value = recordOf(jsonOf(output));
      const base: Base = { verb: "fetch", target: site(text(value, "url") ?? url ?? "", call.cwd) };
      if (!value) return { ...base, state: "ok", outcome: linesWords(call, output), expanded: () => textLines(output) };
      const body = text(value, "body") ?? "", size = count(value, "total_bytes") ?? count(value, "bytes");
      const facts = [String(count(value, "status") ?? "fetched"), ...(size === undefined ? [] : [formatBytes(size)]),
        ...(value["next_offset"] !== null && value["next_offset"] !== undefined ? ["more to read"] : [])];
      const heading = body.split("\n").find(line => line.trim() !== "");
      return { ...base, state: "ok", outcome: facts.join(" · "), ...(heading ? { detail: heading.replace(/^#+\s*/, "") } : {}),
        expanded: () => textLines(body) };
    });
  };
}
function webReadPresenter(): Presenter {
  return call => {
    const id = stringArg(call.args, "content_id");
    return settle(call, { verb: "web read", ...(id ? { target: `snapshot ${id.slice(0, 8)}` } : {}) }, output => {
      const value = recordOf(jsonOf(output)), url = text(value, "url");
      const base: Base = { verb: "web read", target: url ? site(url, call.cwd) : `snapshot ${(id ?? "").slice(0, 8)}` };
      if (!value) return { ...base, state: "ok", outcome: linesWords(call, output), expanded: () => textLines(output) };
      const from = count(value, "offset") ?? 0, length = count(value, "output_bytes") ?? (text(value, "body") ?? "").length, total = count(value, "total_bytes");
      const more = value["next_offset"] !== null && value["next_offset"] !== undefined;
      return { ...base, state: "ok", outcome: `bytes ${from}-${from + length}${total === undefined ? "" : ` of ${total}`} · ${more ? "more to read" : "end"}`,
        expanded: () => textLines(text(value, "body") ?? "") };
    });
  };
}
function webSearchPresenter(): Presenter {
  return call => {
    const query = stringArg(call.args, "query"), base: Base = { verb: "search", ...(query ? { target: `"${query}"` } : {}) };
    return settle(call, base, output => {
      const value = recordOf(jsonOf(output));
      if (!value) {
        const lines = outputLines(call, output);
        return { ...base, state: "ok", outcome: output.trim() ? "answered" : "no answer", ...(output.trim() ? { detail: firstLine(output), expanded: () => textLines(output) } : {}),
          ...(lines ? { hiddenHint: plural(lines, "line") } : {}) };
      }
      if (value["configured"] === false) return { ...base, state: "failed", outcome: "search not configured", ...(text(value, "setup") ? { body: [{ text: text(value, "setup")! }] } : {}) };
      const results = list(value["results"]);
      if (!results.length) return { ...base, state: "ok", outcome: "no results" };
      const title = text(results[0], "title");
      return { ...base, state: "ok", outcome: plural(results.length, "result"), ...(title ? { detail: title } : {}),
        expanded: () => results.flatMap(item => [{ text: text(item, "title") ?? "", tone: "text" as const }, { text: text(item, "url") ?? "" }]) };
    });
  };
}
function browserPresenter(kind: "navigate" | "screenshot"): Presenter {
  return call => {
    const url = stringArg(call.args, "url"), verb = kind === "navigate" ? "browse" : "screenshot";
    return settle(call, { verb, ...(url ? { target: site(url, call.cwd) } : {}) }, output => {
      const value = recordOf(jsonOf(output)), title = text(value, "title"), base: Base = { verb, target: site(text(value, "url") ?? url ?? "", call.cwd) };
      if (kind === "navigate") {
        const body = text(value, "text") ?? output, lines = value ? lineCount(body) : outputLines(call, output);
        return { ...base, state: "ok", outcome: (title ? `"${title}"` : "loaded") + (lines === undefined ? "" : ` · ${plural(lines, "line")} of text`),
          ...(body.trim() ? { expanded: () => textLines(body) } : {}) };
      }
      const bytes = count(value, "bytes"), saved = text(value, "saved");
      return { ...base, state: "ok", outcome: [bytes === undefined ? "captured" : `${formatBytes(bytes)} PNG`, ...(saved ? [`saved ${saved}`] : [])].join(" · "),
        ...(title ? { detail: title } : {}) };
    });
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Owner questions

/** The first question (its header, else its text) and how many more. */
function questionTarget(args: Readonly<Record<string, unknown>>): string | undefined {
  const questions = list(args["questions"]);
  const first = questions[0];
  if (!first) return undefined;
  const label = text(first, "header") ?? text(first, "question") ?? "";
  return label + (questions.length > 1 ? ` + ${questions.length - 1} more` : "");
}
const NO_ANSWER: Readonly<Record<string, string>> = {
  "another owner/permission dialog is open": "no answer: another dialog was open",
  "pending UI unavailable": "no answer: no interactive pane to ask in",
};
function askUserPresenter(): Presenter {
  return call => {
    const withdraw = stringArg(call.args, "withdraw");
    const base: Base = { verb: "ask", ...(withdraw ? { target: `withdraw ${withdraw}` } : questionTarget(call.args) ? { target: questionTarget(call.args)! } : {}) };
    const state = baseState(call);
    if (state === "writing") return { ...base, state };
    if (state === "running") return withdraw ? { ...base, state } : { ...base, state: "needs-you", outcome: "waiting for you" };
    const output = resultText(call.result), details = recordOf(call.result?.details);
    if (state === "failed") return { ...base, state: "failed", outcome: reasonOf(output) || "failed" };
    const status = text(details, "status");
    if (withdraw) return { ...base, state: "ok", outcome: status ?? firstLine(output) };
    if (status === "answered" || status === "partial") {
      const answers = list(details?.["answers"]);
      const answerOf = (item: Json) => text(item, "option") ?? `"${text(item, "text") ?? ""}"`;
      const missed = details?.["unanswered"], unanswered = Array.isArray(missed) ? missed.length : 0;
      const first = answers[0];
      if (answers.length === 1 && first && !unanswered) return { ...base, state: "ok", outcome: `answered: ${answerOf(first)}` };
      return { ...base, state: "ok", outcome: `${answers.length} answered${unanswered ? ` · ${unanswered} unanswered` : ""}`,
        body: answers.slice(0, 3).map(item => ({ text: `${text(item, "question") ?? ""}: ${answerOf(item)}`, tone: "text" as const })),
        ...(answers.length > 3 ? { hiddenHint: `${answers.length - 3} more answers`, expanded: () => textLines(output, "text") } : {}) };
    }
    if (status === "pending") return { ...base, state: "needs-you", outcome: "waiting for you · /questions" };
    if (status === "no answer") {
      const reason = text(details, "reason");
      const known = reason === undefined ? undefined : NO_ANSWER[reason];
      return { ...base, state: "waiting", outcome: known ?? (reason ? `no answer: ${reason}` : "no answer: dismissed or timed out") };
    }
    return { ...base, state: status === "withdrawn" || status === "expired" || status === "cancelled" || status === "ended" ? "waiting" : "ok",
      outcome: status ?? firstLine(output) };
  };
}
/** A build child's blocking question to the owner. */
function askOwnerPresenter(): Presenter {
  return call => {
    const question = stringArg(call.args, "question"), options = list(call.args["options"]);
    const base: Base = { verb: "ask", ...(question ? { target: question } : {}), ...(options.length ? { right: plural(options.length, "option") } : {}) };
    const state = baseState(call);
    if (state === "writing") return { ...base, state };
    if (state === "running") return { ...base, state: "needs-you", outcome: "waiting for you" };
    const output = resultText(call.result);
    return state === "failed" ? { ...base, state: "failed", outcome: reasonOf(output) || "failed" }
      : { ...base, state: "ok", outcome: output.trim() ? `answered: ${firstLine(output)}` : "no answer" };
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Research helpers

const STOP_WORDS: Readonly<Record<string, string>> = {
  completed: "completed", turn_limit: "turn limit", time_reserve: "time limit", timeout: "timed out", cancelled: "cancelled",
  provider_error: "provider error", empty_response: "empty response", output_limit: "output limit", uncertain: "outcome uncertain",
};
/** The helper's findings: `details.text`, else the result text after its header block. */
function findingsOf(details: Json | undefined, output: string): string {
  const own = text(details, "text");
  if (own !== undefined) return own;
  const split = output.indexOf("\n\n");
  return split >= 0 ? output.slice(split + 2) : output;
}
function helperPresenter(kind: "explore" | "oracle", deps: ToolRenderDeps): Presenter {
  return call => {
    const prompt = stringArg(call.args, "prompt") ?? "";
    const preset = [stringArg(call.args, "profile"), stringArg(call.args, "effort")].filter(Boolean).join(" · ");
    const base: Base = { verb: kind, ...(prompt ? { target: firstLine(prompt) } : {}), ...(preset ? { right: preset } : {}) };
    const state = baseState(call), details = recordOf(call.result?.details);
    if (state === "writing") return { ...base, state };
    if (state === "running") {
      const progress = recordOf(details?.["progress"]), latest = firstLine(resultText(call.result));
      const turns = count(progress, "turns"), tools = count(progress, "tools");
      const counts = [turns === undefined ? "" : `turn ${turns}`, tools === undefined ? "" : plural(tools, "tool")].filter(Boolean).join(" · ");
      return { ...base, state, ...(counts ? { right: counts } : {}), ...(latest ? { body: [{ text: latest }] } : {}) };
    }
    const output = resultText(call.result);
    if (state === "failed" && !details) return { ...base, state: "failed", outcome: reasonOf(output) || "failed" };
    const report = recordOf(details?.["report"]), findings = findingsOf(details, output);
    const status = text(report, "status") ?? (text(details, "outcome") === "completed" ? "completed" : "partial");
    const stop = text(report, "stopReason") ?? text(details, "outcome") ?? "";
    const request = text(details, "request"), transcript = text(details, "sessionFile");
    // The headline is Markdown; its emphasis, heading and code marks read as noise on one row.
    const lines = call.totals ? call.totals.lines : lineCount(findings);
    const headline = firstLine(findings).replace(/^#+\s*/, "").replace(/\*\*|__|`/g, "");
    const open = deps.openView, view = open && request ? { view: { label: "Agents", open: () => open("agents", `helper:${request}`) } } : {};
    const expanded = (): readonly BodyLine[] => [
      ...(request ? [{ text: `Request ${request}` }] : []),
      ...(transcript ? [{ text: `Transcript ${transcript}` }] : []),
      ...(details?.["truncated"] === true ? [{ text: "Findings truncated; the transcript has the full answer.", tone: "warning" as const }] : []),
      { text: "" }, ...textLines(findings, "text"),
    ];
    const more = { ...view, expanded, ...(lines ? { hiddenHint: plural(lines, "line") } : {}) };
    if (status === "failed") return { ...base, ...more, state: "failed", outcome: `failed · stopped: ${STOP_WORDS[stop] ?? stop}` };
    if (status !== "completed") return { ...base, ...more, state: "unknown", outcome: `partial · stopped: ${STOP_WORDS[stop] ?? stop}`, ...(headline ? { detail: headline } : {}) };
    return { ...base, ...more, state: "ok", outcome: headline || "completed" };
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Tool search and MCP

function toolSearchPresenter(): Presenter {
  return call => {
    const query = stringArg(call.args, "query"), base: Base = { verb: "tool search", ...(query ? { target: `"${query}"` } : {}) };
    return settle(call, base, output => {
      const value = recordOf(jsonOf(output));
      if (!value) return { ...base, state: "ok", outcome: firstLine(output) || "no result", ...(lineCount(output) > 1 ? { expanded: () => textLines(output) } : {}) };
      const tools = list(value["tools"]).map(item => text(item, "name") ?? "?");
      return tools.length ? { ...base, state: "ok", outcome: plural(tools.length, "tool"), detail: tools.join(", "), expanded: pretty(value) }
        : { ...base, state: "ok", outcome: "no matching tools" };
    });
  };
}
function mcpControlPresenter(name: string): Presenter {
  return call => {
    const server = stringArg(call.args, "server"), uri = stringArg(call.args, "uri");
    const tools = Array.isArray(call.args["tools"]) ? call.args["tools"].filter((item): item is string => typeof item === "string") : [];
    const verb = name === "mcp_connect" ? "MCP connect" : name === "mcp_resources" ? "MCP resources" : name === "mcp_read_resource" ? "MCP read" : "MCP activate";
    const target = name === "mcp_activate" ? `${server ?? ""}: ${tools.join(", ")}` : name === "mcp_read_resource" ? `${server ?? ""} ${uri ?? ""}` : server ?? "all servers";
    const base: Base = { verb, target: target.trim() };
    return settle(call, base, output => {
      const value = jsonOf(output), record = recordOf(value);
      const more = { expanded: pretty(value ?? output) };
      const activated = record?.["activated"];
      if (name === "mcp_activate") return { ...base, state: "ok", outcome: `activated ${plural(Array.isArray(activated) ? activated.length : 0, "tool")}` };
      if (name === "mcp_connect") {
        if (record?.["configured"] === false) return { ...base, ...more, state: "ok", outcome: "MCP not configured", outcomeTone: "warning" };
        const servers = record?.["servers"], total = Array.isArray(servers) ? servers.length : 0;
        return { ...base, ...more, state: "ok", outcome: plural(total, "server") };
      }
      if (name === "mcp_resources") {
        const resources = Array.isArray(value) ? value : list(record?.["resources"]);
        return { ...base, ...more, state: "ok", outcome: plural(resources.length, "resource") };
      }
      const contents = list(record?.["contents"]);
      const size = outputBytes(call, output);
      return { ...base, ...more, state: "ok", outcome: contents.length ? plural(contents.length, "part") : size === undefined ? "read" : formatBytes(size) };
    });
  };
}
/** An MCP server's own tool: its registered label (`server: title`), its first argument, the result size or the error's text. */
function mcpToolPresenter(name: string, label: string | undefined): Presenter {
  const verb = label ?? name.replace(/^mcp_/, "").replace(/_[0-9a-f]{6,}$/, "").replaceAll("_", " ");
  return call => {
    const first = Object.values(call.args).find((value): value is string => typeof value === "string" && value.trim() !== "");
    const base: Base = { verb, ...(first ? { target: first } : {}) };
    return settle(call, base, output => {
      return { ...base, state: "ok", outcome: output.trim() ? linesWords(call, output) : "done", ...(output.trim() ? { expanded: () => textLines(output) } : {}) };
    });
  };
}
function snapshotPresenter(): Presenter {
  return call => {
    const document = stringArg(call.args, "document"), offset = numberArg(call.args, "offset");
    const base: Base = { verb: "read snapshot", ...(document ? { target: `${document}${offset ? ` from byte ${offset}` : ""}` } : {}) };
    return settle(call, base, output => {
      const value = recordOf(jsonOf(output)), body = text(value, "text") ?? output, from = count(value, "offset") ?? 0;
      const more = value?.["next_offset"] !== null && value?.["next_offset"] !== undefined;
      return { ...base, state: "ok", outcome: `bytes ${from}-${from + Buffer.byteLength(body)} · ${more ? "more to read" : "end"}`,
        hiddenHint: "text", expanded: () => textLines(body, "text") };
    });
  };
}

const PRESENTERS: Readonly<Record<string, (deps: ToolRenderDeps) => Presenter>> = {
  lsp_diagnostics: () => diagnosticsPresenter(),
  lsp_definition: () => locationsPresenter("definition"),
  lsp_references: () => locationsPresenter("references"),
  web_fetch: () => webFetchPresenter(),
  web_read: () => webReadPresenter(),
  web_search: () => webSearchPresenter(),
  browser_navigate: () => browserPresenter("navigate"),
  browser_screenshot: () => browserPresenter("screenshot"),
  ask_user: () => askUserPresenter(),
  ask_owner: () => askOwnerPresenter(),
  explore: deps => helperPresenter("explore", deps),
  oracle: deps => helperPresenter("oracle", deps),
  tool_search: () => toolSearchPresenter(),
  mcp_connect: () => mcpControlPresenter("mcp_connect"),
  mcp_resources: () => mcpControlPresenter("mcp_resources"),
  mcp_read_resource: () => mcpControlPresenter("mcp_read_resource"),
  mcp_activate: () => mcpControlPresenter("mcp_activate"),
  read_review_snapshot: () => snapshotPresenter(),
};

/** Presenters for the local tools above, and for any MCP server tool (`mcp_*`). */
export const localPresenters: PresenterModule = {
  presenter(name, deps, label) {
    if (Object.hasOwn(PRESENTERS, name)) return PRESENTERS[name]!(deps);
    return name.startsWith("mcp_") ? mcpToolPresenter(name, label) : undefined;
  },
};
