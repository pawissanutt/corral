// The one read-only pager: job output, peek, board task details, agent details, the transcript reader, recorded
// history, recent notices and the plan. A rounded accent box with the title and the position `a–b / n` set
// into its top border; ↑↓, PgUp/PgDn, Home/End and the wheel scroll it, q or Esc closes it. Lines are clipped with
// `…`, never wrapped, so numbered output stays aligned; `wrap` is for prose (the transcript, the plan) and
// counts wrapped rows. Heading lines draw bold in the head colour. `actions` are keys that close the pager and are
// returned; with `raw`, `d` switches in place between the readable lines and the exact records, each keeping its own
// place. Both draw as key caps in the bottom border, and a click on a cap presses its key.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import {
  matchesKey, sliceByColumn, truncateToWidth, visibleWidth, wrapTextWithAnsi,
  type Component, type OverlayOptions, type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { keyCap, onSurface, titledBox } from "./frame.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_HEAD, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

export interface PagerLine {
  readonly text: string;
  /** A section heading. */
  readonly heading?: boolean;
  /** Cells every wrapped continuation row of this line is indented by, so it hangs under its value column. */
  readonly hanging?: number;
}
export interface PagerAction { readonly key: string; readonly label: string }
export interface PagerInput {
  readonly title: string;
  readonly lines: readonly (string | PagerLine)[];
  /** The source line shown first; past the end opens at the end. */
  readonly start?: number;
  /** Wrap long lines to the frame instead of clipping them; for prose without line numbers. */
  readonly wrap?: boolean;
  /** Keys that close the pager and are returned; drawn as key caps in the bottom border. */
  readonly actions?: readonly PagerAction[];
  /** The exact records behind `lines`; `d` switches between the two in place and keeps the pager open. */
  readonly raw?: readonly (string | PagerLine)[];
}

/** Share of the terminal's rows the frame may take; the overlay itself allows 85%. */
const SHARE = 0.8;
const PLACE: OverlayOptions = { width: "90%", maxHeight: "85%", anchor: "center" };
const RESET = "\x1b[0m";
/** The key `d` presses when `raw` is set; Esc's own cap. */
const RAW = "d", CLOSE = "escape";

/** `a–b / n` over 1-based rows; one row reads `a / n`, nothing `0 / 0`. */
export function pagerPosition(first: number, last: number, count: number): string {
  if (count === 0 || last < first) return "0 / 0";
  return first === last ? `${first + 1} / ${count}` : `${first + 1}–${last + 1} / ${count}`;
}

/** `hanging`: the first row's prefix cells that continuation rows are indented by; 0 for the exact records. */
interface Line { readonly text: string; readonly heading: boolean; readonly hanging: number }
interface Row { readonly text: string; readonly heading: boolean; readonly line: number }
/** One line set (readable or exact) with its own drawn rows and its own place. */
interface Sheet {
  readonly source: readonly Line[];
  /** Exact records: every character kept, cut at the frame edge instead of word-wrapped or clipped. */
  readonly exact: boolean;
  readonly wrap: boolean;
  rows: readonly Row[];
  builtFor: number;
  /** The source line at the top (`source.length` = pinned to the end), so the place survives a resize. */
  anchor: number;
  /** The wanted top row; drawn clamped to the current height, so shrinking and growing back keeps it. */
  offset: number;
  /** The top row, last scroll limit and page as last drawn. */
  top: number;
  max: number;
  page: number;
}
/** A cap in the bottom border: the key it presses and the cells it covers. */
interface Cap { readonly key: string; readonly from: number; readonly to: number }

/**
 * Plain text on one row each: tabs become four blanks, any other control character a blank. A line's own breaks
 * after the first hang under its value column too; the exact records keep their text as given.
 */
function lines(input: readonly (string | PagerLine)[], exact: boolean): Line[] {
  return input.flatMap(entry => {
    const text = typeof entry === "string" ? entry : entry.text, heading = typeof entry !== "string" && entry.heading === true;
    const wanted = typeof entry === "string" ? 0 : entry.hanging ?? 0;
    const hanging = exact || !Number.isFinite(wanted) ? 0 : Math.max(0, Math.trunc(wanted));
    return text.split(/\r?\n/).map((part, index) => {
      const clean = part.replaceAll("\t", "    ").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
      return { text: index && hanging ? " ".repeat(hanging) + clean : clean, heading, hanging };
    });
  });
}
/** `text` cut every `width` cells with nothing dropped: no word wrap, no trimmed blanks, no ellipsis. */
function cut(text: string, width: number): string[] {
  const parts: string[] = [], total = visibleWidth(text);
  for (let column = 0; column < total;) {
    const part = sliceByColumn(text, column, width, true) || sliceByColumn(text, column, width);
    if (!part) break;
    parts.push(part); column += Math.max(1, visibleWidth(part));
  }
  return parts;
}

export class PagerView implements Component {
  private readonly readable: Sheet;
  private readonly exact: Sheet | undefined;
  private shown: Sheet;
  private readonly actions: readonly PagerAction[];
  private caps: readonly Cap[] = [];
  private height = 0;
  private closed = false;
  constructor(private readonly input: PagerInput, private readonly terminalRows: () => number,
    private readonly changed: () => void, private readonly done: (key?: string) => void) {
    const blank = { rows: [], builtFor: -1, offset: 0, top: 0, max: 0, page: 1 };
    this.readable = { ...blank, source: lines(input.lines, false), exact: false, wrap: input.wrap === true, anchor: Math.max(0, Math.trunc(input.start ?? 0)) };
    this.exact = input.raw ? { ...blank, source: lines(input.raw, true), exact: true, wrap: true, anchor: 0 } : undefined;
    this.shown = this.readable;
    // An action never shadows closing, `d` while there are exact records, or a scroll key; the first of a key wins.
    const seen = new Set<string>();
    this.actions = (input.actions ?? []).filter(({ key }) => {
      const reserved = key === "q" || (this.exact !== undefined && key === RAW) || matchesKey(key, "escape")
        || matchesKey(key, "up") || matchesKey(key, "down") || matchesKey(key, "pageUp") || matchesKey(key, "pageDown")
        || matchesKey(key, "home") || matchesKey(key, "end");
      if (!key || reserved || seen.has(key)) return false;
      seen.add(key); return true;
    });
  }
  /**
   * The drawn rows: exact records cut at the edge, prose wrapped, a hanging line wrapped after its first `hanging`
   * cells with its continuations indented under them (plain wrap when the frame is too narrow to hang), else clipped.
   */
  private build(target: Sheet, inner: number): void {
    const rows: Row[] = [];
    target.source.forEach((line, index) => {
      const total = visibleWidth(line.text), hang = line.hanging;
      let parts: string[];
      if (!line.text) parts = [""];
      else if (target.exact) parts = cut(line.text, inner);
      else if (!target.wrap) parts = [line.text];
      else if (hang > 0 && hang < inner - 8 && total > hang) {
        const head = sliceByColumn(line.text, 0, hang, true), indent = " ".repeat(hang);
        const value = wrapTextWithAnsi(sliceByColumn(line.text, hang, total - hang, true), inner - hang);
        parts = value.length ? value.map((part, at) => (at ? indent : head) + part) : [head];
      } else parts = wrapTextWithAnsi(line.text, inner);
      for (const part of parts.length ? parts : [""]) rows.push({ text: part, heading: line.heading, line: index });
    });
    target.rows = rows;
    const first = target.anchor >= target.source.length ? -1 : rows.findIndex(row => row.line >= target.anchor);
    target.offset = first < 0 ? rows.length : first;
  }
  render(width: number): string[] {
    if (width < 8) { this.caps = []; this.height = 0; return []; }
    const inner = width - 4, target = this.shown;
    if (target.builtFor < 0 || ((target.wrap || target.exact) && target.builtFor !== inner)) { this.build(target, inner); target.builtFor = inner; }
    const count = target.rows.length;
    const body = Math.max(1, Math.min(Math.max(1, Math.floor(this.terminalRows() * SHARE) - 2), count));
    target.page = Math.max(1, body - 1);
    target.max = Math.max(0, count - body);
    target.top = Math.max(0, Math.min(target.max, target.offset));
    const shown = count
      ? target.rows.slice(target.top, target.top + body).map(row => row.heading ? `${PANEL_HEAD}\x1b[1m${row.text}\x1b[22m${PANEL_TEXT}` : row.text)
      : [PANEL_MUTED + "· no lines"];
    const last = Math.min(count, target.top + body) - 1;
    const frame = titledBox("", shown, width, { tone: "accent" });
    frame[0] = this.border(pagerPosition(target.top, last, count), width);
    const bottom = this.bottom(width);
    if (bottom) frame[frame.length - 1] = bottom;
    this.height = frame.length;
    return frame;
  }
  /** `╭─ title ──── a–b / n ─╮` in accent: the title bold and clipped first, the position muted. */
  private border(position: string, width: number): string {
    const border = PANEL_BG + PANEL_BLUE;
    const spot = visibleWidth(position) + 3 <= width - 8 ? ` ${PANEL_MUTED}${position}${border} ─` : "";
    const spotWidth = spot ? visibleWidth(position) + 3 : 0;
    const room = width - 5 - spotWidth - 1;
    const title = plainLabel(this.input.title);
    const label = room >= 3 && title ? truncateToWidth(title, room, "…") : "";
    const head = label ? `─ ${PANEL_TEXT}\x1b[1m${onSurface(label, PANEL_BG)}\x1b[22m${RESET}${border} ` : "─";
    const used = 1 + (label ? 2 + visibleWidth(label) + 1 : 1) + spotWidth + 1;
    return `${border}╭${head}${"─".repeat(Math.max(1, width - used))}${spot}╮${PANEL_TEXT}${PANEL_BG}`;
  }
  /**
   * `╰──── t transcript  d exact records  esc close ─╯`: the caps right-aligned in the accent border, `d` in its
   * active form while the exact records show. Labels go first when the row is narrow, then the leftmost caps; Esc
   * stays. None when the pager has neither actions nor exact records, so the plain pagers keep their plain border.
   */
  private bottom(width: number): string | undefined {
    this.caps = [];
    if (!this.actions.length && !this.exact) return undefined;
    const items = [
      ...this.actions.map(action => ({ key: action.key, cap: plainLabel(action.key), label: plainLabel(action.label), on: false })),
      ...(this.exact ? [{ key: RAW, cap: RAW, label: "exact records", on: this.shown === this.exact }] : []),
      { key: CLOSE, cap: "esc", label: "close", on: false },
    ];
    const room = width - 5;
    const size = (shown: typeof items, labels: boolean) =>
      shown.reduce((sum, item, index) => sum + (index ? 2 : 0) + visibleWidth(item.cap) + 2 + (labels && item.label ? 1 + visibleWidth(item.label) : 0), 0);
    let shown = items, labels = size(items, true) <= room;
    if (!labels) while (shown.length > 1 && size(shown, false) > room) shown = shown.slice(1);
    if (size(shown, labels) > room) return undefined;
    const border = PANEL_BG + PANEL_BLUE;
    const fill = room - size(shown, labels);
    let x = 1 + fill + 1, text = "";
    const caps: Cap[] = [];
    shown.forEach((item, index) => {
      if (index) { text += "  "; x += 2; }
      const cells = visibleWidth(item.cap) + 2 + (labels && item.label ? 1 + visibleWidth(item.label) : 0);
      text += keyCap(item.cap, item.on) + (labels && item.label ? ` ${PANEL_MUTED}${item.label}${PANEL_TEXT}` : "");
      caps.push({ key: item.key, from: x, to: x + cells });
      x += cells;
    });
    this.caps = caps;
    return `${border}╰${"─".repeat(fill)} ${text}${border} ─╯${PANEL_TEXT}${PANEL_BG}`;
  }
  private scroll(to: number): void {
    const target = this.shown;
    const next = Math.max(0, Math.min(target.max, to));
    if (next === target.top) return;
    const end = next >= target.max;
    target.top = next;
    target.offset = end ? target.rows.length : next;
    target.anchor = end ? target.source.length : target.rows[next]?.line ?? 0;
    this.changed();
  }
  private close(key?: string): void {
    if (this.closed) return;
    this.closed = true; this.done(key);
  }
  /** Esc, `d` or an action's key, as typed or clicked. */
  private press(key: string): void {
    if (key === CLOSE) this.close();
    else if (key === RAW && this.exact) { this.shown = this.shown === this.exact ? this.readable : this.exact; this.changed(); }
    else this.close(key);
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape") || data === "q") { this.close(); return; }
    const target = this.shown;
    if (data === RAW && this.exact) this.press(RAW);
    else if (matchesKey(data, "up")) this.scroll(target.top - 1);
    else if (matchesKey(data, "down")) this.scroll(target.top + 1);
    else if (matchesKey(data, "pageUp")) this.scroll(target.top - target.page);
    else if (matchesKey(data, "pageDown")) this.scroll(target.top + target.page);
    else if (matchesKey(data, "home")) this.scroll(0);
    else if (matchesKey(data, "end")) this.scroll(target.max);
    else if (this.actions.some(action => action.key === data)) this.press(data);
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type === "wheel") { this.scroll(this.shown.top + (event.wheelDelta ?? 0)); return { handled: true }; }
    if (event.type !== "click" || event.button !== "left" || event.y !== this.height - 1) return undefined;
    const cap = this.caps.find(item => event.x >= item.from && event.x < item.to);
    if (!cap) return undefined;
    this.press(cap.key);
    return { handled: true };
  }
  invalidate(): void { this.readable.builtFor = -1; if (this.exact) this.exact.builtFor = -1; }
}

/**
 * A framed read-only overlay: title in the border, `a–b / n` position, wheel, arrows, PgUp/PgDn, Home/End; `d`
 * switches to the exact records when given. Resolves the key of the action that closed it, or undefined for q/Esc
 * and for `signal`, which closes a pager whose subject was settled elsewhere.
 */
export async function openPager(ui: ExtensionUIContext, input: PagerInput, signal?: AbortSignal): Promise<string | undefined> {
  if (signal?.aborted) return undefined;
  let close: (() => void) | undefined;
  const abort = () => close?.();
  signal?.addEventListener("abort", abort, { once: true });
  try {
    return await ui.custom<string | undefined>((tui, _theme, _keys, done) => {
      close = () => done(undefined);
      return new PagerView(input, () => tui.terminal.rows, () => tui.requestRender(), key => done(key));
    }, { overlay: true, overlayOptions: PLACE });
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}
