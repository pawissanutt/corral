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
  get older(): string | undefined { return this.pages[0]?.older; }
  get newer(): string | undefined { return this.pages.at(-1)?.newer; }
  get stamp(): string | undefined { return this.pages[0]?.stamp; }
  /** The byte offset of the oldest resident record. */
  get first(): number | undefined {
    for (const page of this.pages) for (const row of page.rows) if (row.record !== undefined) return row.record;
    return undefined;
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
    this.pages = direction === "older" ? [page, ...this.pages].slice(0, RESIDENT_PAGES) : [...this.pages, page].slice(-RESIDENT_PAGES);
    return true;
  }
}

/** Keep paragraph breaks without interpreting recorded terminal control codes. */
export function recordedText(text: string): string {
  return text.replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");
}
