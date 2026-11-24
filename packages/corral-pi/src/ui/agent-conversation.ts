import type { TranscriptPage, TranscriptRequest } from "../host/transcript-preview.ts";
import { AgentHistory, RESIDENT_PAGES, type Direction, type TranscriptRead } from "./agent-history.ts";
// One agent's conversation in the reading area, as the mock's `childHTML`: a header line (back link, name,
// profile and model, state and time, cost and context, and the activity sparkline while it runs), then the
// prompt in a bubble, the replies and one line per tool call, then the agent's notes. A build child's prompt is its
// brief from the parent with what it owns inside the bubble; the owner's steers and the messages it exchanged with
// other agents sit among the replies at the time they were sent, then its state (waiting, asking, cancelled).
//
// An agent with a saved session is read from its recording, drawn by the same HelperChat as its live projection, so
// the two look alike. The recording is read a page at a time as the reader scrolls: the page beyond each edge of the
// view is read before the reader reaches it, and a recording that changed is read again in place. Nothing is clicked.
// A running agent's page follows its end until the owner scrolls up; the reading position is kept per agent key.
import { ScrollView, truncateToWidth, visibleWidth, wrapTextWithAnsi, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { duration, isLive, money, sparkline, timeLabel, toneColor, type AgentRow } from "./agents-model.ts";
import { spinner } from "./frame.ts";
import { helperText, mailSteers, type ChatSource, type HelperChat, type MessageSpan, type Steer } from "./helper-chat.ts";
import { PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, RESET, plainLabel } from "./panels-view.ts";
import { assembleRecording, peerMessage, recordedLive, withLiveTail, type RecordedChat } from "./recorded-chat.ts";
import { shiftRows } from "./selection.ts";

export interface AgentConversationHost {
  row(): AgentRow | undefined; chat: HelperChat; color(hue: number): string; now(): number;
  click(event: TuiMouseEvent): boolean; focus(): void; back(): void;
  /** Docks a build child's open question; the `▲` line under its replies is the click target. */
  answer?(key: string): void;
  read?: TranscriptRead;
  lifetime?(): () => boolean;
  changed?(): void;
  canInteract?(): boolean;
  /** Recorded rows were read around the reader: the count of rows that arrived below them starts again. */
  rewrapped?(): void;
}

/** Where the view's first row is in the message it shows: from that message's first row, and to its last. */
/**
 * `side` is the end of the message that stays put while more of it is read: a recorded message is read back from
 * its last row, so its first rows may arrive later; a message still being written grows at its last row.
 */
type Anchor = { readonly id: string; readonly offset: number; readonly tail: number; readonly side: "from" | "to"; readonly record?: number };
type Position = { readonly top: number; readonly following: boolean; readonly anchor?: Anchor };
/** The reader's row in the frame before the resident pages changed: `edge` is the side that changed, `below` the rows from the view's first row to the end. */
type Hold = { readonly edge: Direction; readonly anchor?: Anchor; readonly below?: number };
/** The resident pages changed at `edge` and the next frame keeps the reader's row. `exact`: the rows on the other side are as they were. */
type Join = { readonly edge: Direction; readonly exact: boolean };

/** Why a recording is read again from its end; see `reread`. */
type Reread = "open" | "follow" | "end" | "place";
/** A recording read again and not shown yet: `from` is the resident pages it was read to replace. */
interface Arrival {
  readonly from: AgentHistory; readonly history: AgentHistory; readonly known: ReadonlySet<string>;
  readonly mode: Reread; readonly settling: boolean; readonly saved?: Position;
}

/** The most reads of a recording's latest page in search of one the agent ended no message during. */
const LATEST_READS = 3;
/** Pages read again, off screen, each time a running agent's recording grows under a reader at its end. */
const FOLLOW_PAGES = 8;
/** The most pages one re-read walks back for the reader's row. */
const REREAD_PAGES = 4 * RESIDENT_PAGES;
/** Rows kept read past each edge of the view, at least; two views of rows when that is more. */
const AHEAD_ROWS = 40;

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
  /** Per agent key: where the owner was reading, and whether the page was following the agent's end. An LRU. */
  private readonly positions = new Map<string, Position>();
  /** The place the next frame takes; `null` is the agent's own: a running agent's end, a finished one's first row. */
  private pending: Position | null | undefined = null;
  /** The place kept for a recording that is not read yet. */
  private reopen: Position | undefined;
  private joining: Join | undefined;
  /** Read by `reread`, for the next frame to show. */
  private arrived: Arrival | undefined;
  /** A page joined on this side past the resident limit: the far end is given up one frame later. */
  private trimming: Direction | undefined;
  /**
   * The owner asked for the recording's end and no frame has shown it yet. Whatever is being read, waits for a frame
   * or is joining gives way to it; scrolling up withdraws it.
   */
  private ending = false;
  private jump: "top" | "end" | undefined;
  private history = new AgentHistory();
  private recorded: { readonly pages: readonly TranscriptPage[]; readonly start: boolean; readonly key: string; readonly chat: RecordedChat } | undefined;
  private spans: MessageSpan[] = [];
  private height = 0;
  /** The last read failed. The next tick or scroll reads again, never a frame. */
  private failed = false;
  /** The owner has scrolled since the agent opened: a first read that arrives late leaves them where they are. */
  private moved = false;
  /** The side the owner last scrolled to: only that side is read ahead, so two near edges never trade pages. */
  private toward: Direction = "older";
  /** That scroll pushed against the edge of what is resident: the next page is read even at the cost of the far end. */
  private pushed = false;
  /** This view is placing the reader, not the owner. */
  private placing = false;
  /** The live messages the resident recording holds: `recordedLive` when it was read. */
  private known: ReadonlySet<string> = new Set();
  private wasLive = false;
  /** The agent settled while the reader was above its end: the recording holds more than the resident pages. */
  private latestAvailable = false;
  private scheduled = false;
  private back = { from: 1, to: 1 + visibleWidth(BACK) };
  private chatStart = 0; private chatRows = 0;
  /** Rows of the clickable `▲` question line, and the agent it answers. */
  private ask: { readonly from: number; readonly to: number; readonly key: string } | undefined;
  constructor(private readonly host: AgentConversationHost) {
    const laid = () => this.ahead();
    const scrolled = (by: number, edge: boolean) => {
      if (this.placing || by === 0) return;
      this.failed = false; this.moved = true;
      this.toward = by < 0 ? "older" : "newer"; this.pushed = edge;
      if (by < 0) this.ending = false;
      // A push against an edge moves nothing, so no frame follows it.
      if (edge) this.ahead();
    };
    // Pi's jump control and End ask the view for its end: the recording's, not the resident pages'.
    const ended = () => { if (!this.placing) this.jumpToRecordedEnd(); };
    this.scroll = new class extends ScrollView {
      override scrollBy(lines: number): number { const left = super.scrollBy(lines); scrolled(lines, left !== 0); return left; }
      override scrollTo(top: number, options?: Parameters<ScrollView["scrollTo"]>[1]): void {
        const from = this.scrollTop;
        super.scrollTo(top, options); scrolled(this.scrollTop - from, false);
      }
      override scrollToStart(): void { super.scrollToStart(); scrolled(-1, false); }
      override scrollToEnd(): void { super.scrollToEnd(); scrolled(1, false); ended(); }
      override updateLayout(contentHeight: number, viewportHeight: number, requestRender: () => void): void {
        super.updateLayout(contentHeight, viewportHeight, requestRender);
        laid();
      }
    }({
      render: width => this.render(width),
      handleMouse: event => this.mouse(event),
      selectionRows: (width, y) => y >= this.chatStart && y < this.chatStart + this.chatRows
        ? shiftRows(host.chat.selectionRows(width, y - this.chatStart), this.chatStart, 0) : undefined,
      invalidate: () => host.chat.invalidate(),
    }, { primary: true, follow: "end", scrollbar: "auto" });
  }
  private position(): Position {
    const top = this.scroll.scrollTop, following = this.atEnd();
    const span = this.spans.find(item => item.to > top);
    if (!span) return { top, following };
    const recorded = this.history.pages.length ? this.recorded?.chat : undefined, record = recorded?.records.get(span.id);
    // The newest resident message of a recording whose newer pages are not resident may be cut at its last row.
    const side = record !== undefined && !(this.history.newer && recorded?.messages.at(-1)?.id === span.id) ? "to" : "from";
    return { top, following, anchor: { id: span.id, offset: top - span.from, tail: span.to - top, side, ...(record === undefined ? {} : { record }) } };
  }
  /** The view's first row moved by this view, which is not the owner scrolling. */
  private place(top: number | "end"): void {
    this.placing = true;
    try { if (top === "end") this.scroll.scrollToEnd(); else this.scroll.scrollTo(top, { disableFollow: true }); }
    finally { this.placing = false; }
  }
  /** The reader is at the recording's end. The bottom of the resident pages is not that end while newer pages are not resident. */
  private atEnd(): boolean { return this.following && !this.history.newer; }
  /** The reader's row in the last frame, at the view's first row now. `exact`: the rows below it stay as they are. */
  private grip(edge: Direction, exact: boolean): Hold {
    const { anchor, top } = this.position();
    return { edge, ...(anchor ? { anchor } : {}), ...(exact ? { below: this.height - top } : {}) };
  }
  remember(): void {
    const key = this.host.row()?.key;
    if (key === undefined) return;
    this.positions.delete(key);
    // The end they asked for and have not been shown yet is the place kept.
    this.positions.set(key, this.ending ? { top: 0, following: true } : this.reopen ?? this.pending ?? this.position());
    if (this.positions.size > 8) this.positions.delete(this.positions.keys().next().value!);
  }
  restore(key: string): void {
    this.history = new AgentHistory(); this.recorded = undefined;
    this.spans = []; this.joining = undefined; this.arrived = undefined; this.trimming = undefined; this.ending = false; this.jump = undefined;
    this.failed = false; this.moved = false; this.wasLive = false; this.latestAvailable = false;
    this.toward = "older"; this.pushed = false; this.known = new Set();
    this.reopen = this.positions.get(key);
    this.pending = this.reopen ?? null;
  }
  get following(): boolean { return this.scroll.isFollowingEnd; }
  expandTools(expanded: boolean): void { this.host.chat.expandTools(expanded); }

  /** Called on entry and on workbench ticks, never by render or snapshot publication. */
  refresh(): void {
    const row = this.host.row(), history = this.history;
    // What was read last is shown by the next frame before anything is read again.
    if (!this.host.read || !row?.capabilities.transcript || history.busy || this.arrived) return;
    this.failed = false;
    const live = isLive(row.state);
    const following = (this.pending?.following ?? this.following) && !history.newer;
    if (this.owed()) void this.reread("end");
    else if (!history.pages.length) void this.reread("open");
    else if (following && (live || this.wasLive || this.latestAvailable)) void this.reread("follow", this.wasLive && !live);
    else if (this.wasLive && !live) this.latestAvailable = true;
    this.wasLive = live;
  }

  /**
   * End: the owner asks for the recording's end. True when resident pages do not reach that end, which is read
   * first; false when the scroll view's own end is that end, or all there is to show for now.
   */
  jumpToRecordedEnd(): boolean {
    if (this.host.canInteract?.() === false || !this.session()) return false;
    this.ending = true;
    // They are headed for the end now, however they last scrolled: the window keeps its newest pages.
    this.toward = "newer"; this.pushed = false;
    // The place kept for the agent, if no frame has taken it yet, gives way too.
    if (this.pending !== undefined) this.pending = { top: 0, following: true };
    // The next frame goes there, or shows what is being read first.
    if (!this.owed()) { this.host.changed?.(); return false; }
    // A read in flight ends first; this one follows it.
    void this.reread("end");
    return this.history.pages.length > 0;
  }
  /** The end the owner asked for is not among the resident pages, or these are of a recording that has more by now. */
  private owed(): boolean {
    return this.ending && (!this.history.pages.length || this.history.newer !== undefined || this.latestAvailable);
  }

  /** Reads the page beyond the resident ones on that side; false when there is none or one is being read. */
  navigate(direction: Direction): boolean {
    const request = this.history.cursor(direction);
    // A page that joined is placed, and the far end given up, before the next is read.
    if (!request || this.joining || this.trimming || this.host.canInteract?.() === false || !this.session()) return false;
    void this.page(direction, request);
    return true;
  }

  /** After each layout: the page beyond an edge is read before the reader reaches that edge. */
  private ahead(): void {
    if (this.scheduled || this.failed || this.history.busy || this.joining || this.trimming || !(this.history.older || this.history.newer)) return;
    this.scheduled = true;
    queueMicrotask(() => {
      this.scheduled = false;
      const history = this.history, top = this.scroll.scrollTop, view = this.scroll.viewportHeight, margin = Math.max(AHEAD_ROWS, 2 * view);
      if (this.failed || history.busy) return;
      const above = top < margin, below = this.height - top - view < margin;
      // A full window gives up a page at its far end for each one read. Unless the owner pushes on, that end must be
      // out of the reader's reach, or pages that draw few rows would be traded back and forth.
      const room = this.pushed || history.pages.length < RESIDENT_PAGES;
      if (this.toward === "older" ? above && (room || !below) : below && (room || !above)) this.navigate(this.toward);
    });
  }

  private session() {
    const row = this.host.row(), read = this.host.read, history = this.history;
    if (!row?.capabilities.transcript || !read) return undefined;
    const valid = this.host.lifetime?.() ?? (() => true);
    const current = () => valid() && this.history === history && this.host.row()?.key === row.key
      && this.host.row()?.capabilities.transcript === true;
    return { row, read, history, current };
  }

  private async page(direction: Direction, request: TranscriptRequest): Promise<void> {
    const session = this.session();
    if (!session) return;
    const { row, read, history, current } = session;
    let joined = false;
    history.busy = true;
    try {
      const page = await read(row.id, request);
      if (!current()) return;
      joined = history.accept(direction, page);
      // The next frame keeps the reader's row against the side that grew. The far end is given up in the frame
      // after, so no frame changes both ends of a message the reader may be in.
      if (joined) { this.joining = { edge: direction, exact: true }; this.trimming = direction; this.failed = false; }
    } catch { /* The recording changed or could not be read: it is read again below. */ }
    finally { history.busy = false; }
    if (!current()) return;
    if (this.owed()) await this.reread("end");
    else if (joined) { this.host.rewrapped?.(); this.host.changed?.(); }
    else await this.reread("place");
  }

  /**
   * Reads the recording again from its end, off screen, and has the next frame show it in one step (`arrive`), so
   * no frame draws half of it.
   * `open` is the first read, back to the place kept for the agent. `follow` keeps a reader at a running agent's end
   * on its newest messages. `end` is the End key. `place` follows a recording that changed under the resident pages,
   * whose cursors no longer read it, and keeps the reader's row: a record keeps its byte offset while a recording
   * only grows, so the read walks back until that record is resident again.
   */
  private async reread(mode: Reread, settling = false): Promise<void> {
    const session = this.session();
    if (!session || session.history.busy) return;
    const { row, read, history, current } = session;
    const saved = mode === "open" ? this.reopen : undefined;
    // It is read back as far as the resident pages went, so whichever row the reader is on by then is resident again.
    const keep = mode === "place" && !this.atEnd();
    const until = saved && !saved.following ? saved.anchor?.record : keep ? history.first : undefined;
    const pages = until === undefined ? Math.min(Math.max(1, history.pages.length), FOLLOW_PAGES) : REREAD_PAGES;
    let next: AgentHistory | undefined, known: ReadonlySet<string> = new Set();
    history.busy = true;
    try {
      // A message that ended while the file was read may or may not be in what came back. Only a read that none
      // ended during is shown: the live messages it holds are then exactly the ones that have ended.
      let latest: TranscriptPage | undefined;
      for (let attempt = 0; attempt < LATEST_READS && !latest; attempt++) {
        const before = recordedLive(this.host.row()?.conversation);
        const page = await read(row.id, { mode: "conversation" });
        if (!current()) return;
        known = recordedLive(this.host.row()?.conversation);
        if (known.size === before.size && [...known].every(id => before.has(id))) latest = page;
      }
      // The agent ended a message during every read: none is shown, and the next tick reads again.
      if (!latest) throw new Error("recording written throughout its read");
      if (mode === "follow" && latest.stamp === history.stamp && !history.newer) { this.latestAvailable = false; this.known = known; return; }
      const walk = new AgentHistory();
      walk.open(latest);
      // The reader's record is whole once the record before it is resident too.
      for (let count = 1; count < pages && walk.older && !(until !== undefined && (walk.first ?? Infinity) < until); count++) {
        const page = await read(row.id, { mode: "conversation", cursor: walk.older });
        if (!current()) return;
        // It changed again while it was read: the next tick or scroll reads it again.
        if (!walk.accept("older", page)) throw new Error("recording changed");
        walk.trim("older");
      }
      next = walk;
    } catch { /* `next` stays undefined. */ }
    finally { history.busy = false; }
    if (!current()) return;
    // The owner scrolled away from the end they asked for while it was read: nothing of it is shown or owed.
    if (mode === "end" && !this.ending) return;
    // A refresh that failed is owed: a settled agent's last messages are read on a later tick.
    if (!next) { this.failed = true; if (mode === "follow" || mode === "end") this.latestAvailable = true; this.host.changed?.(); return; }
    this.arrived = { from: history, history: next, known, mode, settling, ...(saved ? { saved } : {}) };
    this.host.changed?.();
  }

  /**
   * What `reread` read takes the resident pages' place as a frame is laid out, the frame that places the reader,
   * and only if what it was read for still holds then: the owner may have scrolled since it was asked for.
   */
  private arrive(): void {
    const arrived = this.arrived;
    this.arrived = undefined;
    if (!arrived || arrived.from !== this.history) return;
    const { mode } = arrived, wanted = this.ending;
    if (mode === "end" && !wanted) return;
    // An automatic refresh never moves a reader who left the end.
    if (mode === "follow" && !wanted && !(this.pending?.following ?? this.following)) { if (arrived.settling) this.latestAvailable = true; return; }
    // The end the owner asked for comes before the place kept for them and the row they were on.
    const saved = wanted ? undefined : arrived.saved;
    // A recording that changed under the reader is shown around their row, unless they are at its end by now.
    const held = mode === "place" && !wanted && !this.atEnd();
    const end = wanted || (!saved && !held && (mode !== "open" || !this.moved || this.following));
    // It was read back to a row, past the resident limit, so its newest pages were given up, and the reader is at
    // the end or asked for it: the end is read instead. The resident pages are of a recording that has more by now.
    if (end && arrived.history.newer) {
      this.ending = true; this.latestAvailable = true;
      queueMicrotask(() => { if (this.ending) void this.reread("end"); });
      return;
    }
    this.history = arrived.history; this.known = arrived.known; this.failed = false; this.latestAvailable = false;
    this.joining = undefined; this.trimming = undefined; this.reopen = undefined; this.ending = false;
    if (saved) this.pending = saved;
    else if (held) this.joining = { edge: "older", exact: false };
    else if (end) { this.pending = undefined; this.jump = "end"; this.toward = "older"; this.pushed = false; }
    this.host.rewrapped?.();
  }

  private recording(history: AgentHistory, row: AgentRow): RecordedChat {
    const start = !history.older, key = [row.key, row.name, row.hue].join("\n");
    if (this.recorded?.pages !== history.pages || this.recorded.start !== start || this.recorded.key !== key) {
      // Messages of the same snapshot that the new pages left alone stay the objects HelperChat has drawn.
      const previous = this.recorded?.key === key && this.recorded.pages[0]?.stamp === history.stamp ? this.recorded.chat : undefined;
      this.recorded = { pages: history.pages, start, key, chat: assembleRecording(history.pages, start, text => this.aside(row, text), previous) };
    }
    return this.recorded.chat;
  }
  /** A message the agent received after its prompt: a peer's under `<from> → <agent>`, a build child's steer under `you → <agent>`. */
  private aside(row: AgentRow, text: string): { readonly label: string; readonly text: string } {
    const self = BOLD + this.host.color(row.hue) + row.name + UNBOLD, peer = peerMessage(text);
    if (!peer) return { label: row.source === "child" ? "you → " + self : "message", text };
    const from = plainLabel(helperText(peer.from));
    const hue = row.mail?.find(message => message.from === peer.from)?.fromHue ?? row.mail?.find(message => message.to === peer.from)?.toHue;
    return { label: (hue === undefined ? from : BOLD + this.host.color(hue) + from + UNBOLD) + PANEL_MUTED + " → " + self, text: peer.text };
  }

  private render(width: number): string[] {
    const row = this.host.row(), lines: string[] = [], now = this.host.now();
    const say = (text: string, color: string) => wrapTextWithAnsi(text, Math.max(1, width - 1)).map(line => " " + color + line + RESET);
    this.arrive();
    // The end the owner asked for is resident: this frame goes there, whatever was joining.
    if (this.ending && !this.owed()) { this.ending = false; this.joining = undefined; this.pending = undefined; this.jump = "end"; }
    // The resident pages changed since the last frame: the reader's row is taken from that frame and where they
    // have scrolled to since, before this frame is laid out. A reader at the recording's end stays there whatever
    // joins above them.
    const join = this.joining;
    const hold = join && !(join.edge === "older" && this.atEnd()) ? this.grip(join.edge, join.exact) : undefined;
    this.joining = undefined;
    this.ask = undefined; this.spans = [];
    if (!row) {
      this.back = { from: 1, to: 1 + visibleWidth(BACK) };
      lines.push(" " + PANEL_BLUE + BACK + RESET, " " + PANEL_WARNING + "This agent is no longer retained." + RESET);
      this.chatStart = lines.length; this.chatRows = 0;
    } else {
      const child = row.source === "child";
      const header = conversationHeader(row, width, now, hue => this.host.color(hue));
      this.back = header.back;
      lines.push(...header.lines.map(line => line + RESET), "");
      const prompt = child
        ? { label: "brief from parent", notes: [`Owns: ${row.owns?.length ? row.owns.join(", ") : "nothing (read-only)"} · no board access · no nested agents`] }
        : { label: "prompt from parent" };
      const name = (id: string, hue: number | undefined) => hue === undefined ? id : BOLD + this.host.color(hue) + id + UNBOLD;
      const history = row.capabilities.transcript && this.host.read ? this.history : undefined;
      const recorded = history?.pages.length ? this.recording(history, row) : undefined;
      let chat: string[];
      if (history && recorded) {
        const end = !history.newer, running = isLive(row.state);
        const messages = end && (running || this.latestAvailable) ? withLiveTail(recorded, row.conversation, this.known) : recorded.messages;
        // A message that never reached the agent is in no recording: it follows the replies, as in the live view.
        const missed = end ? mailSteers((row.mail ?? []).filter(message => message.from !== row.id
          && (message.delivery === "undeliverable" || message.delivery === "answered_from_report")), name) : [];
        const opening = recorded.prompt ?? (history.older ? undefined : row.conversation?.prompt);
        const source: ChatSource = { request: "recorded:" + row.key, phase: row.conversation?.phase ?? (running ? "running" : "completed"), messages,
          ...(opening === undefined ? {} : { prompt: opening }) };
        if (this.failed && history.older) lines.push(...say("Earlier messages could not be read · trying again", PANEL_MUTED), "");
        this.chatStart = lines.length;
        chat = this.host.chat.renderHelper(source, width, { bubble: prompt, now, ...(missed.length ? { steers: { label: "", items: missed } } : {}) });
        this.chatRows = chat.length;
        lines.push(...chat);
        if (!chat.length) lines.push(...say(history.older ? "Reading earlier messages…" : "No recorded messages yet.", PANEL_MUTED));
        if (this.failed && history.newer) lines.push("", ...say("Newer messages could not be read · trying again", PANEL_MUTED));
      } else {
        // No saved session, or none read yet: what this session still holds of the agent, drawn the same way.
        // The owner's steers and the messages it exchanged with other agents sit among the replies by the time they were sent.
        const steers: readonly Steer[] = [...(row.steers ?? []).map((text, index) => {
          const at = row.steeredAt?.[index];
          return at === undefined ? { text } : { text, at };
        }), ...mailSteers(row.mail ?? [], name)];
        this.chatStart = lines.length;
        chat = row.conversation ? this.host.chat.renderHelper(row.conversation, width,
          { bubble: prompt, now, ...(child || steers.length ? { steers: { label: "you → " + BOLD + this.host.color(row.hue) + row.name + UNBOLD, items: steers } } : {}) })
          : [];
        this.chatRows = chat.length;
        lines.push(...chat);
        const waiting = !history ? "Conversation unavailable for this agent."
          : this.failed ? "The saved conversation could not be read · trying again" : "Reading the saved conversation…";
        if (!row.conversation || (history && this.failed)) lines.push(...(chat.length ? [""] : []), ...say(waiting, PANEL_MUTED));
      }
      this.spans = this.chatRows ? this.host.chat.spans().map(span => ({ id: span.id, from: span.from + this.chatStart, to: span.to + this.chatStart })) : [];
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
    this.height = lines.length;
    if (hold || this.jump) {
      this.scroll.updateLayout(lines.length, this.scroll.viewportHeight, () => {});
      const anchor = hold?.anchor, span = anchor && this.spans.find(item => item.id === anchor.id);
      // A change above leaves the reader's row where it is from the end of its message, whose first rows may have
      // changed; a change below, from its start.
      if (hold && anchor && span) this.place(hold.edge === "older" ? span.to - anchor.tail : span.from + anchor.offset);
      else if (hold?.edge === "older" && hold.below !== undefined) this.place(lines.length - hold.below);
      else if (hold ? hold.edge === "older" : this.jump === "end") this.place("end");
      else if (!hold) this.place(0);
      this.jump = undefined;
    }
    if (this.pending !== undefined) {
      const saved = this.pending, live = row !== undefined && isLive(row.state);
      this.scroll.updateLayout(lines.length, this.scroll.viewportHeight, () => {});
      const span = this.spans.find(item => item.id === saved?.anchor?.id);
      const anchor = saved?.anchor;
      if (saved?.following || (live && !saved)) this.place("end");
      else this.place(!span || !anchor ? saved?.top ?? 0 : anchor.side === "to" ? span.to - anchor.tail : Math.min(span.to - 1, span.from + anchor.offset));
      this.pending = undefined;
    }
    if (this.trimming) {
      // What is given up is on the other side of the reader from what joined. A reader who came down to the
      // recording's end keeps that end, and gives up the oldest pages.
      const at = this.atEnd(), edge = at && this.toward === "newer" ? "newer" : this.trimming;
      this.trimming = undefined;
      if (this.history.trim(edge)) {
        if (!at) this.joining = { edge: edge === "older" ? "newer" : "older", exact: true };
        this.host.rewrapped?.();
        queueMicrotask(() => this.host.changed?.());
      }
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
