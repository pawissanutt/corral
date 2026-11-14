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

export type HistoryKind = "tasks" | "assistants";
export interface HistoryEntry {
  readonly kind: HistoryKind;
  readonly id: string;
  readonly title: string;
  readonly status: string | undefined;
  readonly raw: Record<string, unknown>;
}
/**
 * The rows of a list read, at most `HISTORY_ROWS`, and how many the reader returned. Tasks are searched by the
 * reader; assistant sessions are filtered here by the words of `query` over the whole record.
 */
export function historyEntries(kind: HistoryKind, value: unknown, query: string): { readonly rows: readonly HistoryEntry[]; readonly returned: number } {
  const words = kind === "assistants" ? query.toLowerCase().split(/\s+/).filter(Boolean) : [];
  const source = records(value).filter(row => {
    if (!words.length) return true;
    const haystack = JSON.stringify(row).toLowerCase();
    return words.every(word => haystack.includes(word));
  });
  const rows = source.slice(0, HISTORY_ROWS).map((row): HistoryEntry => kind === "tasks"
    ? { kind, id: text(row.id) ?? "", title: oneLine(text(row.title) ?? ""), status: text(row.status), raw: row }
    : { kind, id: text(row.session ?? row.id) ?? "", title: oneLine(text(row.focus ?? row.title) ?? ""),
      status: text(row.status) ?? (text(row.ended) === undefined ? undefined : "ended"), raw: row });
  return { rows, returned: source.length };
}

export interface RunEntry {
  readonly id: string;
  readonly outcome: string | undefined;
  /** The recorded end, undefined while the record has none. */
  readonly ended: string | undefined;
  readonly raw: Record<string, unknown>;
}
/** A task record's runs, latest first. */
export function runEntries(task: Record<string, unknown>): readonly RunEntry[] {
  return records(task.runs).reverse().map(run => ({
    id: text(run.id) ?? "", outcome: text(run.outcome), ended: recordedTime(run.ended), raw: run,
  }));
}

/** One recap fact: words first, where it was read from kept apart. undefined value = not recorded. */
export interface RecapFact { readonly label: string; readonly value: string | undefined; readonly provenance: string }

function ledgerStep(value: unknown): string | undefined {
  const steps = records(record(value).phases).flatMap(phase => records(phase.steps));
  const step = steps.find(step => record(step.status).state === "in_progress") ?? steps.find(step => record(step.status).state === "pending");
  if (!step) return undefined;
  const id = text(step.id), label = text(step.label);
  return [id, label].filter(Boolean).join(" · ") || undefined;
}
function questionText(value: RecordValue): string {
  const key = text(value.key), words = text(value.text ?? value.question ?? value.prompt);
  return oneLine(key && words ? `${key}: ${words}` : words ?? key ?? JSON.stringify(value));
}
function counted(value: unknown, noun: string): string | undefined {
  return Array.isArray(value) ? `${value.length} ${noun}${value.length === 1 ? "" : "s"} recorded` : undefined;
}

/**
 * A task run's recap. `questions` is the question list read for the task (an Error when that read failed); only
 * questions of this run and its children count.
 */
export function taskRecap(task: Record<string, unknown>, run: Record<string, unknown>, questions: unknown): readonly RecapFact[] {
  const checkpoint = record(record(records(run.checkpoints).at(-1)).checkpoint);
  const host = record(run.agent_host);
  const prefix = `${String(task.id)}/${String(run.id)}/`;
  const waiting = questions instanceof Error ? `could not be read: ${oneLine(questions.message)}`
    : Array.isArray(questions) ? (() => {
      const pending = records(questions).map(view => record(view.question)).filter(question => String(question.key ?? "").startsWith(prefix));
      return pending.length ? pending.map(questionText).join("; ") : "nothing pending";
    })() : undefined;
  const next = text(record(checkpoint.input).next_action);
  return [
    { label: "outcome", value: text(run.outcome ?? task.result), provenance: run.outcome !== undefined ? "run.outcome" : "task.result (the run records no outcome)" },
    { label: "summary", value: text(run.summary), provenance: "run.summary" },
    { label: "waiting on", value: waiting, provenance: "question list for the task · effective pending questions of this run and its children" },
    { label: "next step", value: next ?? ledgerStep(run.progress),
      provenance: next !== undefined ? "run.checkpoints[-1].checkpoint.input.next_action" : "run.progress · first in-progress or pending step" },
    { label: "checkpoint", value: recordedTime(checkpoint.captured), provenance: "run.checkpoints[-1].checkpoint.captured · compatibility not inferred" },
    { label: "generations", value: counted(host.generations, "generation"), provenance: "run.agent_host.generations · recorded, not live health" },
    { label: "rewinds", value: counted(host.rewinds, "rewind"), provenance: "run.agent_host.rewinds · recorded, not live health" },
  ];
}

/** An assistant session's recap from its session and container records. */
export function assistantRecap(session: Record<string, unknown>, container: Record<string, unknown>): readonly RecapFact[] {
  const state = record(container.state);
  const ended = text(session.ended) !== undefined;
  const open = records(state.questions).filter(question => question.state === "open" || question.state === "escalated");
  return [
    { label: "outcome", value: undefined, provenance: "assistant sessions record no structured outcome" },
    { label: "summary", value: text(session.summary), provenance: "assistant.session.summary" },
    { label: "ended", value: recordedTime(session.ended), provenance: "assistant.session.ended · a missing end is not success" },
    { label: "waiting on", value: ended ? "nothing · the session ended" : Array.isArray(state.questions) ? open.length ? open.map(questionText).join("; ") : "nothing pending" : undefined,
      provenance: ended ? "assistant.session.ended closes its questions" : "assistant.container.state.questions · open or escalated" },
    { label: "next step", value: ledgerStep(state.todo), provenance: "assistant.container.state.todo · first in-progress or pending step" },
    { label: "generations", value: counted(state.generations, "generation"), provenance: "assistant.container.state.generations · recorded, not live health" },
    { label: "rewinds", value: counted(state.rewinds, "rewind"), provenance: "assistant.container.state.rewinds · recorded, not live health" },
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

/** Recorded checkpoints, rewinds and any unfinished rewind as readable rows, from `agent rewind --list`. */
export function recoveryLines(value: unknown): PagerLine[] {
  const recovery = record(value);
  const checkpoints = records(recovery.checkpoints), rewinds = records(recovery.rewinds);
  const pending = recovery.pending === null || recovery.pending === undefined ? undefined : record(recovery.pending);
  const when = (row: RecordValue) => recordedTime(row.at) ?? "time not recorded";
  const lines: PagerLine[] = [{ text: "Checkpoints", heading: true }];
  if (!Array.isArray(recovery.checkpoints)) lines.push({ text: "not recorded" });
  else if (!checkpoints.length) lines.push({ text: "none recorded" });
  for (const checkpoint of checkpoints) {
    const turn = [text(checkpoint.turn) && `turn ${text(checkpoint.turn)}`, text(checkpoint.generation) && `generation ${text(checkpoint.generation)}`].filter(Boolean).join(" · ");
    lines.push({ text: `${text(checkpoint.id) ?? "checkpoint"}  ${[turn, when(checkpoint), checkpoint.ended === true ? "turn ended" : "turn not ended in record"].filter(Boolean).join(" · ")}` });
  }
  lines.push({ text: "" }, { text: "Rewinds", heading: true });
  if (!Array.isArray(recovery.rewinds)) lines.push({ text: "not recorded" });
  else if (!rewinds.length) lines.push({ text: "none recorded" });
  for (const rewind of rewinds) {
    const target = text(rewind.checkpoint);
    lines.push({ text: `${text(rewind.id) ?? "rewind"}  ${[target && `to ${target}`, text(rewind.stage ?? rewind.state) ?? "stage not recorded", when(rewind), text(rewind.detail)].filter(Boolean).map(part => oneLine(String(part))).join(" · ")}` });
  }
  lines.push({ text: "" }, { text: pending ? `Unfinished rewind ${text(pending.id) ?? ""} · ${text(pending.stage ?? pending.state) ?? "stage not recorded"}`.replace(/\s+·/, " ·") : "No unfinished rewind recorded" });
  lines.push({ text: "Recorded history · read-only · nothing here restores or rewinds" });
  return lines;
}
