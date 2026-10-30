// Host events: one monotonic sequence per generation in a bounded ring. A reader whose cursor
// fell behind the ring gets an explicit gap, which it must reconcile from canonical records.
// Pages are bounded in bytes so a reply always fits a bridge frame; `next` continues a page.

export const EVENT_MAX_BYTES = 8 * 1024;

export interface BridgeEvent {
  readonly seq: number;
  readonly generation: number;
  readonly at: string;
  readonly kind: string;
  readonly [key: string]: unknown;
}

export interface EventGap {
  readonly after: number;
  readonly resumed: number;
  readonly missing: number;
  readonly reconcile: true;
}

export interface EventPage {
  readonly generation: number;
  readonly events: readonly BridgeEvent[];
  readonly last: number;
  readonly next: number;
  readonly more: boolean;
  readonly gap?: EventGap;
}

const bytes = (value: unknown) => Buffer.byteLength(JSON.stringify(value));

export class EventLog {
  private readonly ring: BridgeEvent[] = [];
  private readonly listeners = new Set<(event: BridgeEvent) => void>();
  private seq = 0;

  constructor(readonly generation: number, private readonly capacity = 512) {}

  get last(): number {
    return this.seq;
  }

  emit(kind: string, data: Record<string, unknown> = {}): BridgeEvent {
    const head = { seq: ++this.seq, generation: this.generation, at: new Date().toISOString(), kind };
    let event: BridgeEvent = { ...data, ...head };
    const size = bytes(event);
    if (size > EVENT_MAX_BYTES) event = { ...head, omitted: "event_too_large", bytes: size };
    this.ring.push(event);
    if (this.ring.length > this.capacity) this.ring.shift();
    for (const listener of this.listeners) listener(event);
    return event;
  }

  on(listener: (event: BridgeEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Events after `seq` whose JSON fits `maxBytes`; at least one when any is pending. */
  since(seq: number, maxBytes: number, limit = 256): EventPage {
    const after = this.ring.filter((event) => event.seq > seq);
    const events: BridgeEvent[] = [];
    let used = 2;
    for (const event of after) {
      const size = bytes(event) + 1;
      if (events.length > 0 && (used + size > maxBytes || events.length >= limit)) break;
      events.push(event);
      used += size;
    }
    const oldest = this.ring[0]?.seq;
    const gap = oldest !== undefined && seq + 1 < oldest
      ? { after: seq, resumed: oldest, missing: oldest - seq - 1, reconcile: true as const }
      : undefined;
    return {
      generation: this.generation, events, last: this.seq, next: events.at(-1)?.seq ?? seq,
      more: after.length > events.length, ...(gap ? { gap } : {}),
    };
  }
}
