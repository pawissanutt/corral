// The delivery module interface (F6), loaded by fixed path from src/delivery/index.ts.
// The bridge hands every new (non-replayed) submit_input to the module before the text reaches
// Pi, so the module can write the durable observation corral reads (inbox-adapter).

export type BusyPolicy = "steer" | "follow_up" | "prompt";

export interface DeliveryInput {
  /** The bridge request ID that carried the input. */
  readonly requestId: string;
  readonly idempotencyKey: string;
  readonly origin: string;
  readonly busyPolicy: BusyPolicy;
  readonly text: string;
  /** SHA-256 (hex) of the UTF-8 text. */
  readonly textSha256: string;
  readonly generation: number;
}

/** What the host lends a delivery module. */
export interface DeliveryHost {
  /** Appends a custom entry to the current Pi session file (never sent to the model). */
  appendEntry(customType: string, data: Record<string, unknown>): void;
  sessionId(): string;
}

export interface DeliveryModule {
  /** Called once per new input, before Pi sees it; a throw rejects the input. */
  accept(input: DeliveryInput, host: DeliveryHost): void | Promise<void>;
  /** Non-secret readback for get_state. */
  state?(): Record<string, unknown>;
}
