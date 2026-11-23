import type { TranscriptPage, TranscriptRequest } from "../host/transcript-preview.ts";

export type TranscriptRead = (id: string, request: TranscriptRequest) => Promise<TranscriptPage>;
export type Direction = "older" | "newer" | "latest";

/** One snapshot's resident pages, in chronological order. */
export class AgentHistory {
  pages: readonly TranscriptPage[] = [];
  busy = false;
  retry = false;
  get older(): string | undefined { return this.pages[0]?.older; }
  get newer(): string | undefined { return this.pages.at(-1)?.newer; }

  /** Reuse resident cursors only after revalidating their recording snapshot. */
  resume(pages: readonly TranscriptPage[], latest: TranscriptPage): boolean {
    if (pages[0]?.stamp !== latest.stamp) return false;
    this.pages = pages;
    return true;
  }

  request(direction: Direction): TranscriptRequest | undefined {
    if (this.busy) return undefined;
    if (direction === "latest") return { mode: "conversation" };
    const cursor = this[direction];
    return !this.retry && cursor ? { mode: "conversation", cursor } : undefined;
  }

  accept(direction: Direction, page: TranscriptPage): boolean {
    if (direction !== "latest" && page.stamp !== this.pages[0]?.stamp) {
      this.retry = true;
      return false;
    }
    this.retry = false;
    if (direction === "latest") {
      if (page.stamp === this.pages.at(-1)?.stamp && !this.newer) return false;
      this.pages = [page];
    } else if (direction === "older") this.pages = [page, ...this.pages].slice(0, 8);
    else this.pages = [...this.pages, page].slice(-8);
    return true;
  }
}

/** Keep paragraph breaks without interpreting recorded terminal control codes. */
export function recordedText(text: string): string {
  return text.replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");
}
