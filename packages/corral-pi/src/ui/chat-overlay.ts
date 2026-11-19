// Chat's own presentation around Pi's transcript: the empty state before the first transcript row, the new-rows
// count on Pi's jump-to-latest control while the reader is scrolled up, and the unread mark the Chat tab carries
// while another view is open. Nothing here reaches the model.
import { Container, Text, stripTerminalSequences, type ScrollView, type TUI } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { PANEL_BG, PANEL_MUTED, PANEL_SELECTED, PANEL_TEXT } from "./panels-view.ts";

const BOLD = "\x1b[1m", UNBOLD = "\x1b[22m";

/**
 * Watches a scroll view that follows its end (Pi's transcript, or one agent's conversation). Pi lays it out every
 * frame it shows; the watch wraps that layout call to learn the content height, so the count of rows that arrived
 * below a paused reader costs nothing. While another view is open nothing is laid out or rendered: the unread mark
 * compares the transcript's component counts instead.
 */
export class TranscriptWatch {
  private height = 0;
  /** Content rows when the reader last followed the end; rows past it arrived while they read above. */
  private seen = 0;
  private rewrap = false;
  private started = false;
  /** The transcript's revision once another view showed; undefined while Chat is open. */
  private away: number | undefined;
  /** Where a paused reader was when a dialog opened over the transcript; restored until it closes. */
  private pinned: number | undefined;
  /** `modal`: a dialog holds the keys. Laying out under it never moves a paused reader to the end. */
  constructor(private readonly transcript: ScrollView, private readonly modal?: () => boolean) {
    const layout = transcript.updateLayout.bind(transcript);
    transcript.updateLayout = (contentHeight, viewportHeight, requestRender) => {
      const held = this.modal?.() ?? false;
      if (held && this.pinned === undefined && !transcript.isFollowingEnd) this.pinned = transcript.scrollTop;
      layout(contentHeight, viewportHeight, requestRender);
      if (this.pinned !== undefined) {
        if (transcript.isFollowingEnd || transcript.scrollTop !== this.pinned) transcript.scrollTo(this.pinned, { disableFollow: true });
        if (!held) this.pinned = undefined;
      }
      this.height = contentHeight;
      if (this.transcript.isFollowingEnd || this.rewrap) { this.seen = contentHeight; this.rewrap = false; }
    };
  }
  /** True while the reader follows the end: nothing waits below. */
  get following(): boolean { return this.transcript.isFollowingEnd; }
  /** Rows that arrived below the reader since they left the end; 0 while following. */
  get newRows(): number {
    return this.transcript.isFollowingEnd ? 0 : Math.max(0, this.height - this.seen);
  }
  /** The transcript was laid out at another width: its rows rewrapped, so the count starts again from here. */
  rewrapped(): void { this.rewrap = true; }
  /** End, or a click on the control: back to the latest row, following again. */
  jump(): void { this.transcript.scrollToEnd(); }
  /** True until the transcript draws its first non-blank row; a restored session has one in its first frame. */
  empty(width: number): boolean {
    if (this.started) return false;
    this.started = this.transcript.render(Math.max(1, width)).some(line => stripTerminalSequences(line).trim() !== "");
    return !this.started;
  }
  /** Chat opened again: the mark clears and the next view measures from where Chat was left. */
  back(): void { this.away = undefined; }
  /** Asked each frame while another view is open: true once a message, tool row or other entry was added or removed. */
  unread(): boolean {
    const now = this.revision();
    this.away ??= now;
    return now !== this.away;
  }
  /**
   * Components two levels under the transcript (Pi's document holds its header, resources and chat containers,
   * and the chat container one component per entry): counted, never rendered, so hidden history costs nothing.
   */
  private revision(): number {
    let count = 0;
    for (const child of this.transcript.children) {
      count++;
      if (child instanceof Container) for (const part of child.children) count += 1 + (part instanceof Container ? part.children.length : 0);
    }
    return count;
  }
}

/**
 * Pi's own jump-to-latest control (centred on the transcript's last row while the reader is not following)
 * carries the count of rows that arrived since: `↓ 37 new rows · End`, else `↓ Jump to latest message · End`.
 * The control and its click stay Pi's; only its label is ours, set again whenever Pi's renderer is replaced.
 */
export function labelJumpControl(tui: TUI, label: () => string): void {
  const current: unknown = Reflect.get(tui, "scrollToEndIndicator");
  if (typeof current === "function" && OURS.has(current)) return;
  const draw = () => PANEL_SELECTED + PANEL_TEXT + " " + label() + " " + PANEL_BG + "\x1b[0m";
  OURS.add(draw);
  Reflect.set(tui, "scrollToEndIndicator", draw);
}
const OURS = new WeakSet<object>();
/** The jump control's words: rows that arrived since the reader left the end, and the key that jumps. */
export function jumpLabel(newRows: number, key: string | undefined): string {
  const what = newRows > 0 ? `↓ ${newRows} new ${newRows === 1 ? "row" : "rows"}` : "↓ Jump to latest message";
  return key ? `${what} · ${key}` : what;
}

export interface EmptyChat {
  readonly identity: HostIdentity;
  /** Key labels from Pi's bindings; an unbound key leaves its hint out. */
  readonly send?: string; readonly quit?: string;
}

/** The empty Chat's sentences for a role: the bold lead, then muted paragraphs (the mock's wording for build). */
function emptyCopy(identity: HostIdentity): { readonly lead: string; readonly rest: string; readonly more?: string } {
  switch (identity.role) {
    case "build":
      return { lead: "Build session.", rest: "Describe what to build and press Enter.",
        more: "Independent slices run in parallel in one worktree, each on the profile that fits it. Nothing is filed on the board until you choose an exit." };
    case "assistant":
      return { lead: `Assistant ${identity.assistant ?? ""}`.trimEnd() + ".", rest: "What you send goes to this assistant." };
    default:
      return { lead: "Waiting for corral's first instruction.", rest: "What you send goes to this agent." };
  }
}

/**
 * The empty Chat drawn where the transcript will be, one cell in like the other views: bold lead sentence, muted
 * rest, then a muted line with how to send, the keys list and how to quit (`/finish` ends a build).
 */
export function emptyChatLines(input: EmptyChat, width: number): string[] {
  const copy = emptyCopy(input.identity), inner = Math.max(1, width - 2);
  const wrap = (text: string) => new Text(text, 0, 0).render(inner).map(line => " " + line);
  const keys = [
    input.send && `${input.send} send`, "F1 keys",
    input.identity.role === "build" && "/finish ends the build",
    input.quit && `${input.quit} twice quits`,
  ].filter((item): item is string => typeof item === "string");
  return [
    "",
    ...wrap(BOLD + PANEL_TEXT + copy.lead + UNBOLD + PANEL_MUTED + " " + copy.rest),
    ...(copy.more ? ["", ...wrap(PANEL_MUTED + copy.more)] : []),
    "",
    ...wrap(PANEL_MUTED + keys.join("   ")),
  ];
}
