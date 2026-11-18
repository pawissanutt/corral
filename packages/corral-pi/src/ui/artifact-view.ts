import { createHash } from "node:crypto";
import { z } from "zod";
import { matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { pad, plainLabel, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { onSurface, VIEW_INSET } from "./frame.ts";
import {
  backLine, bold, cells, chip, chipHeader, fitColumns, headerRow, listRow, sectionRule, stateBanner, stateRow, toneColor, wrap, type Tone,
} from "./panel-parts.ts";
import { readState, type ReadState } from "./read-state.ts";
import { SPLIT_MIN } from "./proof-view.ts";

/**
 * Main-view columns from which a comparison puts A and B on one row; below, one row each. The workbench draws this
 * view VIEW_INSET cells narrower than the main view, so the comparison adds them back.
 */
export const COMPARE_ROW_MIN = 100;

const Label = z.string().max(4096);
/** `corral artifact list <task> --json`: the engine's artifacts of one task, oldest first. */
const Schema = z.array(z.object({
  task: Label, id: z.number().int().positive(), sha256: Label, bytes: z.number().int().nonnegative(),
  kind: Label.nullable(), title: Label, at: Label, by: z.enum(["owner", "agent", "reviewer", "assistant", "auto"]),
}));
export type ArtifactCatalog = z.infer<typeof Schema>;
type Entry = ArtifactCatalog[number];
/** The engine's reference of an artifact: `T-1:3`. */
const ref = (entry: Entry) => `${entry.task}:${entry.id}`;

const MAX_TEXT_LINES = 400;
/** List rows shown around the selection: beside the detail from SPLIT_MIN, above it below. */
const WINDOW_SPLIT = 12, WINDOW_STACKED = 8;
/** The one muted caveat line, longest first; `?` shows the full reading rules instead. */
const CAVEAT = [
  "Catalog at last read, not live · content is read only on Enter",
  "Catalog at last read, not live",
  "last read, not live",
  "not live",
];
/** What a read through the viewer showed about the registered bytes. */
type Check = "matched" | "mismatched" | "binary";
/** A muted label column beside wrapped text in `tone`. */
function toned(label: string, value: string, width: number, labels: number, tone: Tone = "none"): string[] {
  const color = tone === "none" ? PANEL_TEXT : toneColor(tone);
  return wrap(plainLabel(value) || "—", width - labels - 1).map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labels)} ${color}${row}${PANEL_TEXT}`);
}

export interface ArtifactViewOptions {
  readonly identity: HostIdentity;
  readonly load?: () => Promise<unknown>;
  /** `corral artifact show <reference>`: the artifact's bytes as UTF-8 text. */
  readonly viewer?: (reference: string) => Promise<string>;
  /** The existing `/corral-export` flow; its refusal message is shown verbatim. */
  readonly exportSession?: () => Promise<string>;
  readonly bound: () => boolean;
  readonly changed: () => void;
  readonly canInteract: () => boolean;
  /** The workbench's `?` legend toggle: the full reading rules replace the one-line caveat. */
  readonly legend?: () => boolean;
  /** True while the view has the keys: the selection bar is accent, otherwise muted. */
  readonly focused?: () => boolean;
}
interface Opened { readonly reference: string; readonly title: string; readonly lines: readonly string[] }

/**
 * Cached catalog of the bound task, newest first. Rendering and focus never read content: Enter is the only action
 * that does, and it acts on the selected identity at that moment; x runs the session export.
 */
export class ArtifactView implements Component {
  current: ArtifactCatalog | undefined;
  refreshing = false;
  /** The last read's failure in plain words, and when it failed; cleared by a successful read. */
  private error: string | undefined; private failedAt: number | undefined;
  marks: string[] = [];
  compare = false;
  notice = "";
  private selected: string | undefined;
  private opened: Opened | undefined;
  private checked = new Map<string, Check>();
  private active: Promise<void> | undefined;
  private disposed = false;
  private hits = new Map<number, string>();
  /** Columns of the list, where a click selects; the end of the `‹ Proof` hit range on row 0. */
  private hitWidth = 0; private backTo = 0;
  private readAt: number | undefined;
  constructor(private readonly options: ArtifactViewOptions) {}
  /** A catalog read exists for this session: a reader and a bound task. */
  get applicable(): boolean { return !!this.options.load && !!this.options.identity.task; }
  /** Where the catalog read stands. Retained rows keep their read time after a failed reread. */
  get state(): ReadState {
    return readState({ applicable: this.applicable, reason: this.options.load ? "no_task" : "no_reader", hasData: this.current !== undefined, stale: false,
      refreshing: this.refreshing, checkedAt: this.readAt, error: this.error, failedAt: this.failedAt });
  }
  /** Rereads the catalog; nothing to do when no read applies. */
  refresh(): Promise<void> {
    if (this.active || this.disposed || !this.applicable) return this.active ?? Promise.resolve();
    this.active = this.read().finally(() => { this.active = undefined; });
    return this.active;
  }
  private async read(): Promise<void> {
    const { identity, load } = this.options;
    if (!load) return;
    this.refreshing = true; this.options.changed();
    try {
      const data = Schema.parse(await load());
      const foreign = data.find(a => a.task !== identity.task);
      if (foreign) throw new Error(`the read returned ${foreign.task}, not this session's task`);
      if (this.disposed) return;
      this.current = data.toSorted((a, b) => b.id - a.id); this.error = undefined; this.failedAt = undefined; this.readAt = Date.now();
      const first = this.current[0];
      this.selected ??= first && ref(first);
    } catch (error) {
      if (this.disposed) return;
      this.error = error instanceof z.ZodError ? "the read returned data of an unexpected shape" : error instanceof Error ? error.message : String(error);
      this.failedAt = Date.now();
    }
    finally { this.refreshing = false; if (!this.disposed) this.options.changed(); }
  }
  private entry(reference: string | undefined): Entry | undefined {
    return reference === undefined ? undefined : this.current?.find(a => ref(a) === reference);
  }
  handleInput(data: string): boolean {
    if (!this.options.canInteract()) return false;
    const list = this.current ?? [];
    if (data === "j" || data === "k" || matchesKey(data, "down") || matchesKey(data, "up")) {
      const index = list.findIndex(a => ref(a) === this.selected);
      const next = list[Math.max(0, Math.min(list.length - 1, index + (data === "j" || matchesKey(data, "down") ? 1 : -1)))];
      if (next) this.selected = ref(next);
      this.opened = undefined; this.options.changed(); return true;
    }
    if (data === "r") { void this.refresh(); return true; }
    if (data === "m") { this.mark(); return true; }
    if (data === "c") {
      this.compare = !this.compare && this.marks.length === 2;
      this.notice = this.compare || this.marks.length === 2 ? "" : "Comparison needs two artifacts selected explicitly with m";
      this.options.changed(); return true;
    }
    if (matchesKey(data, "enter")) { void this.open(); return true; }
    if (data === "x") { void this.exportSession(); return true; }
    return false;
  }
  handleMouse(event: TuiMouseEvent) {
    if (!this.options.canInteract() || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const reference = this.hits.get(event.y);
    if (reference === undefined || event.x >= this.hitWidth) return undefined;
    if (reference !== this.selected) this.opened = undefined;
    this.selected = reference; this.options.changed(); return { handled: true };
  }
  private mark(): void {
    const reference = this.selected;
    if (!reference || !this.entry(reference)) { this.notice = "No artifact selected"; this.options.changed(); return; }
    if (this.marks.includes(reference)) this.marks = this.marks.filter(m => m !== reference);
    else if (this.marks.length < 2) this.marks = [...this.marks, reference];
    else this.notice = "Two artifacts already selected · m on a selected one clears it";
    if (this.marks.length < 2) this.compare = false;
    this.options.changed();
  }
  private say(text: string): void { if (!this.disposed) { this.notice = plainLabel(text).slice(0, 2000); this.options.changed(); } }
  /**
   * Reads the selected artifact through the viewer. Its bytes arrive decoded as UTF-8, so text shows only when
   * re-encoding them gives the registered SHA256; binary content (decoded with replacement characters) is not shown.
   */
  private async open(): Promise<void> {
    const entry = this.entry(this.selected);
    if (!entry) { this.say("No artifact selected"); return; }
    const reference = ref(entry);
    const show = (lines: string[]) => { if (!this.disposed && this.selected === reference) { this.opened = { reference, title: entry.title, lines }; this.notice = ""; this.options.changed(); } };
    if (!this.options.viewer) { show(["Viewer unavailable in this host"]); return; }
    try {
      const text = await this.options.viewer(reference);
      const hash = createHash("sha256").update(text, "utf8").digest("hex");
      const check: Check = hash === entry.sha256 ? "matched" : text.includes("\uFFFD") ? "binary" : "mismatched";
      this.checked.set(reference, check);
      if (check === "binary") { show(["Binary content · not shown in the terminal", `corral artifact show ${reference} writes its ${entry.bytes.toLocaleString("en-US")} bytes to stdout`]); return; }
      if (check === "mismatched") { show(["Not displayed: bytes differ from the registered artifact", `MISMATCH: bytes at open hash ${hash}; registered ${entry.sha256}`]); return; }
      const lines = text.split("\n").map(line => line.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, " "));
      show([...lines.slice(0, MAX_TEXT_LINES), ...(lines.length > MAX_TEXT_LINES ? [`… ${lines.length - MAX_TEXT_LINES} more lines · corral artifact show ${reference}`] : [])]);
    } catch (error) { show([`Viewer unavailable: ${error instanceof Error ? error.message : String(error)}`]); }
  }
  private async exportSession(): Promise<void> {
    if (!this.options.exportSession) { this.say("Session export unavailable: session_export is not enabled for this role"); return; }
    this.say("Exporting through /corral-export…");
    try { this.say(await this.options.exportSession()); }
    catch (error) { this.say(`Session export refused: ${error instanceof Error ? error.message : String(error)}`); }
  }
  /** True when a click at (`x`, `y`) in this view's own cells lands on the `‹ Proof` back link of row 0. */
  backAt(x: number, y: number): boolean { return y === 0 && x >= 0 && x < this.backTo; }
  /** The selected artifact: title with its A/B slot, label/value rows and what Enter opened. */
  private detail(entry: Entry, width: number): string[] {
    const reference = ref(entry), slot = this.marks.indexOf(reference);
    const tags = slot < 0 ? "" : chip(slot ? "B" : "A");
    const out = [`${PANEL_TEXT}${bold(truncateToWidth(plainLabel(entry.title), Math.max(1, width - visibleWidth(tags) - 2), "…"))}${tags ? `  ${tags}` : ""}`];
    const check = this.checked.get(reference), labels = 10;
    const digest = check === undefined ? "bytes not re-hashed" : check === "matched" ? "bytes matched at open"
      : check === "binary" ? "binary content · not re-hashed as text" : "BYTES MISMATCHED at open";
    out.push(...toned("Reference", reference, width, labels), ...toned("Kind", entry.kind ?? "", width, labels),
      ...toned("By", entry.by, width, labels), ...toned("Added", entry.at, width, labels),
      ...toned("Size", `${entry.bytes.toLocaleString("en-US")} bytes`, width, labels),
      ...toned("Digest", `${entry.sha256.slice(0, 12)} · ${digest}`, width, labels, check === "mismatched" ? "warn" : "none"),
      ...toned("SHA256", entry.sha256, width, labels));
    if (this.opened?.reference === reference) {
      out.push(sectionRule("Opened", width), ...wrap(`${PANEL_MUTED}${plainLabel(this.opened.title)} · read at action time${PANEL_TEXT}`, width));
      for (const line of this.opened.lines) out.push(...(line.trim() ? wrap(PANEL_TEXT + line.replaceAll("\t", "  "), width) : [""]));
    }
    return out;
  }
  /** A and B in the owner's selection order; one row per field when `side`, else one row each. */
  private comparison(width: number, side: boolean): string[] {
    const out = [sectionRule("Compare", width), ...wrap(`${PANEL_MUTED}A and B in your selection order · no before/after, candidate order or freshness implied${PANEL_TEXT}`, width)];
    const [a, b] = this.marks.map(m => this.entry(m));
    if (!a || !b) return [...out, ...wrap(`${PANEL_WARNING}! A selected artifact is no longer in the catalog · comparison unavailable${PANEL_TEXT}`, width)];
    out.push(...wrap(PANEL_TEXT + (a.sha256 === b.sha256 ? "Registered bytes identical (same SHA256)" : "Registered bytes differ (different SHA256)"), width));
    const fields = [["Title", (e: Entry) => e.title], ["Kind", (e: Entry) => e.kind ?? "—"], ["By", (e: Entry) => e.by], ["Added", (e: Entry) => e.at],
      ["Bytes", (e: Entry) => e.bytes.toLocaleString("en-US")], ["SHA256", (e: Entry) => e.sha256.slice(0, 12)]] as const;
    if (!side) {
      for (const [label, pick] of fields) out.push(...toned(`${label} A`, pick(a), width, 11), ...toned(`${label} B`, pick(b), width, 11));
      return [...out, ""];
    }
    const column = Math.max(4, Math.floor((width - 9 - 3) / 2));
    out.push(`${PANEL_MUTED}${pad("", 8)} ${pad(bold("A"), column)}${PANEL_RULE} │ ${PANEL_MUTED}${bold("B")}${PANEL_TEXT}`);
    for (const [label, pick] of fields)
      out.push(`${PANEL_MUTED}${pad(label, 8)}${PANEL_TEXT} ${pad(plainLabel(pick(a)), column)}${PANEL_RULE} │ ${PANEL_TEXT}${truncateToWidth(plainLabel(pick(b)), column, "…")}`);
    return [...out, ""];
  }
  /** The list: a muted header row, then a window of `window` rows around the selection with `↑`/`↓ n more`. */
  private list(items: readonly Entry[], index: number, width: number, window: number): { readonly text: string; readonly reference?: string }[] {
    const kind = { label: "kind", width: Math.min(12, Math.max(4, ...items.map(a => visibleWidth(plainLabel(a.kind ?? "—"))))) };
    const title = { label: "title", width: 10 }, by = { label: "by", width: Math.max(2, ...items.map(a => a.by.length)) };
    const columns = fitColumns([{ columns: [kind, title, by], flex: 1 }, { columns: [title, by], flex: 0 }], width - 3);
    const focused = this.options.focused?.() ?? true;
    const start = Math.max(0, Math.min(index - Math.floor(window / 3), items.length - window));
    const rows: { text: string; reference?: string }[] = [{ text: headerRow(columns, width) }];
    if (start > 0) rows.push({ text: listRow(`${PANEL_MUTED}↑ ${start} more${PANEL_TEXT}`, "", false, width) });
    for (const a of items.slice(start, start + window)) {
      const reference = ref(a), slot = this.marks.indexOf(reference), on = a === items[index];
      const values: Readonly<Record<string, string>> = {
        kind: PANEL_MUTED + plainLabel(a.kind ?? "—"), by: PANEL_MUTED + a.by,
        title: (slot < 0 ? "" : chip(slot ? "B" : "A") + " ") + PANEL_TEXT + plainLabel(a.title),
      };
      const text = cells(columns, columns.map(c => values[c.label] ?? "")) + PANEL_TEXT;
      rows.push({ text: listRow(on ? onSurface(text, PANEL_SELECTED) : text, "", on, width, focused), reference });
    }
    const after = items.length - start - window;
    if (after > 0) rows.push({ text: listRow(`${PANEL_MUTED}↓ ${after} more${PANEL_TEXT}`, "", false, width) });
    return rows;
  }
  render(width: number): string[] {
    const lines: string[] = []; this.hits.clear();
    const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
    const { identity } = this.options, data = this.current, items = data ?? [];
    const back = backLine("Proof", "Artifacts", identity.task ?? "no task", width);
    this.backTo = back.to; lines.push(back.line);
    const state = this.state, legend = this.options.legend?.() ?? false;
    lines.push(...chipHeader([
      ...(state.phase === "not_applicable" ? [] : [chip("canonical catalog")]),
      ...(data ? [chip(`${items.length} ${items.length === 1 ? "artifact" : "artifacts"}, newest first`)] : []),
      ...this.marks.map((m, i) => chip(`${i ? "B" : "A"} ${stripTerminalSequences(truncateToWidth(plainLabel(this.entry(m)?.title ?? m), 24, "…"))}`)),
    ], width, state, Date.now(), { unbound: !this.options.bound() }));
    lines.push(...stateBanner(state, "artifact catalog", width, legend));
    if (this.notice) full(PANEL_TEXT + plainLabel(this.notice));
    if (legend) {
      full(`${PANEL_MUTED}Read-only canonical catalog at the last read, not a live subscription. Content is read only on Enter, never on focus or selection.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}Enter shows text whose bytes match the registered SHA256; binary content is not shown in the terminal. By names who registered the artifact.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}A and B mark the two artifacts chosen with m, in your selection order.${PANEL_TEXT}`);
    } else {
      const caveat = CAVEAT.find(c => visibleWidth(c) <= width) ?? CAVEAT.at(-1) ?? "";
      lines.push(PANEL_MUTED + truncateToWidth(caveat, width, "…") + PANEL_TEXT);
    }
    if (!data) {
      const why = state.phase === "failed" ? "No catalog shown · nothing is inferred from its absence"
        : identity.task ? "this host has no artifact catalog reader." : "this session has no bound task.";
      lines.push(...stateRow(state, "artifact catalog", width, why));
      return lines;
    }
    if (!items.length) { full(`${PANEL_MUTED}No artifacts registered for this task${PANEL_TEXT}`); return lines; }
    if (this.selected !== undefined && !this.entry(this.selected)) full(`${PANEL_WARNING}! Selected artifact no longer in the catalog · j/k selects an existing row${PANEL_TEXT}`);
    const index = Math.max(0, items.findIndex(a => ref(a) === this.selected)), entry = items[index];
    const wide = width + VIEW_INSET >= SPLIT_MIN;
    const listWidth = wide ? Math.max(46, Math.min(60, Math.floor(width * 0.45))) : width;
    const paneWidth = wide ? Math.max(20, width - listWidth - 2) : width;
    const list = this.list(items, index, listWidth, wide ? WINDOW_SPLIT : WINDOW_STACKED);
    const pane = [...(this.compare ? this.comparison(paneWidth, width + VIEW_INSET >= COMPARE_ROW_MIN) : []), ...(entry ? this.detail(entry, paneWidth) : [])];
    this.hitWidth = listWidth;
    if (wide) {
      lines.push(`${PANEL_RULE}${"─".repeat(listWidth)}┬${"─".repeat(Math.max(0, width - listWidth - 1))}${PANEL_TEXT}`);
      const start = lines.length;
      for (let i = 0; i < Math.max(list.length, pane.length); i++) {
        const row = list[i];
        if (row?.reference !== undefined) this.hits.set(start + i, row.reference);
        lines.push(`${row?.text ?? " ".repeat(listWidth)}${PANEL_RULE}│${PANEL_TEXT} ${pane[i] ?? ""}`);
      }
    } else {
      for (const row of list) { if (row.reference !== undefined) this.hits.set(lines.length, row.reference); lines.push(row.text); }
      lines.push(`${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`, ...pane);
    }
    return lines.map(line => truncateToWidth(line, width, "…"));
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; }
}
