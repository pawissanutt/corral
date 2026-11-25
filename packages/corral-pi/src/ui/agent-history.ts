import type { TranscriptPage, TranscriptRequest } from "../host/transcript-preview.ts";

export type TranscriptRead = (id: string, request: TranscriptRequest) => Promise<TranscriptPage>;
export type Direction = "older" | "newer";
/**
 * Pages kept resident: about 1.5 MB of recorded text at the host's 24 KiB a page, which holds nearly every recording
 * whole. Past it the far end is dropped and read again when the reader comes back.
 */
export const RESIDENT_PAGES = 64;

/** One snapshot's resident pages, in chronological order. */
export class AgentHistory {
  pages: readonly TranscriptPage[] = [];
  busy = false;
  /** Characters `extend` has joined to the newest page since the pages were read. */
  grown = 0;
  /** A newest page that `extend` had added to was given up: what it held past the snapshot's end is read again. */
  owed = false;
  get older(): string | undefined { return this.pages[0]?.older; }
  get newer(): string | undefined { return this.pages.at(-1)?.newer; }
  get stamp(): string | undefined { return this.pages[0]?.stamp; }
  /** The byte offset of the oldest resident record. */
  get first(): number | undefined {
    for (const page of this.pages) for (const row of page.rows) if (row.record !== undefined) return row.record;
    return undefined;
  }

  /** Reads what was recorded after the resident pages' end; none while that end is not resident, or from a host that offers none. */
  get grow(): string | undefined {
    const last = this.pages.at(-1);
    return last?.newer === undefined ? last?.grow : undefined;
  }

  /**
   * What the agent recorded since joins the newest page, so the pages above stay the ones already read and drawn.
   * False when `page` is of another snapshot or the recording's end is not resident.
   */
  extend(page: TranscriptPage): boolean {
    const last = this.pages.at(-1);
    if (!last || last.newer !== undefined || page.stamp !== this.stamp) return false;
    const next: TranscriptPage = { ...last, rows: page.rows.length ? [...last.rows, ...page.rows] : last.rows };
    if (page.grow === undefined) delete next.grow; else next.grow = page.grow;
    this.pages = [...this.pages.slice(0, -1), next];
    // A row without text (a stop, an empty result) still counts, so rows alone reach the limit too.
    for (const row of page.rows) this.grown += row.text.length + 1;
    this.owed = false;
    return true;
  }

  /** Begins at the recording's newest page. */
  open(latest: TranscriptPage): void { this.pages = [latest]; }

  cursor(direction: Direction): TranscriptRequest | undefined {
    const cursor = this.busy ? undefined : this[direction];
    return cursor ? { mode: "conversation", cursor } : undefined;
  }

  /** False when `page` is of another snapshot of the recording: it does not join the resident pages. */
  accept(direction: Direction, page: TranscriptPage): boolean {
    if (page.stamp !== this.stamp) return false;
    this.pages = direction === "older" ? [page, ...this.pages] : [...this.pages, page];
    return true;
  }

  /** Gives up the pages past the resident limit at the end away from `direction`; false when there were none. */
  trim(direction: Direction): boolean {
    if (this.pages.length <= RESIDENT_PAGES) return false;
    // The page read back in its place is the snapshot's own, without what was recorded after it.
    if (direction === "older" && this.grown > 0) { this.owed = true; this.grown = 0; }
    this.pages = direction === "older" ? this.pages.slice(0, RESIDENT_PAGES) : this.pages.slice(-RESIDENT_PAGES);
    return true;
  }
}

/** Keep paragraph breaks without interpreting recorded terminal control codes. */
export function recordedText(text: string): string {
  return text.replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");
}
