// One agent's conversation in the reading area: a header line (back link, name, profile and model, state, and
// cost, context and pulse when known), then the prompt and replies through Pi's own message components, then
// the agent's notes. The reading position is kept per agent key.
import { ScrollView, truncateToWidth, visibleWidth, wrapTextWithAnsi, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { duration, money, statusLabel, toneColor, type AgentRow } from "./agents-model.ts";
import type { HelperChat } from "./helper-chat.ts";
import { PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";

export interface AgentConversationHost {
  row(): AgentRow | undefined; chat: HelperChat; color(hue: number): string; now(): number;
  click(event: TuiMouseEvent): boolean; focus(): void; back(): void;
}

const BACK = "‹ Agents", BARS = " ▁▂▃▄▅▆▇";

/**
 * The header for `row`, wrapped to `width`: `‹ Agents` always opens line 0 at column 1, and `back` is its
 * hit range. Segments that do not fit move to the next line; a segment wider than a line is clipped.
 */
export function conversationHeader(row: AgentRow, width: number, now: number, color: (hue: number) => string):
  { readonly lines: string[]; readonly back: { readonly from: number; readonly to: number } } {
  const running = row.state === "running" && row.startedAt !== undefined;
  const status = running ? { ...row.status, time: duration((row.endedAt ?? now) - (row.startedAt ?? now)) } : row.status;
  const usage = [row.usage?.cost === undefined ? "" : money(row.usage.cost),
    row.usage?.contextPercent === undefined ? "" : `ctx ${Math.round(row.usage.contextPercent)}%`].filter(Boolean).join(" · ");
  const live = row.state === "running" || row.state === "needs";
  const pulse = live && row.pulse?.length ? row.pulse.slice(-12).map(value => BARS[Math.max(0, Math.min(7, Math.round(value)))] ?? " ").join("") : "";
  const segments = [
    PANEL_BLUE + BACK,
    "\x1b[1m" + color(row.hue) + row.name + "\x1b[22m",
    PANEL_MUTED + row.profile + " · " + row.model + (row.pendingModel ? " → " + row.pendingModel : ""),
    toneColor(status.tone) + status.glyph + " " + statusLabel(status),
    ...(usage ? [PANEL_MUTED + usage] : []),
    ...(pulse ? [color(row.hue) + pulse] : []),
  ];
  const lines: string[] = [];
  let line = " ", used = 1;
  for (const segment of segments) {
    const size = visibleWidth(segment);
    if (used > 1 && used + 2 + size > width) { lines.push(line + PANEL_TEXT); line = " "; used = 1; }
    const piece = truncateToWidth(segment, Math.max(1, width - used - (used > 1 ? 2 : 0)), "…");
    line += (used > 1 ? "  " : "") + piece; used += (used > 1 ? 2 : 0) + visibleWidth(piece);
  }
  lines.push(line + PANEL_TEXT);
  return { lines: lines.map(text => truncateToWidth(text, width, "")), back: { from: 1, to: 1 + Math.min(visibleWidth(BACK), Math.max(0, width - 1)) } };
}

/** The scrollable one-agent page; the workbench mounts `scroll` in place of the reading area. */
export class AgentConversation {
  readonly scroll: ScrollView;
  private readonly positions = new Map<string, number>();
  private pending: number | undefined;
  private back = { from: 1, to: 1 + visibleWidth(BACK) };
  private chatStart = 0; private chatRows = 0;
  constructor(private readonly host: AgentConversationHost) {
    this.scroll = new ScrollView({
      render: width => this.render(width),
      handleMouse: event => this.mouse(event),
      invalidate: () => host.chat.invalidate(),
    }, { primary: true, follow: "none", scrollbar: "auto" });
  }
  /** Keeps the current reading position for the shown agent. */
  remember(): void {
    const key = this.host.row()?.key;
    if (key !== undefined) this.positions.set(key, this.scroll.scrollTop);
  }
  /** The next render starts where `key` was last read, or at the top. */
  restore(key: string): void { this.pending = this.positions.get(key) ?? 0; }
  expandTools(expanded: boolean): void { this.host.chat.expandTools(expanded); }

  private render(width: number): string[] {
    const row = this.host.row(), lines: string[] = [];
    if (!row) {
      this.back = { from: 1, to: 1 + visibleWidth(BACK) };
      lines.push(" " + PANEL_BLUE + BACK + RESET, " " + PANEL_WARNING + "This agent is no longer retained." + RESET);
      this.chatStart = lines.length; this.chatRows = 0;
    } else {
      const header = conversationHeader(row, width, this.host.now(), hue => this.host.color(hue));
      this.back = header.back;
      lines.push(...header.lines.map(line => line + RESET), "");
      this.chatStart = lines.length;
      const chat = row.conversation ? this.host.chat.renderHelper(row.conversation, width)
        : [" " + PANEL_MUTED + "Conversation unavailable for this agent." + RESET];
      this.chatRows = row.conversation ? chat.length : 0;
      lines.push(...chat);
      for (const note of row.notes) {
        lines.push("", ...wrapTextWithAnsi(note.text, Math.max(1, width - 1)).map(text => " " + toneColor(note.tone) + text + RESET));
      }
    }
    if (this.pending !== undefined) {
      this.scroll.updateLayout(lines.length, this.scroll.viewportHeight, () => {});
      this.scroll.scrollTo(this.pending, { disableFollow: true });
      this.pending = undefined;
    }
    return lines;
  }
  private mouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.host.click(event)) return undefined;
    if (event.y === 0 && event.x >= this.back.from && event.x < this.back.to) { this.host.back(); return { handled: true }; }
    this.host.focus();
    if (event.y >= this.chatStart && event.y < this.chatStart + this.chatRows) {
      return this.host.chat.handleMouse({ ...event, y: event.y - this.chatStart }) ?? { handled: true };
    }
    return { handled: true };
  }
}
