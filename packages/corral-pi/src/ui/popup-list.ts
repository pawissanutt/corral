// A filterable list on the house popup (frame.ts `popup`), shared by the History list and the assistant resume
// chooser: the title set into the top border with the position `a–b / n` while the list scrolls, the filter field
// (`› query`, the real cursor, `right()` on the right) over a divider, then the entries. An entry may take several
// rows; the selected one is drawn on the selected surface with the accent bar, and when it is taller than the list
// PgUp/PgDn and the wheel move through it before they move the selection, so every row stays reachable.
import {
  Input, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi,
  type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { keyCap, onSurface, popup } from "./frame.ts";
import { pagerPosition } from "./pager.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_RAISED, PANEL_RULE, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

export interface PopupRow<T> {
  readonly value: T;
  /** The words the filter matches, besides the unavailable reason. */
  readonly match: string;
  /** The entry's rows, each at most `width` cells; `selected` when it is the selected entry. */
  lines(width: number, selected: boolean): readonly string[];
  /** Why the entry cannot be chosen; drawn under its first row. It stays selectable, never choosable. */
  readonly disabled?: string;
}
/** Every field is read again at each render, so getters may change the title, field or rows in place. */
export interface PopupListOptions<T> {
  readonly title: string;
  readonly right?: () => string;
  readonly placeholder: string;
  /** Muted rows under the list: what stays true while the list is open. */
  readonly note?: string;
  /** The Enter verb; when set, a last row shows `⏎ verb  esc cancel  ↑↓ select`. */
  readonly verb?: string;
  /** The row shown when nothing matches the filter. */
  readonly empty?: string;
  rows(): readonly PopupRow<T>[];
  /** Body rows below the divider (list, note and hint); the four rows of frame around them are extra. */
  capacity(): number;
  /** Sees every key first, with the selected value; true when it used the key. */
  key?(data: string, selected: T | undefined): boolean;
  done(value: T | undefined): void;
  changed(): void;
}

/** Rows above the body: the top border, the filter row and its divider. */
const HEAD_ROWS = 3;
/** Cells the filter keeps before `right()` is clipped. */
const FIELD_MIN = 12;
/** Body cells left of the row text: the side border and the selection bar. */
const ROW_X = 2;
/** Body-row hit markers besides an entry index. */
const NO_ENTRY = -1, HINT_ROW = -2;
/** A cap in the hint row: the key it presses (none for the arrows) and the cells it covers. */
interface HintCap { readonly key: "enter" | "escape" | undefined; readonly from: number; readonly to: number }

export class PopupList<T> implements Component, Focusable {
  private input: Input;
  private placeholder: string;
  /** The selected entry among the matches, kept by value across rereads; its rows scrolled by `within`. */
  private index = 0;
  private value: T | undefined;
  private within = 0;
  private start = 0;
  /** As last drawn: the list rows, the selected entry's height, the entries visible, and what each body row is. */
  private budget = 1;
  private height = 0;
  private visible = 1;
  private hits: readonly number[] = [];
  private caps: readonly HintCap[] = [];
  private finished = false;
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  constructor(private readonly options: PopupListOptions<T>) {
    this.placeholder = options.placeholder;
    this.input = this.field(options.placeholder);
  }
  /** The filter text as typed. */
  get query(): string { return this.input.getValue(); }
  /** The selected entry's value among the current matches. */
  get selected(): T | undefined {
    const matches = this.matches();
    this.settle(matches);
    return matches[this.index]?.value;
  }
  /** Sets the filter to `query` and selects the first entry at the top; the caller asks for the render. */
  reset(query = ""): void {
    this.input.setValue(query);
    this.index = 0; this.value = undefined; this.within = 0; this.start = 0;
  }
  private field(placeholder: string): Input {
    return new Input({ prompt: PANEL_BLUE + "›" + PANEL_TEXT + " ", placeholder,
      placeholderStyle: text => PANEL_MUTED + text + PANEL_TEXT });
  }
  private matches(): readonly PopupRow<T>[] {
    const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
    const rows = this.options.rows();
    return words.length ? rows.filter(row => words.every(word => `${row.match} ${row.disabled ?? ""}`.toLowerCase().includes(word))) : rows;
  }
  /** Follows the selected value when the rows change under it; else keeps the place, clamped. */
  private settle(matches: readonly PopupRow<T>[]): void {
    const at = this.value === undefined ? -1 : matches.findIndex(row => Object.is(row.value, this.value));
    const next = at >= 0 ? at : Math.max(0, Math.min(this.index, matches.length - 1));
    const value = matches[next]?.value;
    if (!Object.is(value, this.value)) this.within = 0;
    this.index = next; this.value = value;
  }
  private move(matches: readonly PopupRow<T>[], to: number): void {
    this.index = Math.max(0, Math.min(matches.length - 1, to));
    this.value = matches[this.index]?.value; this.within = 0;
  }
  /** PgUp/PgDn: through a selected entry taller than the list first, then a page of entries. */
  private page(matches: readonly PopupRow<T>[], direction: 1 | -1): void {
    const room = this.height - this.budget, step = Math.max(1, this.budget - 1);
    if (direction > 0 && this.within < room) this.within = Math.min(room, this.within + step);
    else if (direction < 0 && this.within > 0) this.within = Math.max(0, this.within - step);
    else this.move(matches, this.index + direction * Math.max(1, this.visible - 1));
  }
  private choose(row: PopupRow<T> | undefined): void {
    if (row && !row.disabled) this.finish(row.value);
  }
  private finish(value: T | undefined): void {
    if (this.finished) return;
    this.finished = true; this.options.done(value);
  }
  handleInput(data: string): void {
    if (this.finished) return;
    const matches = this.matches();
    this.settle(matches);
    if (this.options.key?.(data, matches[this.index]?.value)) { this.options.changed(); return; }
    if (matchesKey(data, "escape")) { this.finish(undefined); return; }
    if (matchesKey(data, "enter")) { this.choose(matches[this.index]); return; }
    if (matchesKey(data, "up")) this.move(matches, this.index - 1);
    else if (matchesKey(data, "down")) this.move(matches, this.index + 1);
    else if (matchesKey(data, "pageUp")) this.page(matches, -1);
    else if (matchesKey(data, "pageDown")) this.page(matches, 1);
    else {
      const before = this.input.getValue();
      this.input.handleInput(data);
      if (this.input.getValue() !== before) { this.index = 0; this.value = undefined; this.within = 0; this.start = 0; }
    }
    this.options.changed();
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (this.finished) return { handled: true };
    const matches = this.matches();
    this.settle(matches);
    if (event.type === "wheel") {
      const delta = Math.trunc(event.wheelDelta ?? 0), room = this.height - this.budget;
      if (delta > 0 && this.within < room) this.within = Math.min(room, this.within + delta);
      else if (delta < 0 && this.within > 0) this.within = Math.max(0, this.within + delta);
      else if (delta) this.move(matches, this.index + Math.sign(delta));
      this.options.changed();
      return { handled: true };
    }
    // The filter row places the cursor; the Input counts its column after the two-cell prompt.
    if (event.y === 1 && event.type === "press") {
      const result = this.input.handleMouse({ ...event, x: event.x - ROW_X, y: 0 });
      this.options.changed();
      return result ?? { handled: true };
    }
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl || event.y < HEAD_ROWS) return { handled: true };
    const hit = this.hits[event.y - HEAD_ROWS] ?? NO_ENTRY;
    if (hit === HINT_ROW) {
      const cap = this.caps.find(item => event.x >= item.from && event.x < item.to);
      if (cap?.key === "escape") this.finish(undefined);
      else if (cap?.key === "enter") this.choose(matches[this.index]);
    } else if (hit === this.index) this.choose(matches[hit]);
    else if (hit >= 0 && hit < matches.length) { this.move(matches, hit); this.options.changed(); }
    return { handled: true };
  }
  /** The entry's rows with its unavailable reason, wrapped in warning tone, right under its first row. */
  private block(row: PopupRow<T>, width: number, selected: boolean): readonly string[] {
    const lines = row.lines(width, selected);
    if (!row.disabled) return lines.length ? lines : [""];
    const reason = wrapTextWithAnsi(`· unavailable: ${plainLabel(row.disabled)}`, width).map(line => PANEL_WARNING + line + PANEL_TEXT);
    return [lines[0] ?? "", ...reason, ...lines.slice(1)];
  }
  render(width: number): string[] {
    this.hits = []; this.caps = [];
    if (width < 6) return [];
    const options = this.options, inner = width - 2, text = inner - 1;
    if (options.placeholder !== this.placeholder) {
      const value = this.input.getValue(), focused = this.input.focused;
      this.placeholder = options.placeholder;
      this.input = this.field(options.placeholder); this.input.setValue(value); this.input.focused = focused;
    }
    const matches = this.matches();
    this.settle(matches);

    // The list keeps at least two rows: the hint row goes first, then the note shrinks to one row, then it goes.
    // The hint offers Enter only while the selected entry can be chosen.
    let budget = Math.max(1, Math.floor(options.capacity()));
    const choosable = matches[this.index] !== undefined && !matches[this.index]!.disabled;
    const hint = options.verb && budget - 1 >= 2 ? this.hint(plainLabel(options.verb), text, choosable) : undefined;
    if (hint) budget--;
    let note = options.note ? wrapTextWithAnsi(plainLabel(options.note), text).map(line => PANEL_MUTED + line + PANEL_TEXT) : [];
    if (budget - note.length < 2) note = budget - 1 >= 2 ? note.slice(0, 1) : [];
    budget -= note.length;

    const blocks = new Map<number, readonly string[]>();
    const block = (at: number): readonly string[] => {
      let lines = blocks.get(at);
      if (!lines) { lines = this.block(matches[at]!, text, at === this.index); blocks.set(at, lines); }
      return lines;
    };
    const body: string[] = [], hits: number[] = [];
    let selection: { from: number; to: number } | undefined, position = "";
    if (!matches.length) {
      body.push(PANEL_MUTED + plainLabel(options.empty ?? "nothing matches") + PANEL_TEXT); hits.push(NO_ENTRY);
      this.height = 0; this.visible = 1; this.within = 0; this.start = 0;
    } else {
      const own = block(this.index);
      this.height = own.length;
      if (own.length > budget) {
        // Taller than the list: the list is a window on this entry alone.
        this.within = Math.max(0, Math.min(own.length - budget, this.within));
        this.start = this.index; this.visible = 1;
        const rows = own.slice(this.within, this.within + budget);
        body.push(...rows); hits.push(...rows.map(() => this.index));
        selection = { from: 0, to: rows.length - 1 };
        position = `${pagerPosition(this.index, this.index, matches.length)} · ${pagerPosition(this.within, this.within + rows.length - 1, own.length)}`;
      } else {
        this.within = 0;
        this.start = Math.min(this.start, this.index);
        let used = 0;
        for (let at = this.start; at <= this.index; at++) used += block(at).length;
        while (used > budget) used -= block(this.start++).length;
        // Fill the rows left under the last entry by showing earlier ones.
        let below = used;
        for (let at = this.index + 1; at < matches.length && below < budget; at++) below += block(at).length;
        while (this.start > 0 && below + block(this.start - 1).length <= budget) below += block(--this.start).length;
        let last = this.start;
        for (let at = this.start; at < matches.length && body.length < budget; at++) {
          const rows = block(at).slice(0, budget - body.length);
          if (at === this.index) selection = { from: body.length, to: body.length + rows.length - 1 };
          body.push(...rows); hits.push(...rows.map(() => at)); last = at;
        }
        this.visible = last - this.start + 1;
        if (this.start > 0 || last < matches.length - 1) position = pagerPosition(this.start, last, matches.length);
      }
    }
    this.budget = budget;
    body.push(...note); hits.push(...note.map(() => NO_ENTRY));
    if (hint) { body.push(hint.text); hits.push(HINT_ROW); this.caps = hint.caps; }
    this.hits = hits;

    const wanted = options.right?.() ?? "", room = inner - 3 - FIELD_MIN;
    const right = wanted && room >= 4 ? (visibleWidth(wanted) > room ? truncateToWidth(wanted, room, "…") : wanted) : "";
    const field = this.input.render(Math.max(1, right ? inner - 3 - visibleWidth(right) : inner - 2))[0] ?? "";
    const frame = popup(field, right, body, width, selection);
    const title = plainLabel(options.title);
    if (title || position) frame[0] = this.border(title, position, width);
    return frame;
  }
  /** `╭─ title ──── a–b / n ─╮` in the rule colour on the raised surface: the title bold and clipped first. */
  private border(title: string, position: string, width: number): string {
    const border = PANEL_RAISED + PANEL_RULE;
    const spot = position && visibleWidth(position) + 3 <= width - 8 ? ` ${PANEL_MUTED}${position}${border} ─` : "";
    const spotWidth = spot ? visibleWidth(position) + 3 : 0;
    const room = width - 5 - spotWidth - 1;
    const label = room >= 3 && title ? truncateToWidth(title, room, "…") : "";
    const head = label ? `─ ${PANEL_TEXT}\x1b[1m${onSurface(label, PANEL_RAISED)}\x1b[22m${border} ` : "─";
    const used = 1 + (label ? 2 + visibleWidth(label) + 1 : 1) + spotWidth + 1;
    return `${border}╭${head}${"─".repeat(Math.max(1, width - used))}${spot}╮${PANEL_TEXT}${PANEL_BG}`;
  }
  /** `⏎ verb  esc cancel  ↑↓ select` (no Enter while nothing choosable is selected), dropping the arrows, then the labels, to fit. */
  private hint(verb: string, width: number, choosable: boolean): { readonly text: string; readonly caps: readonly HintCap[] } | undefined {
    const items = [
      ...(choosable ? [{ key: "enter" as const, cap: "⏎", label: verb }] : []),
      { key: "escape" as const, cap: "esc", label: "cancel" },
      { key: undefined, cap: "↑↓", label: "select" },
    ];
    const keys = items.length - 1;
    const tiers = [{ count: items.length, labels: true }, { count: keys, labels: true }, { count: keys, labels: false }];
    for (const tier of tiers) {
      const shown = items.slice(0, tier.count);
      const cells = shown.map(item => visibleWidth(item.cap) + 2 + (tier.labels && item.label ? 1 + visibleWidth(item.label) : 0));
      if (cells.reduce((sum, size) => sum + size, 0) + 2 * (shown.length - 1) > width) continue;
      let x = ROW_X, text = "";
      const caps = shown.map((item, index) => {
        if (index) { text += "  "; x += 2; }
        text += keyCap(item.cap) + (tier.labels && item.label ? ` ${PANEL_MUTED}${item.label}${PANEL_TEXT}` : "");
        const cap = { key: item.key, from: x, to: x + cells[index]! };
        x += cells[index]!;
        return cap;
      });
      return { text, caps };
    }
    return undefined;
  }
  invalidate(): void { this.input.invalidate(); }
}
