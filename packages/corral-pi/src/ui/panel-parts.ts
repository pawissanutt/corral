// Shared drawing vocabulary for the read-only inspection views (Proof, Inbox, Graph, Timeline, Usage): chips, the chip
// row with its freshness note, section rules with a right label, list rows with the selection bar, aligned list
// columns, label/value fields, short times and the read-state words (one wording per state, used by every view).
// Key hints live in the footer.
// Colours are the packaged PANEL_* tokens; colour is never the only signal.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_LINE, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_TINT,
  PANEL_WARNING,
} from "./panels-view.ts";
import type { ReadState } from "./read-state.ts";

export type Tone = "ok" | "bad" | "warn" | "none";
export const toneColor = (t: Tone) => t === "ok" ? PANEL_SUCCESS : t === "bad" ? PANEL_ERROR : t === "warn" ? PANEL_WARNING : PANEL_MUTED;
export const bold = (text: string) => `\x1b[1m${text}\x1b[22m`;
/** Ends a chip or selection background; the panel canvas maps the default back to its own surface. */
export const BG_END = "\x1b[49m";
export const wrap = (text: string, width: number) => wrapTextWithAnsi(text, Math.max(1, width));

/** A chip: glyph and word on a tone-tinted surface, coloured by tone. */
export function chip(text: string, tone: Tone = "none"): string {
  const back = tone === "ok" ? PANEL_TINT.success : tone === "bad" ? PANEL_TINT.error : tone === "warn" ? PANEL_TINT.warning : PANEL_TINT.raised;
  return `${back}${tone === "none" ? PANEL_TEXT : toneColor(tone)} ${plainLabel(text)} ${BG_END}${PANEL_TEXT}`;
}
export function chipRows(chips: readonly string[], width: number): string[] {
  const rows: string[] = [];
  let row = "";
  for (const c of chips) {
    const piece = visibleWidth(c) > width ? truncateToWidth(c, width, "…") : c;
    if (row && visibleWidth(row) + 1 + visibleWidth(piece) > width) { rows.push(row); row = ""; }
    row = row ? `${row} ${piece}` : piece;
  }
  if (row) rows.push(row);
  return rows;
}
/**
 * A section heading in muted bold followed by a rule in the line colour to `width`. `right` lists the label at the
 * rule's right end from the longest form to the shortest; the first that leaves at least three rule cells is drawn,
 * none when even the shortest does not fit. A form may carry its own colour; it starts muted.
 */
export function sectionRule(label: string, width: number, right?: readonly string[]): string {
  const head = plainLabel(label).toUpperCase(), room = width - visibleWidth(head) - 1;
  const note = right?.find(form => form && visibleWidth(form) + 4 <= room);
  const dashes = note === undefined ? Math.max(0, room) : room - visibleWidth(note) - 1;
  return `${PANEL_MUTED}${bold(head)} ${PANEL_LINE}${"─".repeat(dashes)}${note === undefined ? "" : ` ${PANEL_MUTED}${note}`}${PANEL_TEXT}`;
}
/** A muted label column beside wrapped text; an empty value reads as `—`. */
export function field(label: string, value: string, width: number, labelWidth: number): string[] {
  return wrap(plainLabel(value) || "—", width - labelWidth - 1)
    .map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labelWidth)}${PANEL_TEXT} ${row}`);
}
/** One list-pane row `width` wide; the selected row carries the `▌` bar on the selection surface, accent while `focused`, else muted. */
export function listRow(left: string, right: string, selected: boolean, width: number, focused = true): string {
  const body = splitRow(left, right, width - 3);
  return selected ? `${PANEL_SELECTED}${focused ? PANEL_BLUE : PANEL_MUTED}▌${PANEL_TEXT} ${body} ${BG_END}` : `  ${body} `;
}
/**
 * A sub-view's back line on row 0: `‹ Parent` in accent, ` · ` muted, `title` bold, `right` muted at the right edge
 * (dropped first when narrow). `to` ends the hit range of `‹ Parent`, which starts at column 0.
 */
export function backLine(parent: string, title: string, right: string, width: number): { line: string; to: number } {
  const back = `‹ ${plainLabel(parent)}`;
  const left = `${PANEL_BLUE}${back}${PANEL_MUTED} · ${PANEL_TEXT}${bold(plainLabel(title))}${PANEL_TEXT}`;
  const room = width - visibleWidth(left) - 2;
  const tail = right && room >= 6 ? PANEL_MUTED + truncateToWidth(plainLabel(right), room, "…") + PANEL_TEXT : "";
  return { line: tail ? splitRow(left, tail, width) : pad(left, width), to: Math.min(width, visibleWidth(back)) };
}

/** One list column: a muted header label, a width in cells and an alignment. */
export interface Column { readonly label: string; readonly width: number; readonly align?: "right" }
/** A column layout candidate; the `flex` column takes the cells the others leave and needs at least its `width`. */
export interface ColumnLayout { readonly columns: readonly Column[]; readonly flex: number }
/**
 * The first layout whose flex column keeps at least its width inside `inner` cells (columns joined by two blanks),
 * with the flex column widened to fill; the last layout when none fits.
 */
export function fitColumns(layouts: readonly ColumnLayout[], inner: number): Column[] {
  let chosen: Column[] = [];
  for (const layout of layouts) {
    const fixed = layout.columns.reduce((sum, c, i) => sum + (i === layout.flex ? 0 : c.width), 0) + 2 * Math.max(0, layout.columns.length - 1);
    const room = inner - fixed, flex = layout.columns[layout.flex];
    chosen = layout.columns.map((c, i) => i === layout.flex ? { ...c, width: Math.max(1, room) } : c);
    if (flex && room >= flex.width) break;
  }
  return chosen;
}
/** `values` set into `columns`: each clipped with `…` and padded (right-aligned where asked), two blanks apart. */
export function cells(columns: readonly Column[], values: readonly string[]): string {
  return columns.map((c, i) => {
    const value = values[i] ?? "", fitted = pad(value, c.width);
    if (c.align !== "right") return fitted;
    const text = truncateToWidth(value, c.width, "…");
    return " ".repeat(Math.max(0, c.width - visibleWidth(text))) + text;
  }).join("  ");
}
/** The muted header row of a list, aligned with `listRow` rows of the same `columns` and `width`. */
export function headerRow(columns: readonly Column[], width: number): string {
  return listRow(PANEL_MUTED + cells(columns, columns.map(c => c.label)) + PANEL_TEXT, "", false, width);
}
/** Facts about a source that are not read phases. */
export interface StateFlags { /** The data comes from a host that is not bound. */ readonly unbound?: boolean }
/**
 * The chip rows of a view: the state's own chips (`stateChip`) first, then the caller's source, count and gap chips,
 * with the freshness note right-aligned on the last row. The note is the longest `stateNote` form that fits the
 * width; when it does not fit beside the last chip row it takes a row of its own, so chips wrap before the note
 * shortens and the time of the data on screen stays visible. A chip already names reading or stale, so the note does
 * not repeat it. Not applicable: no state chip and no note.
 */
export function chipHeader(chips: readonly string[], width: number, state: ReadState, now: number, flags?: StateFlags): string[] {
  const rows = chipRows([...stateChips(state, flags), ...chips], width);
  const forms = notes(state, now, true);
  const form = forms.find(note => visibleWidth(note) <= width) ?? forms.at(-1);
  if (form === undefined) return rows;
  const note = PANEL_MUTED + truncateToWidth(form, Math.max(1, width), "…") + PANEL_TEXT, last = rows.at(-1);
  if (last !== undefined && visibleWidth(last) + 2 + visibleWidth(note) <= width) return [...rows.slice(0, -1), splitRow(last, note, width)];
  return [...rows, splitRow("", note, width)];
}

export function ago(at: number, now: number): string | undefined {
  if (!Number.isFinite(at)) return undefined;
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  return seconds < 60 ? `${seconds}s ago` : seconds < 3600 ? `${Math.floor(seconds / 60)}m ago`
    : seconds < 86_400 ? `${Math.floor(seconds / 3600)}h ago` : `${Math.floor(seconds / 86_400)}d ago`;
}
/** `HH:MM` local time, or `HH:MM:SS` with `seconds`. */
export function clock(at: number, seconds = false): string {
  const d = new Date(at), two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}${seconds ? `:${two(d.getSeconds())}` : ""}`;
}

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
/** `text` wrapped in `color` with its rows after the first indented two cells, under the words after a leading glyph. */
export function glyphRows(text: string, width: number, color: string = PANEL_MUTED): string[] {
  return wrap(plainLabel(text), Math.max(1, width - 2)).map((row, i) => `${color}${i ? "  " : ""}${row}${PANEL_TEXT}`);
}
/**
 * The freshness note of a source at the right of its section rule, longest form first. The shortest form keeps the
 * time of the data on screen (`2m ago`), never only the key; a retained read names its time in every form.
 */
export function stateNote(state: ReadState, now: number): readonly string[] {
  return notes(state, now, false);
}
/** Note forms; `chipped` when a chip beside the note already names the state (`reading… rows kept`, `! stale · 12:04`). */
function notes(state: ReadState, now: number, chipped: boolean): readonly string[] {
  const at = state.readAt, since = at === undefined ? undefined : ago(at, now);
  const reading = state.refreshing ? "reading… · " : "";
  switch (state.phase) {
    case "not_applicable": return [];
    case "loading": return [chipped ? "not read yet" : "reading…"];
    case "never_read": return ["not read yet · r read", "not read yet"];
    case "failed": return state.refreshing ? ["reading…"] : ["r retry"];
    case "current": {
      const lead = chipped ? "" : reading;
      if (at === undefined || since === undefined) return [`${lead}r reread`];
      return lead ? [`reading… · read ${clock(at)} (${since})`, `reading… · ${since}`, since]
        : [`read ${clock(at)} (${since}) · r reread`, `read ${since} · r reread`, since];
    }
    case "retained": {
      // On a chip row the chip says stale and names the time; the note keeps the time and its age.
      const stale = chipped ? "" : `${PANEL_WARNING}! stale${PANEL_MUTED} · `;
      if (at === undefined || since === undefined) return chipped ? [`${reading}r reread`] : [`${reading}${stale}r reread`, `${PANEL_WARNING}! stale${PANEL_MUTED}`];
      return [`${reading}${stale}last good read ${clock(at)} (${since}) · r reread`, `${reading}${stale}last good ${clock(at)} (${since})`,
        `${stale}${clock(at)} (${since})`, `${stale}${clock(at)}`];
    }
  }
}
/** The chips that name a state, then the host-unbound chip when flagged; empty when nothing needs saying. */
function stateChips(state: ReadState, flags?: StateFlags): string[] {
  const own = state.phase === "failed" ? chip("! read failed", "warn")
    : state.phase === "retained" ? chip(`! stale${state.readAt === undefined ? "" : ` · ${clock(state.readAt)}`}`, "warn")
      : state.phase === "loading" ? chip("reading…")
        : state.phase === "current" && state.refreshing ? chip("reading… rows kept") : undefined;
  return [...(own === undefined ? [] : [own]), ...(flags?.unbound ? [chip("! host unbound", "warn")] : [])];
}
/**
 * The state chip of a source on a chip row, followed by `! host unbound` when flagged (a separate fact, never in place
 * of the state); undefined when nothing needs saying (current, not applicable, never read).
 */
export function stateChip(state: ReadState, flags?: StateFlags): string | undefined {
  const chips = stateChips(state, flags);
  return chips.length ? chips.join(" ") : undefined;
}
/**
 * The problem line of a source in warning: a failed read with its reason, or retained data with the time it was read
 * and why it may be out of date. One clipped row unless `full`; nothing for the other states.
 */
export function stateBanner(state: ReadState, subject: string, width: number, full = false): string[] {
  const read = state.readAt === undefined ? "" : clock(state.readAt);
  const text = state.phase === "failed" ? `! ${capital(subject)} could not be read: ${state.error ?? "no reason given"}`
    : state.phase !== "retained" ? undefined
      : state.error !== undefined ? `! Reread failed${state.failedAt === undefined ? "" : ` at ${clock(state.failedAt)}`}: ${state.error} · showing data read ${read}`
        : `! Showing data read ${read} · a newer read is due`;
  if (text === undefined) return [];
  const line = plainLabel(text);
  return full ? glyphRows(line, width, PANEL_WARNING) : [PANEL_WARNING + truncateToWidth(line, Math.max(1, width), "…") + PANEL_TEXT];
}
/**
 * The body of a source that has no data to show: why it does not apply (`why`), that it is being read, that it was
 * never read, or for a failed read what is not inferred (`why`, when given). Nothing while data shows.
 */
export function stateRow(state: ReadState, subject: string, width: number, why?: string): string[] {
  const text = state.phase === "not_applicable" ? `○ Does not apply: ${why ?? `this session has no ${subject}.`}`
    : state.phase === "loading" ? `Reading ${subject}…`
      : state.phase === "never_read" ? `○ ${capital(subject)} not read yet`
        : state.phase === "failed" ? why : undefined;
  return text === undefined ? [] : glyphRows(text, width);
}
/** A value the record predates, in place of the missing value: unknown, never zero. */
export function legacyRow(what: string, record: string, width: number): string[] {
  return glyphRows(`○ ${capital(what)} not recorded by this ${record} · unknown, not zero`, width);
}
