// The one read-only pager: job output, peek, host receipts, board task details, agent details, the transcript reader
// and the Inbox detail sheet. A rounded accent box with the title and the position `a–b / n` set into its top border;
// ↑↓, PgUp/PgDn, Home/End and the wheel scroll it, q or Esc closes it. Lines are clipped with `…`, never wrapped, so
// numbered output stays aligned; `wrap` is for prose (the transcript, the Inbox sheet) and counts wrapped rows.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import {
  matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi,
  type Component, type OverlayOptions, type TuiMouseEvent, type TuiMouseEventResult,
} from "@earendil-works/pi-tui";
import { onSurface, titledBox } from "./frame.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

export interface PagerInput {
  readonly title: string;
  readonly lines: readonly string[];
  /** The source line shown first; past the end opens at the end. */
  readonly start?: number;
  /** Wrap long lines to the frame instead of clipping them; for prose without line numbers. */
  readonly wrap?: boolean;
}

/** Share of the terminal's rows the frame may take; the overlay itself allows 85%. */
const SHARE = 0.8;
const PLACE: OverlayOptions = { width: "90%", maxHeight: "85%", anchor: "center" };
const RESET = "\x1b[0m";

/** `a–b / n` over 1-based rows; one row reads `a / n`, nothing `0 / 0`. */
export function pagerPosition(first: number, last: number, count: number): string {
  if (count === 0 || last < first) return "0 / 0";
  return first === last ? `${first + 1} / ${count}` : `${first + 1}–${last + 1} / ${count}`;
}

export class PagerView implements Component {
  private readonly source: readonly string[];
  /** Drawn rows and the source line each comes from; rebuilt when a wrapping pager changes width. */
  private rows: readonly string[] = [];
  private lineOf: readonly number[] = [];
  private builtFor = -1;
  /** The source line at the top, so a wrapping pager keeps its passage across a resize. */
  private anchor: number;
  private offset = 0;
  private max = 0;
  private page = 1;
  constructor(private readonly input: PagerInput, private readonly terminalRows: () => number,
    private readonly changed: () => void, private readonly done: () => void) {
    // Plain text on one row each: tabs become four blanks, any other control character a blank.
    this.source = input.lines.flatMap(line => line.split(/\r?\n/))
      .map(line => line.replaceAll("\t", "    ").replace(/[\u0000-\u001f\u007f-\u009f]/g, " "));
    this.anchor = Math.max(0, Math.trunc(input.start ?? 0));
  }
  private build(inner: number): void {
    const rows: string[] = [], lineOf: number[] = [];
    this.source.forEach((line, index) => {
      const parts = this.input.wrap && line ? wrapTextWithAnsi(line, inner) : [line];
      for (const part of parts.length ? parts : [""]) { rows.push(part); lineOf.push(index); }
    });
    this.rows = rows; this.lineOf = lineOf;
    const first = lineOf.findIndex(line => line >= this.anchor);
    this.offset = first < 0 ? rows.length : first;
  }
  render(width: number): string[] {
    if (width < 8) return [];
    const inner = width - 4;
    if (this.builtFor < 0 || (this.input.wrap && this.builtFor !== inner)) { this.build(inner); this.builtFor = inner; }
    const count = this.rows.length;
    const body = Math.max(1, Math.min(Math.max(1, Math.floor(this.terminalRows() * SHARE) - 2), count));
    this.page = Math.max(1, body - 1);
    this.max = Math.max(0, count - body);
    this.offset = Math.max(0, Math.min(this.max, this.offset));
    const shown = count ? this.rows.slice(this.offset, this.offset + body) : [PANEL_MUTED + "· no lines"];
    const last = Math.min(count, this.offset + body) - 1;
    const frame = titledBox("", shown, width, { tone: "accent" });
    frame[0] = this.top(pagerPosition(this.offset, last, count), width);
    return frame;
  }
  /** `╭─ title ──── a–b / n ─╮` in accent: the title bold and clipped first, the position muted. */
  private top(position: string, width: number): string {
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
  private scroll(to: number): void {
    const next = Math.max(0, Math.min(this.max, to));
    if (next === this.offset) return;
    this.offset = next; this.anchor = this.lineOf[next] ?? 0;
    this.changed();
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape") || data === "q") { this.done(); return; }
    if (matchesKey(data, "up")) this.scroll(this.offset - 1);
    else if (matchesKey(data, "down")) this.scroll(this.offset + 1);
    else if (matchesKey(data, "pageUp")) this.scroll(this.offset - this.page);
    else if (matchesKey(data, "pageDown")) this.scroll(this.offset + this.page);
    else if (matchesKey(data, "home")) this.scroll(0);
    else if (matchesKey(data, "end")) this.scroll(this.max);
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type !== "wheel") return undefined;
    this.scroll(this.offset + (event.wheelDelta ?? 0));
    return { handled: true };
  }
  invalidate(): void { this.builtFor = -1; }
}

/** A framed read-only overlay: title in the border, `a–b / n` position, wheel, arrows, PgUp/PgDn, Home/End, q or Esc closes. */
export async function openPager(ui: ExtensionUIContext, input: PagerInput): Promise<void> {
  await ui.custom<void>((tui, _theme, _keys, done) =>
    new PagerView(input, () => tui.terminal.rows, () => tui.requestRender(), () => done()),
  { overlay: true, overlayOptions: PLACE });
}
