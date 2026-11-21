// One agent's conversation in the reading area, as the mock's `childHTML`: a header line (back link, name,
// profile and model, state and time, cost and context, and the activity sparkline while it runs), then the
// prompt in a bubble, the retention note when earlier messages were dropped, the replies and one line per tool
// call, then the agent's notes. A build child's prompt is its brief from the parent with what it owns inside the
// bubble; the owner's steers and the messages it exchanged with other agents sit among the replies at the time they were sent, then its state (waiting, asking,
// cancelled). A running agent's page follows its end until the owner scrolls up; the reading position is kept per agent key.
import { ScrollView, truncateToWidth, visibleWidth, wrapTextWithAnsi, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { duration, isLive, money, sparkline, timeLabel, toneColor, type AgentRow } from "./agents-model.ts";
import { spinner } from "./frame.ts";
import { bubble, mailSteers, type HelperChat, type Steer } from "./helper-chat.ts";
import { PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";
import { shiftRows } from "./selection.ts";

export interface AgentConversationHost {
  row(): AgentRow | undefined; chat: HelperChat; color(hue: number): string; now(): number;
  click(event: TuiMouseEvent): boolean; focus(): void; back(): void;
  /** Docks a build child's open question; the `▲` line under its replies is the click target. */
  answer?(key: string): void;
}

const BACK = "‹ Agents", BOLD = "\x1b[1m", UNBOLD = "\x1b[22m";

/**
 * The header for `row`, wrapped to `width` with one cell between its parts: `‹ Agents` always opens line 0 at
 * column 1, and `back` is its hit range. Parts that do not fit move to the next line; a part wider than a line is clipped.
 */
export function conversationHeader(row: AgentRow, width: number, now: number, color: (hue: number) => string):
  { readonly lines: string[]; readonly back: { readonly from: number; readonly to: number } } {
  const running = row.state === "running";
  const status = running && row.startedAt !== undefined ? { ...row.status, time: duration((row.endedAt ?? now) - row.startedAt) } : row.status;
  const usage = [row.usage?.cost === undefined ? "" : money(row.usage.cost),
    row.usage?.contextPercent === undefined ? "" : `ctx ${Math.round(row.usage.contextPercent)}%`].filter(Boolean).join(" · ");
  const pulse = running && row.pulse?.length ? sparkline(row.pulse, 12) : "";
  const segments = [
    PANEL_BLUE + BACK,
    BOLD + color(row.hue) + row.name + UNBOLD,
    PANEL_MUTED + row.profile + " · " + row.model + (row.pendingModel ? " → " + row.pendingModel : ""),
    toneColor(status.tone) + (running ? spinner(now) : status.glyph) + " " + timeLabel(status),
    ...(usage ? [PANEL_MUTED + usage] : []),
    ...(pulse ? [color(row.hue) + pulse] : []),
  ];
  const lines: string[] = [];
  let line = " ", used = 1;
  for (const segment of segments) {
    const size = visibleWidth(segment);
    if (used > 1 && used + 1 + size > width) { lines.push(line + PANEL_TEXT); line = " "; used = 1; }
    const piece = truncateToWidth(segment, Math.max(1, width - used - (used > 1 ? 1 : 0)), "…");
    line += (used > 1 ? " " : "") + piece; used += (used > 1 ? 1 : 0) + visibleWidth(piece);
  }
  lines.push(line + PANEL_TEXT);
  return { lines: lines.map(text => truncateToWidth(text, width, "")), back: { from: 1, to: 1 + Math.min(visibleWidth(BACK), Math.max(0, width - 1)) } };
}

/** The scrollable one-agent page; the workbench mounts `scroll` in place of the reading area. */
export class AgentConversation {
  readonly scroll: ScrollView;
  /** Per agent key: where the owner was reading, and whether the page was following the agent's end. */
  private readonly positions = new Map<string, { readonly top: number; readonly following: boolean }>();
  /** Set by `restore`: the next render places the page, from the saved position (if any) and whether the agent is live. */
  private pending: { readonly saved?: { readonly top: number; readonly following: boolean } } | undefined = {};
  private back = { from: 1, to: 1 + visibleWidth(BACK) };
  private chatStart = 0; private chatRows = 0;
  /** Rows of the clickable `▲` question line, and the agent it answers. */
  private ask: { readonly from: number; readonly to: number; readonly key: string } | undefined;
  constructor(private readonly host: AgentConversationHost) {
    this.scroll = new ScrollView({
      render: width => this.render(width),
      handleMouse: event => this.mouse(event),
      selectionRows: (width, y) => y >= this.chatStart && y < this.chatStart + this.chatRows
        ? shiftRows(host.chat.selectionRows(width, y - this.chatStart), this.chatStart, 0) : undefined,
      invalidate: () => host.chat.invalidate(),
    }, { primary: true, follow: "end", scrollbar: "auto" });
  }
  /** Keeps the current reading position for the shown agent. */
  remember(): void {
    const key = this.host.row()?.key;
    if (key !== undefined) this.positions.set(key, { top: this.scroll.scrollTop, following: this.scroll.isFollowingEnd });
  }
  /**
   * The next render starts where `key` was last read, or at the top. A live agent's page follows its end instead,
   * unless the owner had scrolled up from the end there.
   */
  restore(key: string): void {
    const saved = this.positions.get(key);
    this.pending = saved === undefined ? {} : { saved };
  }
  /** Whether the page keeps the newest row in view as the agent adds to it. */
  get following(): boolean { return this.scroll.isFollowingEnd; }
  expandTools(expanded: boolean): void { this.host.chat.expandTools(expanded); }

  private render(width: number): string[] {
    const row = this.host.row(), lines: string[] = [], now = this.host.now();
    const say = (text: string, color: string) => wrapTextWithAnsi(text, Math.max(1, width - 1)).map(line => " " + color + line + RESET);
    this.ask = undefined;
    if (!row) {
      this.back = { from: 1, to: 1 + visibleWidth(BACK) };
      lines.push(" " + PANEL_BLUE + BACK + RESET, " " + PANEL_WARNING + "This agent is no longer retained." + RESET);
      this.chatStart = lines.length; this.chatRows = 0;
    } else {
      const child = row.source === "child";
      const header = conversationHeader(row, width, now, hue => this.host.color(hue));
      this.back = header.back;
      lines.push(...header.lines.map(line => line + RESET), "");
      this.chatStart = lines.length;
      const prompt = child
        ? { label: "brief from parent", notes: [`Owns: ${row.owns?.length ? row.owns.join(", ") : "nothing (read-only)"} · no board access · no nested agents`] }
        : { label: "prompt from parent" };
      // The owner's steers and the messages it exchanged with other agents sit among the replies by the time they were sent.
      const steers: readonly Steer[] = [...(row.steers ?? []).map((text, index) => {
        const at = row.steeredAt?.[index];
        return at === undefined ? { text } : { text, at };
      }), ...mailSteers(row.mail ?? [], (id, hue) => hue === undefined ? id : BOLD + this.host.color(hue) + id + UNBOLD)];
      const chat = row.conversation ? this.host.chat.renderHelper(row.conversation, width,
        { bubble: prompt, now, ...(child || steers.length ? { steers: { label: "you → " + BOLD + this.host.color(row.hue) + row.name + UNBOLD, items: steers } } : {}) })
        : [" " + PANEL_MUTED + "Conversation unavailable for this agent." + RESET];
      this.chatRows = row.conversation ? chat.length : 0;
      lines.push(...chat);
      if (child) {
        if (row.state === "queued") lines.push("", ...say(`Not started: ${row.activity}.`, PANEL_MUTED));
        if (row.state === "needs" && row.question !== undefined) {
          lines.push("");
          const from = lines.length;
          lines.push(...say("▲ " + row.question + (row.capabilities.answer ? PANEL_MUTED + " · answer in the dialog" : ""), PANEL_WARNING));
          if (row.capabilities.answer) this.ask = { from, to: lines.length, key: row.key };
        }
        if (row.state === "cancelled") lines.push("", ...say("✗ Cancelled. Its edits so far stay in the worktree.", PANEL_ERROR));
      }
      for (const note of row.notes) {
        // A child's live wait is already its `Not started` line; its failure reads as the conversation's last word.
        // The retention note already stands under the prompt.
        if ((child && note.tone === "muted") || (note.retention && row.conversation)) continue;
        const mark = child && row.state === "failed" && note.tone === "error" ? "✗ " : "";
        lines.push("", ...say(mark + note.text, toneColor(note.tone)));
      }
    }
    if (this.pending !== undefined) {
      const saved = this.pending.saved, live = row !== undefined && isLive(row.state);
      this.scroll.updateLayout(lines.length, this.scroll.viewportHeight, () => {});
      if (live && (saved === undefined || saved.following)) this.scroll.scrollToEnd();
      else this.scroll.scrollTo(saved?.top ?? 0, { disableFollow: true });
      this.pending = undefined;
    }
    return lines;
  }
  private mouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.host.click(event)) return undefined;
    if (event.y === 0 && event.x >= this.back.from && event.x < this.back.to) { this.host.back(); return { handled: true }; }
    this.host.focus();
    const ask = this.ask;
    if (ask && event.y >= ask.from && event.y < ask.to && this.host.answer) { this.host.answer(ask.key); return { handled: true }; }
    if (event.y >= this.chatStart && event.y < this.chatStart + this.chatRows) {
      return this.host.chat.handleMouse({ ...event, y: event.y - this.chatStart }) ?? { handled: true };
    }
    return { handled: true };
  }
}
