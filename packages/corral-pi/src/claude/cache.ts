// One history marker shares Claude Code's four-breakpoint budget. Large image batches retain a
// reachable earlier write; ordinary text batches are sealed into compact, stable prompt blocks.
import { createHash } from "node:crypto";

import type { PromptBlock } from "./prompt.ts";
import { cacheLifetimeMs } from "./cache-policy.ts";

const LOOKBACK = 20;
const MAX_SCOPES = 64;
type Frontier = { readonly index: number; readonly digest: string; readonly startedAt: number; readonly request: number };

export interface ReplayObservation {
  readonly localState: "disabled" | "untracked" | "cold" | "continued" | "changed" | "expired";
  readonly historyBlocks: number;
  readonly cachedThroughBlock: number;
  readonly uncachedTailBlocks: number;
  readonly prefixHash?: string;
  readonly sinceConfirmedStartMs?: number;
}

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
  private request = 0;

  prepare(blocks: PromptBlock[], scope?: string, now = performance.now()): {
    blocks: PromptBlock[]; observation: ReplayObservation; commit(): void;
  } {
    const request = ++this.request;
    const latest = blocks.findLastIndex((block) => block.type === "text" && block.cache_control !== undefined);
    const marker = blocks[latest];
    if (latest < 0 || marker?.type !== "text" || !marker.cache_control) {
      return { blocks, observation: { localState: "disabled", historyBlocks: 0, cachedThroughBlock: 0,
        uncachedTailBlocks: 0 }, commit() {} };
    }
    const previous = scope ? this.written.get(scope) : undefined;
    const elapsed = previous ? now - previous.startedAt : undefined;
    const matches = previous !== undefined && previous.index <= latest && digest(blocks, previous.index) === previous.digest;
    const expired = elapsed !== undefined && (elapsed < 0 || elapsed >= cacheLifetimeMs(marker.cache_control.ttl));
    const localState = !scope ? "untracked" : !previous ? "cold" : !matches ? "changed" : expired ? "expired" : "continued";
    let index = latest;
    if (previous && matches && !expired) {
      index = Math.min(latest, previous.index + LOOKBACK - 1);
      while (index > previous.index) {
        const block = blocks[index];
        if (block?.type === "text" && block.text.trim()) break;
        index--;
      }
    }
    const selected = blocks[index];
    if (selected?.type !== "text") throw new Error("claude_cache_boundary_invalid: expected a text history boundary");
    const prepared = blocks.map(plain);
    prepared[index] = { ...selected, cache_control: marker.cache_control };
    const frontier = { index, digest: digest(prepared, index), startedAt: now, request };
    return { blocks: prepared, observation: {
      localState, historyBlocks: latest + 1, cachedThroughBlock: index + 1,
      uncachedTailBlocks: latest - index, prefixHash: frontier.digest,
      ...(elapsed === undefined ? {} : { sinceConfirmedStartMs: Math.max(0, elapsed) }),
    }, commit: () => {
      // An uncertain request never commits. A late completion cannot regress a newer frontier.
      if (!scope || (this.written.get(scope)?.request ?? 0) > request) return;
      this.written.delete(scope);
      this.written.set(scope, frontier);
      if (this.written.size > MAX_SCOPES) {
        const oldest = this.written.keys().next().value;
        if (oldest !== undefined) this.written.delete(oldest);
      }
    } };
  }
}
