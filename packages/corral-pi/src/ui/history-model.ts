// Recorded history as data for the History popup and pager: list entries, runs, recap facts with their
// provenance kept apart, readable transcript and checkpoint rows, and the exact records behind them. Pure: no
// I/O, no terminal styling. Everything here is recorded history, never live state and never an instruction.
import type { PagerLine } from "./pager.ts";

type RecordValue = Record<string, unknown>;
export const record = (value: unknown): RecordValue =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};
const records = (value: unknown): RecordValue[] => Array.isArray(value) ? value.map(record) : [];

/** Rows a list shows at most; the reader's own output is byte-bounded as well, so no list is exhaustive. */
const HISTORY_ROWS = 200;
/** Characters of one record or page the pager shows at most. */
const DISPLAY_LIMIT = 64 * 1024;
/** Records one transcript page asks for. */
export const TRANSCRIPT_PAGE = 200;

/** A recorded value as one line of text; undefined when absent or empty. */
function text(value: unknown): string | undefined {
  if (value === undefined || value === null || value === "") return undefined;
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}
const oneLine = (value: string): string => value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/gu, " ").trim();
const two = (n: number): string => String(n).padStart(2, "0");
/** `MM-DD HH:MM` local time of a recorded timestamp, or the recorded text itself when it is not a time. */
function recordedTime(value: unknown): string | undefined {
  const raw = text(value);
  if (raw === undefined) return undefined;
  const at = typeof value === "number" ? new Date(value) : new Date(raw);
  if (Number.isNaN(at.getTime())) return oneLine(raw);
  return `${two(at.getMonth() + 1)}-${two(at.getDate())} ${two(at.getHours())}:${two(at.getMinutes())}`;
}
function clockTime(value: unknown): string | undefined {
  const raw = text(value);
  const at = raw === undefined ? undefined : new Date(typeof value === "number" ? value : raw);
  return at && !Number.isNaN(at.getTime()) ? `${two(at.getHours())}:${two(at.getMinutes())}` : undefined;
}

/** `text` cut to the display limit, with the statement that says so. */
function bounded(value: string): string[] {
  const lines = value.slice(0, DISPLAY_LIMIT).split(/\r?\n/);
  if (lines.at(-1) === "") lines.pop();
  if (value.length > DISPLAY_LIMIT) lines.push(`display cut at ${DISPLAY_LIMIT / 1024} KiB · the recorded data is larger`);
  return lines;
}
/** The exact record, pretty-printed and bounded; `not recorded` when absent. */
export function rawLines(value: unknown): PagerLine[] {
  if (value === undefined) return [{ text: "not recorded" }];
  const json = typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? String(value);
  return bounded(json).map(line => ({ text: line }));
}

export interface HistoryEntry {
  readonly id: string;
  readonly title: string;
  readonly status: string | undefined;
  readonly raw: Record<string, unknown>;
}
/** The task rows of a list read, at most `HISTORY_ROWS`, and how many the reader returned. The reader searched them. */
export function historyEntries(value: unknown): { readonly rows: readonly HistoryEntry[]; readonly returned: number } {
  const source = records(value);
  const rows = source.slice(0, HISTORY_ROWS).map((row): HistoryEntry =>
    ({ id: text(row.id) ?? "", title: oneLine(text(row.title) ?? ""), status: text(row.status), raw: row }));
  return { rows, returned: source.length };
}

export interface RunEntry {
  readonly id: string;
  readonly outcome: string | undefined;
  /** The recorded end, undefined while the record has none. */
  readonly ended: string | undefined;
  readonly raw: Record<string, unknown>;
}
/** A task's runs from its `corral show` read, latest first. */
export function runEntries(view: Record<string, unknown>): readonly RunEntry[] {
  return records(view.runs).reverse().map(run => ({
    id: text(run.id) ?? "", outcome: text(run.outcome), ended: recordedTime(run.ended), raw: run,
  }));
}

/** One recap fact: words first, where it was read from kept apart. undefined value = not recorded. */
export interface RecapFact { readonly label: string; readonly value: string | undefined; readonly provenance: string }

/** A task run's recap from its run record. */
export function taskRecap(run: Record<string, unknown>): readonly RecapFact[] {
  return [
    { label: "outcome", value: text(run.outcome), provenance: "run.outcome · absent while the run is live" },
    { label: "summary", value: text(run.summary), provenance: "run.summary" },
    { label: "profile", value: [text(run.profile), text(run.model)].filter(Boolean).join(" · ") || undefined, provenance: "run.profile and run.model" },
    { label: "ended", value: recordedTime(run.ended), provenance: "run.ended · a missing end is not success" },
  ];
}

/** Label/value rows; `hang` is the cells of the label column, under which every wrapped continuation row hangs. */
function factRows(facts: readonly RecapFact[]): { readonly rows: PagerLine[]; readonly hang: number } {
  const hang = Math.max(0, ...facts.map(fact => fact.label.length)) + 2;
  return { hang, rows: facts.map(fact => ({
    text: `${fact.label.padEnd(hang - 2)}  ${fact.value === undefined ? "not recorded" : oneLine(fact.value)}`, hanging: hang,
  })) };
}

/** The recap as label/value rows; no field path. */
export function recapLines(facts: readonly RecapFact[]): PagerLine[] {
  return factRows(facts).rows;
}
/** Each fact with its source field path, then the bounded raw record it was read from. */
export function sourceLines(facts: readonly RecapFact[], record: unknown): PagerLine[] {
  const { rows, hang } = factRows(facts);
  return [
    { text: "Facts and their sources", heading: true },
    ...rows.flatMap((row, i) => [row, { text: `${" ".repeat(hang)}from ${facts[i]?.provenance ?? ""}`, hanging: hang }]),
    { text: "" },
    { text: "Recorded record · exact, recorded not live", heading: true },
    ...rawLines(record),
  ];
}

/** The first record of the next page: the reader's byte-limit marker, else the record after this page. */
export function nextHistoryPage(text: string, first: number): number {
  const marker = /\[page byte limit; resume with --lines (\d+)-\d+\]/.exec(text);
  return marker ? Number(marker[1]) : first + TRANSCRIPT_PAGE;
}

const LIMIT_MARK = /^\[page byte limit; resume with --lines \d+-\d+\]$/;
const SPEAKER = 6;
/** Cells before a transcript row's text: the `HH:MM` column, two blanks, the speaker column and one blank. */
const TRANSCRIPT_HANG = 5 + 2 + SPEAKER + 1;
function contentText(content: unknown): { readonly words: string; readonly tools: readonly string[]; readonly thinking: boolean } {
  if (typeof content === "string") return { words: content, tools: [], thinking: false };
  const parts = records(content);
  return {
    words: parts.filter(part => part.type === "text").map(part => text(part.text) ?? "").join("\n"),
    tools: parts.filter(part => part.type === "toolCall").map(part => text(part.name) ?? "tool"),
    thinking: parts.some(part => part.type === "thinking"),
  };
}
const firstLine = (words: string): string => oneLine(words.split(/\r?\n/).find(line => line.trim()) ?? "");
/** One readable row for one transcript record: who, then what, wrapped by the pager. */
function transcriptRow(line: string): string {
  let entry: RecordValue;
  try { entry = record(JSON.parse(line)); } catch { return `${"".padEnd(5)}  ${"note".padEnd(SPEAKER)} unreadable record · d shows it exactly`; }
  const message = record(entry.message);
  const at = (clockTime(entry.timestamp ?? message.timestamp) ?? "").padEnd(5);
  const row = (who: string, what: string) => `${at}  ${who.padEnd(SPEAKER)} ${what}`;
  if (entry.type === "message") {
    const role = text(message.role) ?? "";
    const content = contentText(message.content);
    if (role === "user") return row("you", firstLine(content.words) || "(no text)");
    if (role === "assistant") {
      const said = oneLine(content.words);
      const calls = content.tools.length ? `calls ${content.tools.join(", ")}` : "";
      return row("agent", [said, calls].filter(Boolean).join(" · ") || (content.thinking ? "(thinking only)" : "(no text)"));
    }
    if (role === "toolResult") return row("tool", `${text(message.toolName) ?? "tool"} · ${message.isError === true ? "failed" : "done"}`);
    if (role === "bashExecution") {
      const code = text(message.exitCode);
      return row("bash", `${firstLine(text(message.command) ?? "")} · ${message.cancelled === true ? "cancelled" : code === undefined ? "no exit code" : `exit ${code}`}`);
    }
    return row("note", `${role || "message"} · ${firstLine(content.words) || "(no text)"}`);
  }
  if (entry.type === "session") return row("note", `session started${text(entry.cwd) ? ` in ${text(entry.cwd)}` : ""}`);
  if (entry.type === "model_change") return row("note", `model ${[text(entry.provider), text(entry.modelId)].filter(Boolean).join("/") || "changed"}`);
  if (entry.type === "thinking_level_change") return row("note", `effort ${text(entry.thinkingLevel) ?? "changed"}`);
  if (entry.type === "compaction") return row("note", "context compacted");
  return row("note", text(entry.customType) ?? text(entry.type) ?? "record");
}

export interface TranscriptPage {
  /** One readable row per record; byte-limit statements in words. */
  readonly lines: readonly PagerLine[];
  /** The page's exact JSONL, bounded. */
  readonly raw: readonly PagerLine[];
  /** The reader cut the page at its byte limit; the next page continues where it stopped. */
  readonly cut: boolean;
  /** The reader returned no records. */
  readonly empty: boolean;
}
/** A transcript page as the reader returned it. */
export function transcriptPage(page: string): TranscriptPage {
  const shown = bounded(page).filter(line => line.trim());
  const cut = page.split(/\r?\n/).some(line => LIMIT_MARK.test(line.trim()));
  const lines = shown.map((line): PagerLine => {
    const trimmed = line.trim();
    if (LIMIT_MARK.test(trimmed)) return { text: "page cut at the byte limit · n continues" };
    if (trimmed === "[record truncated]") return { text: "the record above is cut at the byte limit · d shows what was read" };
    if (trimmed.startsWith("display cut at ")) return { text: trimmed };
    return { text: transcriptRow(trimmed), hanging: TRANSCRIPT_HANG };
  });
  return { lines, raw: shown.map(line => ({ text: line })), cut, empty: !page.trim() };
}

