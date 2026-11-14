// The transcript marks (`src/host/transcript-marks.ts`) as quiet rows: one muted row per mark in the row kit's
// geometry, the state glyph first and the time of day right-aligned. A run's end says how it ended and what the agent
// now waits for; a stopped or failed run only adds its counts (`○ run ended · 12s · 3 tool calls`), because the
// assistant message right above already names the cause (`src/ui/error-copy.ts`). A model change, a host note and
// what passed between the owner and a child read the same way. Marks are custom entries: Pi draws them live and again
// on resume, and they never reach the model. A collapsed row clips and never wraps; when the row is too narrow the
// least useful facts drop first (re-billed tokens, tool calls, the time, the duration, the resume hint, the cost) and
// the reason clips last. A click or Ctrl+O opens the full text. `quietNote` draws a note row for a line Pi hands
// corral (`notice-route.ts`): it is held in the transcript directly, not journaled, and opens the same way.
import type { CustomEntry, EntryRenderOptions, ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";

import { MARK_ENTRY } from "../host/transcript-marks.ts";
import type { ChildMark, MarkSink, ModelMark, NoteMark, TranscriptMark, TurnEndMark, TurnWait } from "../host/transcript-marks.ts";
import { PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET, plainLabel } from "./panels-view.ts";
import { clipPlain, formatDuration, recordOf } from "./tool-row.ts";

type Tone = "muted" | "text" | "accent" | "success" | "warning" | "error";
const TONE: Readonly<Record<Tone, () => string>> = {
  muted: () => PANEL_MUTED, text: () => PANEL_TEXT, accent: () => PANEL_BLUE,
  success: () => PANEL_SUCCESS, warning: () => PANEL_WARNING, error: () => PANEL_ERROR,
};

/** A run of text in one tone. The lead's last span is the one that clips; `close` stays after the cut (a closing quote). */
interface Span { readonly text: string; readonly tone: Tone; readonly close?: string }
/** A fact after the lead, joined by ` · ` (or `join`). Lower `drop` ranks leave a narrow row first. */
interface Piece { readonly text: string; readonly join?: string; readonly drop: number }
export interface MarkRow {
  readonly glyph: string;
  readonly glyphTone: Tone;
  /** Never dropped; its last span clips once every piece is gone. */
  readonly lead: readonly Span[];
  readonly pieces: readonly Piece[];
  /** Time of day, muted and right-aligned; empty for none. */
  readonly time: string;
  readonly timeDrop: number;
  /** Expanded only: rows listed under the line (every wait of a run). */
  readonly list?: readonly string[];
  /** Expanded only: the full text the row quotes, kept with its line breaks. */
  readonly body?: string;
}

// Drop ranks, first to leave a narrow row first: the counts before the time, the time before what the agent waits on.
const DROP = { rebilled: 1, calls: 2, time: 3, duration: 4, hint: 5, cost: 6, more: 8, goOn: 9 } as const;
/** A run shorter than this with no tool calls that simply finished gets the shortest row. */
const QUICK_MS = 2000;
/** Expanded rows shown before the rest is counted. */
const EXPANDED_ROWS = 40;

// ---------------------------------------------------------------------------------------------------------------
// Words

/** `14:35` in local time; empty when `at` is not a time. */
export function timeOfDay(at: number): string {
  if (!Number.isFinite(at) || at <= 0) return "";
  const date = new Date(at), two = (value: number) => String(value).padStart(2, "0");
  return `${two(date.getHours())}:${two(date.getMinutes())}`;
}
function money(cost: number): string {
  return cost > 0 && cost < 0.005 ? "<$0.01" : `$${cost.toFixed(2)}`;
}
function tokens(count: number): string {
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1).replace(/\.0$/u, "")}M`;
  if (count >= 1000) return `${Math.round(count / 1000)}k`;
  return String(Math.round(count));
}
const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;
/** The first non-blank line, flattened, and a muted `n more lines` piece for the rest. */
function opening(text: string): { readonly first: string; readonly rest: Piece[] } {
  const lines = text.split("\n").filter(line => line.trim() !== "");
  const first = plainLabel(lines[0] ?? ""), more = lines.length - 1;
  return { first, rest: more > 0 ? [{ text: plural(more, "more line", "more lines"), drop: DROP.more }] : [] };
}

function turnRow(mark: TurnEndMark): MarkRow {
  const time = timeOfDay(mark.at);
  const quick = mark.reason === "done" && mark.durationMs < QUICK_MS && mark.toolCalls === 0;
  // The shortest row: nothing happened worth counting, so the time follows the word.
  if (quick) return { glyph: "✓", glyphTone: "success", lead: [{ text: "done", tone: "muted" }], pieces: time ? [{ text: time, drop: DROP.time }] : [], time: "", timeDrop: DROP.time };
  const note = mark.note ? plainLabel(mark.note) : "";
  const pieces: Piece[] = [];
  let glyph = "○", glyphTone: Tone = "muted", lead: Span[] = [];
  let list: string[] | undefined;
  switch (mark.reason) {
    case "done":
      glyph = "✓"; glyphTone = "success"; lead = [{ text: "done", tone: "muted" }];
      break;
    case "waiting": {
      const first = mark.waits[0];
      lead = [{ text: first ? `waiting for ${plainLabel(first.label)}` : "waiting", tone: "text" }];
      if (mark.waits.length > 1) {
        pieces.push({ text: `+ ${mark.waits.length - 1} more`, join: " ", drop: DROP.more });
        list = mark.waits.map(wait => plainLabel(wait.label));
      }
      pieces.push({ text: "the agent resumes when it finishes", drop: DROP.hint });
      break;
    }
    // The assistant message above names the cause and the calls that did not run; this row adds only the counts.
    case "stopped":
    case "error":
      lead = [{ text: "run ended", tone: "muted" }];
      break;
    case "length":
      glyph = "!"; glyphTone = "warning"; lead = [{ text: "the reply was cut at the output limit", tone: "text" }];
      break;
    case "continue_stopped":
      glyph = "!"; glyphTone = "warning"; lead = [{ text: note ? `auto-continue stopped: ${note}` : "auto-continue stopped", tone: "text" }];
      pieces.push({ text: "send a message to go on", drop: DROP.goOn });
      break;
  }
  const duration = formatDuration(mark.durationMs);
  if (duration) pieces.push({ text: duration, drop: DROP.duration });
  if (mark.toolCalls > 0) pieces.push({ text: plural(mark.toolCalls, "tool call", "tool calls"), drop: DROP.calls });
  if (mark.cost !== undefined && Number.isFinite(mark.cost)) pieces.push({ text: money(mark.cost), drop: DROP.cost });
  if (mark.rebilledTokens && mark.rebilledTokens > 0) pieces.push({ text: `${tokens(mark.rebilledTokens)} tokens re-billed`, drop: DROP.rebilled });
  return { glyph, glyphTone, lead, pieces, time, timeDrop: DROP.time, ...(list ? { list } : {}) };
}

function modelRow(mark: ModelMark): MarkRow {
  const model = plainLabel(mark.model), effort = mark.effort ? plainLabel(mark.effort) : undefined;
  const was = mark.was, wasModel = was ? plainLabel(was.model) : undefined, wasEffort = was?.effort ? plainLabel(was.effort) : undefined;
  const time = timeOfDay(mark.at);
  // Only the effort changed: say that, not the model again.
  if (was && wasModel === model && effort !== undefined && effort !== wasEffort) {
    const pieces: Piece[] = wasEffort ? [{ text: `(was ${wasEffort})`, join: " ", drop: DROP.rebilled }] : [];
    return { glyph: "○", glyphTone: "muted", lead: [{ text: "effort → ", tone: "muted" }, { text: effort, tone: "text" }], pieces, time, timeDrop: DROP.time };
  }
  const pieces: Piece[] = [];
  if (effort) pieces.push({ text: `effort ${effort}`, drop: DROP.duration });
  if (was && wasModel !== model) {
    const before = [wasModel, wasEffort].filter(Boolean).join(" · ");
    if (before) pieces.push({ text: `(was ${before})`, join: " ", drop: DROP.rebilled });
  }
  return { glyph: "○", glyphTone: "muted", lead: [{ text: "model → ", tone: "muted" }, { text: model, tone: "text" }], pieces, time, timeDrop: DROP.time };
}

function noteRow(mark: NoteMark): MarkRow {
  const [glyph, glyphTone]: [string, Tone] = mark.tone === "error" ? ["✗", "error"] : mark.tone === "warning" ? ["!", "warning"] : ["○", "muted"];
  const { first, rest } = opening(mark.text);
  return {
    glyph, glyphTone, lead: [{ text: first, tone: mark.tone === "text" ? "muted" : "text" }],
    pieces: rest, time: timeOfDay(mark.at), timeDrop: DROP.time, body: mark.text,
  };
}

function childRow(mark: ChildMark): MarkRow {
  const name: Span = { text: plainLabel(mark.child), tone: "text" }, { first, rest } = opening(mark.text);
  const row = { pieces: rest, time: timeOfDay(mark.at), timeDrop: DROP.time, body: mark.text };
  switch (mark.act) {
    case "asked":
      return { ...row, glyph: "▲", glyphTone: "warning", lead: [name, { text: " asked: ", tone: "muted" }, { text: first, tone: "text" }] };
    case "answered":
      return { ...row, glyph: "→", glyphTone: "accent", lead: [name, { text: ": ", tone: "muted" }, { text: first, tone: "text" }] };
    case "steered":
      return { ...row, glyph: "→", glyphTone: "accent", lead: [name, { text: " steered: ", tone: "muted" }, { text: `“${first}`, tone: "text", close: "”" }] };
  }
}

/** The row model of a mark: glyph, lead, facts in drop order and the time. Pure; exported for tests and scripts. */
export function markRow(mark: TranscriptMark): MarkRow {
  switch (mark.kind) {
    case "turn_end": return turnRow(mark);
    case "model": return modelRow(mark);
    case "note": return noteRow(mark);
    case "child": return childRow(mark);
  }
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing

const spanWidth = (span: Span) => visibleWidth(span.text) + visibleWidth(span.close ?? "");
const pieceWidth = (piece: Piece) => visibleWidth(piece.join ?? " · ") + visibleWidth(piece.text);
function paint(spans: readonly Span[]): string {
  return spans.map(span => TONE[span.tone]() + span.text + (span.close ?? "")).join("");
}

/** The collapsed line at `room` cells, and whether everything the row holds is on it. */
export function markLine(row: MarkRow, room: number): { readonly line: string; readonly whole: boolean } {
  const W = Math.max(1, room);
  let pieces = [...row.pieces], time = row.time, lead = [...row.lead];
  const width = () => 2 + lead.reduce((sum, span) => sum + spanWidth(span), 0) + pieces.reduce((sum, piece) => sum + pieceWidth(piece), 0)
    + (time ? 2 + visibleWidth(time) : 0);
  let whole = true;
  // The lowest rank leaves first; on a tie the later fact goes first.
  const ranks = [...new Set([...pieces.map(piece => piece.drop), ...(time ? [row.timeDrop] : [])])].sort((a, b) => a - b);
  for (const rank of ranks) {
    if (width() <= W) break;
    for (let index = pieces.length - 1; index >= 0 && width() > W; index--) {
      if (pieces[index]!.drop === rank) { pieces = pieces.filter((_, at) => at !== index); whole = false; }
    }
    if (width() > W && time && row.timeDrop === rank) { time = ""; whole = false; }
  }
  if (width() > W && lead.length) {
    // The last span of the lead clips; its closing mark stays.
    const last = lead[lead.length - 1]!, room = visibleWidth(last.text) - (width() - W);
    lead = room >= 2 ? [...lead.slice(0, -1), { ...last, text: clipPlain(last.text, room) }] : lead.slice(0, -1);
    whole = false;
  }
  const left = TONE[row.glyphTone]() + row.glyph + " " + paint(lead)
    + pieces.map(piece => PANEL_MUTED + (piece.join ?? " · ") + piece.text).join("");
  const leftWidth = width() - (time ? 2 + visibleWidth(time) : 0);
  const line = time ? left + " ".repeat(Math.max(2, W - leftWidth - visibleWidth(time))) + PANEL_MUTED + time : left;
  return { line: truncateToWidth(line, W, "…") + RESET, whole };
}

/**
 * Every row the mark draws at `width`: one clipped row collapsed; expanded, the full text and every wait, `maxRows`
 * of them under the line before the rest is counted.
 */
export function markLines(row: MarkRow, width: number, expanded: boolean, maxRows = EXPANDED_ROWS): string[] {
  // The row kit's geometry: one cell of padding each side.
  const pad = Math.max(0, Math.min(1, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2), margin = " ".repeat(pad);
  const { line, whole } = markLine(row, room);
  if (!expanded) return [margin + line];
  const indent = margin + "  ", inner = Math.max(1, room - 2);
  const rows: string[] = [];
  const multiline = row.body !== undefined && row.body.trim().includes("\n");
  if (whole || multiline) rows.push(margin + line);
  else {
    // The whole sentence, wrapped under its glyph. A fact keeps its words together and its separator at the line end.
    const full = paint(row.lead) + row.pieces.map(piece => PANEL_MUTED + (piece.join ?? " · ").replace(/^ /u, "\u00a0") + piece.text.replace(/ /g, "\u00a0")).join("")
      + (row.time ? PANEL_MUTED + "\u00a0· " + row.time : "");
    wrapTextWithAnsi(full, inner).forEach((text, index) => {
      rows.push((index === 0 ? margin + TONE[row.glyphTone]() + row.glyph + " " : indent) + text + RESET);
    });
  }
  const body: string[] = [];
  if (multiline && row.body !== undefined) {
    const text = row.body.replace(/\t/g, "  ").replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ").trim();
    for (const part of text.split("\n")) body.push(...(part.trim() === "" ? [""] : wrapTextWithAnsi(part, inner).map(text => PANEL_TEXT + text)));
  }
  for (const label of row.list ?? []) body.push(PANEL_MUTED + truncateToWidth(`○ ${label}`, inner, "…"));
  const shown = body.slice(0, maxRows);
  const more = body.length - shown.length;
  if (more > 0) shown.push(PANEL_MUTED + truncateToWidth(`… ${more} more rows`, inner, "…"));
  return [...rows, ...shown.map(text => indent + text + RESET)];
}

// ---------------------------------------------------------------------------------------------------------------
// Entries

/** A click's own open or folded state, kept while Ctrl+O stays where it was when the click happened. */
interface Opened { readonly base: boolean; readonly open: boolean }
const opened = new WeakMap<object, Opened>();

class MarkRows implements Component {
  constructor(private readonly row: MarkRow, private readonly entry: object, protected base: boolean,
    private readonly maxRows = EXPANDED_ROWS) {}
  private expanded(): boolean {
    const state = opened.get(this.entry);
    return state && state.base === this.base ? state.open : this.base;
  }
  invalidate(): void {}
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type !== "click" || event.button !== "left") return undefined;
    opened.set(this.entry, { base: this.base, open: !this.expanded() });
    return { handled: true };
  }
  render(width: number): string[] {
    return markLines(this.row, width, this.expanded(), this.maxRows);
  }
}

/**
 * A note row held in the transcript itself: Pi's tool-output toggle (Ctrl+O) reaches it through `setExpanded`. It is
 * the only place its text is drawn, so expanded it shows every row; the transcript scrolls.
 */
class QuietNote extends MarkRows {
  setExpanded(expanded: boolean): void { this.base = expanded; }
}

/**
 * One quiet note row in the run-mark geometry, for a line that is not a journal entry: the first line of `text` is the
 * row, the time of `at` right; a click or Ctrl+O shows the whole text under it, wrapped, every line.
 */
export function quietNote(text: string, tone: NoteMark["tone"], at: number): Component {
  return new QuietNote(noteRow({ kind: "note", at, tone, text }), {}, false, Number.POSITIVE_INFINITY);
}

const isText = (value: unknown): value is string => typeof value === "string";
const isNumber = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value);
const WAIT_KINDS: readonly TurnWait["kind"][] = ["job", "gate", "owner", "child", "cell", "event"];
const REASONS: readonly TurnEndMark["reason"][] = ["done", "waiting", "stopped", "error", "length", "continue_stopped"];
const NOTE_TONES: readonly NoteMark["tone"][] = ["text", "warning", "error"];
const CHILD_ACTS: readonly ChildMark["act"][] = ["asked", "answered", "steered"];

function waitOf(value: unknown): TurnWait | undefined {
  const wait = recordOf(value), kind = WAIT_KINDS.find(entry => entry === wait?.kind);
  if (!wait || !kind || !isText(wait.id) || !isText(wait.label)) return undefined;
  return { kind, id: wait.id, label: wait.label };
}

/** The mark an entry's data holds, or undefined when it is not one this build draws (an older or damaged entry). */
export function markOf(value: unknown): TranscriptMark | undefined {
  const data = recordOf(value);
  if (!data || !isNumber(data.at)) return undefined;
  const at = data.at;
  switch (data.kind) {
    case "turn_end": {
      const reason = REASONS.find(entry => entry === data.reason);
      if (!reason || !isNumber(data.durationMs) || !isNumber(data.toolCalls)) return undefined;
      const waits = Array.isArray(data.waits) ? data.waits.flatMap(wait => waitOf(wait) ?? []) : [];
      return {
        kind: "turn_end", reason, at, durationMs: data.durationMs, toolCalls: data.toolCalls, waits,
        ...(isNumber(data.notRun) ? { notRun: data.notRun } : {}), ...(isNumber(data.cost) ? { cost: data.cost } : {}),
        ...(isNumber(data.rebilledTokens) ? { rebilledTokens: data.rebilledTokens } : {}), ...(isText(data.note) ? { note: data.note } : {}),
      };
    }
    case "model": {
      if (!isText(data.model)) return undefined;
      const before = recordOf(data.was);
      const was = before && isText(before.model) ? { model: before.model, ...(isText(before.effort) ? { effort: before.effort } : {}) } : undefined;
      return { kind: "model", at, model: data.model, ...(isText(data.effort) ? { effort: data.effort } : {}), ...(was ? { was } : {}) };
    }
    case "note": {
      const tone = NOTE_TONES.find(entry => entry === data.tone);
      return tone && isText(data.text) ? { kind: "note", at, tone, text: data.text } : undefined;
    }
    case "child": {
      const act = CHILD_ACTS.find(entry => entry === data.act);
      return act && isText(data.child) && isText(data.text) ? { kind: "child", at, act, child: data.child, text: data.text } : undefined;
    }
    default:
      return undefined;
  }
}

/** The entry renderer for MARK_ENTRY: one quiet row per mark; nothing for data it does not recognise. */
export function renderMark(entry: CustomEntry<unknown>, options: Pick<EntryRenderOptions, "expanded">): Component | undefined {
  const mark = markOf(entry.data);
  return mark ? new MarkRows(markRow(mark), entry, options.expanded) : undefined;
}

/** Registers the entry renderer for MARK_ENTRY and returns the sink that appends marks. */
export function installMarkRows(pi: ExtensionAPI): MarkSink {
  pi.registerEntryRenderer(MARK_ENTRY, (entry, options) => renderMark(entry, options));
  return { add: mark => { pi.appendEntry(MARK_ENTRY, mark); return true; } };
}
