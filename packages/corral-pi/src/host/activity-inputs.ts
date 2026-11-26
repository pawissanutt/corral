import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { isInputId } from "./remote/input.ts";

export type ActivityInputState = "pending" | "applied" | "failed";
type TerminalInput = { state: "applied" | "failed"; turn: number };
type InputReceipt = TerminalInput | { state: "pending"; quiet_ms: number };

// Explicitly reserved for engine obligations; app/automation traffic does not
// consume this budget or evict a tracked board receipt.
export function isBoardInputId(id: unknown): id is string {
  return isInputId(id) && id.startsWith("board-");
}
/** Unlike corralRequestId, this is never inferred from the bridge frame ID. */
export const ACTIVITY_INPUT_ID = "corralInputId";
const MAX_PENDING = 16;
const MAX_RECENT = 32;

/** Holds IDs only. Live receipts cannot age out while Pi still owns the input. */
export class ActivityInputs {
  private readonly pending = new Map<string, number>();
  private readonly claimed = new Set<string>();
  private readonly recent = new Map<string, TerminalInput>();
  private settledAt: number;

  constructor(private readonly turn = () => 0, private readonly now = () => performance.now()) {
    this.settledAt = now();
  }

  get(id: string): ActivityInputState | undefined {
    return this.pending.has(id) ? "pending" : this.recent.get(id)?.state;
  }

  admit(id: string): boolean {
    if (!isBoardInputId(id) || this.get(id) !== undefined || this.pending.size >= MAX_PENDING) return false;
    this.pending.set(id, this.now());
    return true;
  }

  /** A start/drain claims the input for this run, but is not proof Pi recorded it. */
  claim(id: string): void {
    if (this.pending.has(id)) this.claimed.add(id);
  }

  finish(id: string, state: "applied" | "failed"): void {
    if (!this.pending.delete(id)) return;
    this.claimed.delete(id);
    this.remember(id, { state, turn: this.turn() });
  }

  settle(): void {
    this.settledAt = this.now();
    // An abort or persistence failure can end a run without recording a claimed input.
    for (const id of this.claimed) this.finish(id, "failed");
  }

  restore(entries: readonly SessionEntry[]): void {
    // Scan on open, not on `get_state`, and keep only the newest distinct persisted IDs.
    const applied = new Set<string>();
    for (let i = entries.length - 1; i >= 0 && applied.size < MAX_RECENT; i--) {
      const entry = entries[i];
      if (entry?.type !== "message" || entry.message.role !== "user" || !(ACTIVITY_INPUT_ID in entry.message)) continue;
      const id = entry.message[ACTIVITY_INPUT_ID];
      if (isBoardInputId(id)) applied.add(id);
    }
    for (const id of [...applied].reverse()) this.remember(id, { state: "applied", turn: 0 });
  }

  snapshot(): Record<string, InputReceipt> {
    const now = this.now();
    return Object.fromEntries([
      ...this.recent,
      ...Array.from(this.pending, ([id, at]) => [id, {
        state: "pending", quiet_ms: Math.floor(now - Math.max(at, this.settledAt)),
      }] as const),
    ]);
  }

  private remember(id: string, receipt: TerminalInput): void {
    this.recent.delete(id);
    this.recent.set(id, receipt);
    if (this.recent.size > MAX_RECENT) this.recent.delete(this.recent.keys().next().value!);
  }
}
