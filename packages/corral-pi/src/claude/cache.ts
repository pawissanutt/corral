// A stable prefix is useful only if a previous request actually wrote it. Advance one marker
// within the 20-block lookup window instead of spending another of the SDK's four breakpoints.
import { createHash } from "node:crypto";

import type { PromptBlock } from "./prompt.ts";

const LOOKBACK = 20;
const MAX_SCOPES = 64;
type Frontier = { readonly index: number; readonly digest: string };

function plain(block: PromptBlock): PromptBlock {
  return block.type === "text" ? { type: "text", text: block.text } : block;
}

function digest(blocks: readonly PromptBlock[], end: number): string {
  return createHash("sha256").update(JSON.stringify(blocks.slice(0, end + 1).map(plain))).digest("hex");
}

export function cacheScope(value: unknown): string {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export class ReplayCache {
  private readonly written = new Map<string, Frontier>();

  prepare(blocks: PromptBlock[], scope?: string): { blocks: PromptBlock[]; commit(): void } {
    const latest = blocks.findLastIndex((block) => block.type === "text" && block.cache_control !== undefined);
    if (latest < 0 || !scope) return { blocks, commit() {} };
    let index = latest;
    const previous = this.written.get(scope);
    if (previous && previous.index <= latest && digest(blocks, previous.index) === previous.digest) {
      index = Math.min(latest, previous.index + LOOKBACK - 1);
      while (index > previous.index) {
        const block = blocks[index];
        if (block?.type === "text" && block.text.trim()) break;
        index--;
      }
    }
    const marker = blocks[latest];
    const selected = blocks[index];
    if (marker?.type !== "text" || selected?.type !== "text") return { blocks, commit() {} };
    const prepared = blocks.map(plain);
    prepared[index] = { ...selected, cache_control: marker.cache_control };
    const frontier = { index, digest: digest(prepared, index) };
    return { blocks: prepared, commit: () => {
      // Commit only after a successful SDK response; aborted/failed writes are unknown.
      this.written.delete(scope);
      this.written.set(scope, frontier);
      if (this.written.size > MAX_SCOPES) {
        const oldest = this.written.keys().next().value;
        if (oldest !== undefined) this.written.delete(oldest);
      }
    } };
  }
}
