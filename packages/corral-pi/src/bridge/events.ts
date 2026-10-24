// Host events: one monotonic sequence per generation in a bounded ring. A reader whose cursor
// fell behind the ring gets an explicit gap, which it must reconcile from canonical records.

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
  readonly gap?: EventGap;
}

export class EventLog {
  private readonly ring: BridgeEvent[] = [];
  private readonly listeners = new Set<(event: BridgeEvent) => void>();
  private seq = 0;

  constructor(readonly generation: number, private readonly capacity = 512) {}

  get last(): number {
    return this.seq;
  }

  emit(kind: string, data: Record<string, unknown> = {}): BridgeEvent {
    const event: BridgeEvent = {
      ...data, seq: ++this.seq, generation: this.generation, at: new Date().toISOString(), kind,
    };
    this.ring.push(event);
    if (this.ring.length > this.capacity) this.ring.shift();
    for (const listener of this.listeners) listener(event);
    return event;
  }

  on(listener: (event: BridgeEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  since(seq: number, limit = 256): EventPage {
    const events = this.ring.filter((event) => event.seq > seq).slice(0, limit);
    const oldest = this.ring[0]?.seq;
    const gap = oldest !== undefined && seq + 1 < oldest
      ? { after: seq, resumed: oldest, missing: oldest - seq - 1, reconcile: true as const }
      : undefined;
    return { generation: this.generation, events, last: this.seq, ...(gap ? { gap } : {}) };
  }
}
