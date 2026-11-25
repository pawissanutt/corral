// The submitted plan as a reading view. The document is drawn as Chat draws Markdown, one section after another,
// with its outline beside it when there is room: `[` and `]` step through the sections, a click on an outline row
// goes to it, and `d` shows what changed since the submission before. The owner's choices are numbered options
// under the document: a digit selects one and Enter carries it out, so no single key approves a plan. `c` marks the
// section being read for feedback; Keep planning hands the marked headings to the composer, where the owner's own
// message is the feedback. The view decides nothing itself: it returns the choice to whoever opened it.
import { getMarkdownTheme, type ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import {
  Markdown, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi,
  type Component, type MarkdownTheme, type OverlayOptions, type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { PLAN_OPTIONS, type PlanChoice } from "../host/plan-mode.ts";
import { keyCap, onSurface, rule, titledBox } from "./frame.ts";
import { lineDiff, planSections, planText, sectionChanges, type PlanSection, type SectionChange } from "./plan-doc.ts";
import {
  plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_HEAD, PANEL_LINE, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING,
} from "./panels-view.ts";

export interface PlanViewInput {
  readonly readonly?: boolean;
  readonly title: string; readonly revision: number;
  /** The model an execute choice would run on. */
  readonly model: string | undefined;
  /** The submission's text; undefined when its copy is missing or was changed. */
  readonly text: string | undefined;
  /** The submission before this one, when there was one to compare against. */
  readonly previous?: string | undefined;
}
/** A choice with the headings marked for feedback, or a request to change the model; undefined when closed. */
export type PlanViewResult = { readonly choice: PlanChoice; readonly quotes: readonly string[] } | { readonly model: true };

type Mode = "read" | "changes";
/** The drawn document for one width and mode; `starts[n]` is the first row of section n, undefined when not shown. */
interface Built { readonly width: number; readonly mode: Mode; readonly rows: readonly string[]; readonly starts: readonly (number | undefined)[] }
interface Cap { readonly key: string; readonly from: number; readonly to: number }
/** An option on the wrapped options rows: which one, the row it is on and the cells it covers. */
interface OptionCell { readonly index: number; readonly row: number; readonly from: number; readonly to: number }

const BOLD = "\x1b[1m", UNBOLD = "\x1b[22m", RESET = "\x1b[0m";
/** The outline column, the inner width it needs beside the document, and the fewest sections worth an outline. */
const RAIL = 30, RAIL_FROM = 96, RAIL_SECTIONS = 3;
/** Share of the terminal's rows the frame takes; the overlay allows a little more. */
const SHARE = 0.92;
/** Share of the terminal's rows the overlay shows of a frame; what it cuts off below is not on screen. */
const SEEN = 0.96;
const PLACE: OverlayOptions = { width: "96%", maxHeight: "96%", anchor: "center" };
/** Unchanged lines kept around a change, and the rows from which the options take a row each. */
const CONTEXT = 2, FULL_ROWS = 24;
const LABEL = 32;

const sectionName = (section: PlanSection) => plainLabel(section.title) || "Introduction";
/** Styled text clipped to `width` cells and padded to it. */
function cell(text: string, width: number): string {
  const clipped = visibleWidth(text) > width ? truncateToWidth(text, width, "…") : text;
  return clipped + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}
/** A changed section as its lines: added in green, removed in red, `CONTEXT` kept lines around each change. */
function diffRows(before: string, after: string, width: number): string[] {
  const lines = lineDiff(before, after), shown = lines.map(() => false);
  lines.forEach((line, index) => {
    if (line.kind === " ") return;
    for (let at = Math.max(0, index - CONTEXT); at <= Math.min(lines.length - 1, index + CONTEXT); at++) shown[at] = true;
  });
  const rows: string[] = [];
  let gap = false;
  lines.forEach((line, index) => {
    if (!shown[index]) { gap = true; return; }
    if (gap && rows.length) rows.push(PANEL_MUTED + "  ⋯" + PANEL_TEXT);
    gap = false;
    const color = line.kind === "+" ? PANEL_SUCCESS : line.kind === "-" ? PANEL_ERROR : PANEL_MUTED;
    const parts = line.text ? wrapTextWithAnsi(line.text, Math.max(1, width - 2)) : [""];
    parts.forEach((part, at) => rows.push(color + (at ? "  " : line.kind + " ") + part + PANEL_TEXT));
  });
  return rows;
}

export class PlanView implements Component {
  private readonly sections: readonly PlanSection[];
  private readonly changes: readonly SectionChange[] | undefined;
  private readonly before: readonly (PlanSection | undefined)[];
  private readonly removed: readonly PlanSection[];
  private mode: Mode = "read";
  private built: Built | undefined;
  /** The drawn document no longer matches what it shows (a mark, the theme): the next frame draws it again. */
  private stale = false;
  /** The section `[`, `]` or a click went to; scrolling by rows gives the place back to the rows on screen. */
  private cursor: number | undefined;
  private top = 0;
  private max = 0;
  private page = 1;
  /** The option a digit chose; Enter carries it out. */
  private selected: number | undefined;
  /** Sections marked for feedback, by index. */
  private readonly quoted = new Set<number>();
  private closed = false;
  // The last frame's geometry, for the pointer.
  private height = 0;
  private docTop = 0;
  private docRows = 0;
  private railWidth = 0;
  private railSections: readonly (number | undefined)[] = [];
  private optionRows = new Map<number, number>();
  private optionCells: readonly OptionCell[] = [];
  /** The first frame row of the options. */
  private optionTop = 0;
  /** The choices on screen in the last frame, and whether the model an execute choice runs on read whole in it. */
  private optionSeen = new Set<number>();
  private named = true;
  private caps: readonly Cap[] = [];

  constructor(private readonly input: PlanViewInput, private readonly terminalRows: () => number,
    private readonly changed: () => void, private readonly done: (result: PlanViewResult | undefined) => void,
    private readonly theme: () => MarkdownTheme = getMarkdownTheme) {
    this.sections = input.text === undefined ? [] : planSections(planText(input.text));
    const compared = input.previous === undefined || input.text === undefined ? undefined
      : sectionChanges(planSections(planText(input.previous)), this.sections);
    this.changes = compared?.changes; this.before = compared?.before ?? []; this.removed = compared?.removed ?? [];
  }

  /** The headings marked for feedback, in the document's order. */
  quotes(): string[] { return [...this.quoted].sort((a, b) => a - b).map(index => sectionName(this.sections[index]!)); }
  private counts(): { readonly changed: number; readonly added: number; readonly removed: number } {
    const count = (kind: SectionChange) => this.changes?.filter(change => change === kind).length ?? 0;
    return { changed: count("changed"), added: count("added"), removed: this.removed.length };
  }

  private build(width: number): Built {
    if (!this.stale && this.built?.width === width && this.built.mode === this.mode) return this.built;
    // The reader's place is a section and the rows read into it, which another width or mode still has.
    const was = this.built, at = was ? this.current(was) : undefined, began = at === undefined ? undefined : was?.starts[at];
    const into = began === undefined ? 0 : Math.max(0, this.top - began);
    this.stale = false;
    const rows: string[] = [], starts: (number | undefined)[] = this.sections.map(() => undefined);
    const drawn = (text: string) => new Markdown(text, 0, 0, this.theme()).render(width);
    // One at a time: a long section draws more rows than a call takes arguments.
    const add = (more: readonly string[]) => { for (const row of more) rows.push(row); };
    const head = (mark: string, color: string, section: PlanSection, note: string) =>
      color + BOLD + mark + " " + sectionName(section) + UNBOLD + PANEL_MUTED + " · " + note + PANEL_TEXT;
    if (this.input.text === undefined) {
      const message = this.input.readonly ? "The approved plan's copy is missing or was changed."
        : "The submitted plan's copy is missing or was changed. Choose Keep planning and have it submitted again.";
      rows.push(...wrapTextWithAnsi(PANEL_WARNING + message + PANEL_TEXT, width));
    } else if (this.mode === "read") {
      this.sections.forEach((section, index) => {
        if (rows.length) rows.push("");
        starts[index] = rows.length;
        if (this.quoted.has(index)) rows.push(PANEL_BLUE + "✎ marked for your feedback" + PANEL_TEXT);
        add(drawn(section.text));
      });
    } else {
      this.sections.forEach((section, index) => {
        const change = this.changes?.[index] ?? "same", old = this.before[index];
        if (change === "same") return;
        if (rows.length) rows.push("");
        starts[index] = rows.length;
        if (change === "added" || !old) { rows.push(head("+", PANEL_SUCCESS, section, "new section")); add(drawn(section.text)); }
        else { rows.push(head("~", PANEL_WARNING, section, "changed")); add(diffRows(old.text, section.text, width)); }
      });
      for (const gone of this.removed) rows.push(...(rows.length ? [""] : []), head("-", PANEL_ERROR, gone, "removed"));
      if (!rows.length) rows.push(PANEL_MUTED + `Nothing changed since revision ${this.input.revision - 1}.` + PANEL_TEXT);
    }
    this.built = { width, mode: this.mode, rows, starts };
    const start = at === undefined ? undefined : starts[at];
    if (was) this.top = start === undefined ? 0 : start + into;
    return this.built;
  }
  /** The section being read: the one a jump went to, else the last shown one that starts at or above the view's first row. */
  private current(built: Built): number | undefined {
    if (this.cursor !== undefined && built.starts[this.cursor] !== undefined) return this.cursor;
    let found: number | undefined;
    built.starts.forEach((start, index) => {
      if (start !== undefined && (found === undefined || start <= this.top)) found = index;
    });
    return found;
  }

  render(width: number): string[] {
    if (width < 24) { this.height = 0; this.caps = []; return []; }
    const inner = width - 4, total = Math.max(10, Math.floor(this.terminalRows() * SHARE));
    const rail = inner >= RAIL_FROM && this.sections.length >= RAIL_SECTIONS ? RAIL : 0;
    const built = this.build(inner - (rail ? rail + 3 : 0));
    // A row of its own needs room for an option's whole label; shorter or narrower, every option still shows under its digit.
    const readonly = this.input.readonly === true, full = total >= FULL_ROWS && inner >= 6 + LABEL;
    // Every choice and one row of the document come first; the facts take the rows that are left, and a model that
    // does not read whole in them is not executed on. Borders, the two rules and that one row are `fixed`.
    const fixed = readonly ? 4 : 5;
    let options = readonly ? [] : this.options(inner, full, true);
    let known = this.facts(inner, Math.max(1, total - fixed - options.length));
    if (!readonly && !known.named) {
      options = this.options(inner, full, false);
      known = this.facts(inner, Math.max(1, total - fixed - options.length));
    }
    const facts = known.rows;
    this.named = known.named;
    const docRows = Math.max(1, total - 3 - facts.length - (readonly ? 0 : 1 + options.length));
    this.max = Math.max(0, built.rows.length - docRows); this.page = Math.max(1, docRows - 1);
    this.top = Math.max(0, Math.min(this.max, this.top));
    const current = this.current(built);
    const outline = rail ? this.outline(rail, docRows, current, built) : undefined;
    const body: string[] = [...facts, rule(inner)];
    for (let index = 0; index < docRows; index++) {
      const row = built.rows[this.top + index] ?? "";
      body.push(outline ? cell(outline.rows[index] ?? "", rail) + PANEL_LINE + " │ " + PANEL_TEXT + row : row);
    }
    if (!this.input.readonly) body.push(rule(inner), ...options);
    const frame = titledBox("", body, width, { tone: "accent" });
    const last = Math.min(built.rows.length, this.top + docRows);
    const place = current === undefined || !this.sections.length ? "" : `§ ${current + 1}/${this.sections.length} · `;
    frame[0] = this.border(place + (built.rows.length ? `${this.top + 1}–${last} / ${built.rows.length}` : "0 / 0"), width);
    frame[frame.length - 1] = this.bottom(width);
    this.height = frame.length; this.docTop = facts.length + 2; this.docRows = docRows;
    this.railWidth = rail; this.railSections = outline?.sections ?? [];
    this.optionTop = this.docTop + docRows + 1;
    this.optionRows = new Map(full && options.length ? PLAN_OPTIONS.map((_, index) => [this.optionTop + 1 + index, index]) : []);
    if (full || !options.length) this.optionCells = [];
    // A choice the overlay cut off below is not one a digit may pick.
    const limit = Math.min(frame.length - 1, Math.floor(this.terminalRows() * SEEN));
    this.optionSeen = new Set(full ? [...this.optionRows].flatMap(([row, index]) => row < limit ? [index] : [])
      : this.optionCells.flatMap(item => this.optionTop + item.row < limit ? [item.index] : []));
    return frame;
  }
  /**
   * `revision 3 · <model> · 9 sections`, then what changed since the submission before, in warning while it shows.
   * Wrapped over at most `room` rows; `named` says the model an execute choice would run on reads whole in them.
   */
  private facts(inner: number, room: number): { readonly rows: string[]; readonly named: boolean } {
    const { revision, model, previous } = this.input, count = this.sections.length;
    const lead = PANEL_MUTED + `revision ${revision} · ${plainLabel(model ?? "") || "no model"}`;
    let text = lead + ` · ${count} ${count === 1 ? "section" : "sections"}`;
    if (previous !== undefined && this.changes) {
      const { changed, added, removed } = this.counts();
      const parts = [changed ? `${changed} changed` : "", added ? `${added} new` : "", removed ? `${removed} removed` : ""].filter(Boolean);
      text += ` · since revision ${revision - 1}: ` + (this.mode === "changes" ? PANEL_WARNING : "") + (parts.join(", ") || "nothing changed");
    }
    const rows = wrapTextWithAnsi(text + PANEL_TEXT, inner);
    if (rows.length <= room) return { rows, named: true };
    // Words wrap the same way at the start of a longer text: the lead's rows say whether the model is among those kept.
    return { rows: rows.slice(0, room), named: wrapTextWithAnsi(lead + " ·", inner).length <= room };
  }
  /** The outline column: a window of the sections around the one being read, each indented by its level. */
  private outline(width: number, rows: number, current: number | undefined, built: Built):
    { readonly rows: string[]; readonly sections: (number | undefined)[] } {
    const count = this.sections.length, first = Math.max(0, Math.min(count - rows, (current ?? 0) - Math.floor(rows / 2)));
    const levels = this.sections.map(section => section.level).filter(level => level > 0), base = levels.length ? Math.min(...levels) : 1;
    const drawn: string[] = [], sections: (number | undefined)[] = [];
    for (let index = first; index < Math.min(count, first + rows); index++) {
      const section = this.sections[index]!, change = this.changes?.[index] ?? "same";
      const mark = change === "added" ? PANEL_SUCCESS + "+" : change === "changed" ? PANEL_WARNING + "~" : " ";
      const here = index === current, hidden = built.starts[index] === undefined;
      const name = " ".repeat(Math.max(0, section.level - base) * 2) + sectionName(section) + (this.quoted.has(index) ? " ✎" : "");
      drawn.push((here ? PANEL_BLUE + "▌" : " ") + mark + " " + (here ? BOLD + PANEL_TEXT : hidden ? PANEL_MUTED : PANEL_TEXT)
        + truncateToWidth(name, Math.max(1, width - 3), "…") + UNBOLD + PANEL_TEXT);
      sections.push(hidden ? undefined : index);
    }
    return { rows: drawn, sections };
  }
  /**
   * The owner's choices. With room, a question row and one row per option: its digit as a key cap, its label and
   * what it does, the chosen one on the selected surface. On a short or narrow terminal, each digit with a short label,
   * wrapped over as many rows as they take, the chosen one in the active key cap and bold; then the hint.
   */
  private options(inner: number, full: boolean, named: boolean): string[] {
    const quotes = this.quoted.size, chosen = this.selected;
    const hint = !named ? "widen the terminal to execute" : chosen === undefined ? "press 1–4 to choose" : "Enter carries it out";
    if (!full) {
      const cells: OptionCell[] = [], rows: string[] = [];
      let text = "", used = 0;
      const flush = () => { if (used) rows.push(text); text = ""; used = 0; };
      PLAN_OPTIONS.forEach((option, index) => {
        const short = option.label.replace(/^Execute with /, "").replace(/ context$/, ""), label = short[0]!.toUpperCase() + short.slice(1);
        const piece = keyCap(String(index + 1), index === chosen) + " " + (index === chosen ? BOLD + PANEL_TEXT + label + UNBOLD : PANEL_MUTED + label + PANEL_TEXT);
        const size = visibleWidth(piece);
        if (used && used + 2 + size > inner) flush();
        const from = used ? used + 2 : 0;
        text += (used ? "  " : "") + piece; cells.push({ index, row: rows.length, from: 2 + from, to: 2 + from + size }); used = from + size;
      });
      if (used + 3 + hint.length > inner) flush();
      text += PANEL_MUTED + (used ? " · " : "") + hint + PANEL_TEXT; used += (used ? 3 : 0) + hint.length;
      flush();
      this.optionCells = cells;
      return rows;
    }
    const rows = [PANEL_HEAD + BOLD + "How should this plan proceed?" + UNBOLD + PANEL_MUTED + "  " + hint + PANEL_TEXT];
    PLAN_OPTIONS.forEach((option, index) => {
      const on = index === chosen, surface = on ? PANEL_SELECTED : PANEL_BG;
      const detail = option.id !== "revise" ? option.detail
        : quotes ? `Write your feedback in the composer · ${quotes} ${quotes === 1 ? "section" : "sections"} quoted there`
        : "Write your feedback in the composer; the review stays open until you send it";
      const text = (on ? PANEL_BLUE + "▌" : " ") + " " + keyCap(String(index + 1), false, surface) + " " + (on ? BOLD : "") + PANEL_TEXT
        + cell(option.label, LABEL) + UNBOLD + PANEL_MUTED + detail + PANEL_TEXT;
      rows.push(surface + onSurface(cell(text, inner), surface) + RESET + PANEL_TEXT + PANEL_BG);
    });
    return rows;
  }
  /** `╭─ Plan review · title ──── § 3/9 · 41–72 / 214 ─╮`: the title bold and clipped first, the place muted. */
  private border(place: string, width: number): string {
    const border = PANEL_BG + PANEL_BLUE;
    const spot = visibleWidth(place) + 3 <= width - 8 ? ` ${PANEL_MUTED}${place}${border} ─` : "";
    const spotWidth = spot ? visibleWidth(place) + 3 : 0, room = width - 5 - spotWidth - 1;
    const title = plainLabel(`${this.input.readonly ? "Approved plan" : "Plan review"} · ${this.input.title}`);
    const label = room >= 3 ? truncateToWidth(title, room, "…") : "";
    const head = label ? `─ ${PANEL_TEXT}${BOLD}${onSurface(label, PANEL_BG)}${UNBOLD}${RESET}${border} ` : "─";
    const used = 1 + (label ? 2 + visibleWidth(label) + 1 : 1) + spotWidth + 1;
    return `${border}╭${head}${"─".repeat(Math.max(1, width - used))}${spot}╮${PANEL_TEXT}${PANEL_BG}`;
  }
  /** The reading keys as caps, right-aligned in the bottom border; labels go first when it is narrow, then the leftmost caps. */
  private bottom(width: number): string {
    const items = [
      ...(this.sections.length > 1 ? [{ key: "]", cap: "[ ]", label: "section" }] : []),
      ...(!this.input.readonly && this.sections.length && this.mode === "read" ? [{ key: "c", cap: "c", label: "mark for feedback" }] : []),
      ...(this.changes ? [{ key: "d", cap: "d", label: this.mode === "changes" ? "whole plan" : "changes" }] : []),
      ...(this.input.readonly ? [] : [{ key: "m", cap: "m", label: "model" }]), { key: "escape", cap: "esc", label: "close" },
    ];
    const room = width - 5, border = PANEL_BG + PANEL_BLUE;
    const size = (shown: typeof items, labels: boolean) =>
      shown.reduce((sum, item, index) => sum + (index ? 2 : 0) + visibleWidth(item.cap) + 2 + (labels ? 1 + visibleWidth(item.label) : 0), 0);
    let shown = items;
    const labels = size(items, true) <= room;
    if (!labels) while (shown.length > 1 && size(shown, false) > room) shown = shown.slice(1);
    if (size(shown, labels) > room) { this.caps = []; return `${border}╰${"─".repeat(width - 2)}╯${PANEL_TEXT}${PANEL_BG}`; }
    const fill = room - size(shown, labels), caps: Cap[] = [];
    let x = 1 + fill + 1, text = "";
    shown.forEach((item, index) => {
      if (index) { text += "  "; x += 2; }
      const cells = visibleWidth(item.cap) + 2 + (labels ? 1 + visibleWidth(item.label) : 0);
      text += keyCap(item.cap) + (labels ? ` ${PANEL_MUTED}${item.label}${PANEL_TEXT}` : "");
      caps.push({ key: item.key, from: x, to: x + cells });
      x += cells;
    });
    this.caps = caps;
    return `${border}╰${"─".repeat(fill)} ${text}${border} ─╯${PANEL_TEXT}${PANEL_BG}`;
  }

  private scroll(to: number): void {
    const next = Math.max(0, Math.min(this.max, to));
    if (next === this.top) return;
    this.top = next; this.cursor = undefined; this.changed();
  }
  /** Section `index` becomes the one being read, at the view's first row when the document is long enough. */
  private jump(index: number, start: number): void {
    this.top = Math.max(0, Math.min(this.max, start)); this.cursor = index; this.changed();
  }
  /** The section `delta` shown sections from the one being read. */
  private step(delta: number): void {
    const built = this.built;
    if (!built) return;
    const shown = built.starts.flatMap((start, index) => start === undefined ? [] : [{ index, start }]);
    const at = shown.findIndex(entry => entry.index === this.current(built));
    // `[` inside a section goes to its own heading first.
    const back = delta < 0 && at >= 0 && this.cursor === undefined && this.top > shown[at]!.start ? 0 : delta;
    const next = shown[Math.max(0, Math.min(shown.length - 1, (at < 0 ? 0 : at) + back))];
    if (next) this.jump(next.index, next.start);
  }
  private goTo(section: number): void {
    const start = this.built?.starts[section];
    if (start !== undefined) this.jump(section, start);
  }
  private close(result: PlanViewResult | undefined): void {
    if (this.closed) return;
    this.closed = true; this.done(result);
  }
  private choose(index: number, confirm: boolean): void {
    const option = PLAN_OPTIONS[index];
    // Only a choice that is on screen, and an execute choice only while the model it would run on reads whole.
    if (!option || !this.optionSeen.has(index) || (option.id !== "revise" && !this.named)) return;
    if (confirm && this.selected === index) { this.close({ choice: option.id, quotes: this.quotes() }); return; }
    this.selected = index; this.changed();
  }
  private press(key: string): void {
    const built = this.built;
    if (key === "escape") this.close(undefined);
    else if (key === "m" && !this.input.readonly) this.close({ model: true });
    else if (key === "]") this.step(1);
    else if (key === "[") this.step(-1);
    else if (key === "d" && this.changes) { this.mode = this.mode === "read" ? "changes" : "read"; this.changed(); }
    else if (key === "c" && !this.input.readonly && built && this.mode === "read") {
      const at = this.current(built);
      if (at === undefined) return;
      if (!this.quoted.delete(at)) this.quoted.add(at);
      // The mark is a row of the document: it is drawn again, and the reader stays in the section.
      this.stale = true;
      this.changed();
    }
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape") || data === "q") { this.close(undefined); return; }
    if (matchesKey(data, "enter")) { if (!this.input.readonly && this.selected !== undefined) this.choose(this.selected, true); return; }
    if (/^[1-9]$/.test(data)) { if (!this.input.readonly) this.choose(Number(data) - 1, false); return; }
    if (matchesKey(data, "up") || data === "k") this.scroll(this.top - 1);
    else if (matchesKey(data, "down") || data === "j") this.scroll(this.top + 1);
    else if (matchesKey(data, "pageUp")) this.scroll(this.top - this.page);
    else if (matchesKey(data, "pageDown") || data === " ") this.scroll(this.top + this.page);
    else if (matchesKey(data, "home") || data === "g") this.scroll(0);
    else if (matchesKey(data, "end") || data === "G") this.scroll(this.max);
    else if (data === "[" || data === "]" || data === "c" || data === "d" || data === "m") this.press(data);
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type === "wheel") { this.scroll(this.top + (event.wheelDelta ?? 0)); return { handled: true }; }
    if (event.type !== "click" || event.button !== "left") return undefined;
    if (event.y === this.height - 1) {
      const cap = this.caps.find(item => event.x >= item.from && event.x < item.to);
      if (cap) this.press(cap.key);
      return { handled: true };
    }
    // A click selects, as a digit does; only Enter carries a choice out.
    const option = this.optionRows.get(event.y)
      ?? this.optionCells.find(item => event.y === this.optionTop + item.row && event.x >= item.from && event.x < item.to)?.index;
    if (option !== undefined) { this.choose(option, false); return { handled: true }; }
    if (event.y >= this.docTop && event.y < this.docTop + this.docRows && event.x >= 2 && event.x < 2 + this.railWidth) {
      const section = this.railSections[event.y - this.docTop];
      if (section !== undefined) this.goTo(section);
    }
    return { handled: true };
  }
  invalidate(): void { this.stale = true; }
}

/**
 * The plan review as an overlay over the session. Resolves the owner's choice, a request to change the model, or
 * undefined when they closed it or `signal` closed it because the review was settled elsewhere.
 */
export async function openPlanView(ui: ExtensionUIContext, input: PlanViewInput, signal?: AbortSignal): Promise<PlanViewResult | undefined> {
  if (signal?.aborted) return undefined;
  let close: (() => void) | undefined;
  const abort = () => close?.();
  signal?.addEventListener("abort", abort, { once: true });
  try {
    return await ui.custom<PlanViewResult | undefined>((tui, _theme, _keys, done) => {
      close = () => done(undefined);
      return new PlanView(input, () => tui.terminal.rows, () => tui.requestRender(), result => done(result));
    }, { overlay: true, overlayOptions: PLACE });
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}
