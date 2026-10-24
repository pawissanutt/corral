// The real delivery module (inbox-adapter). For every new bridge input it appends one
// `corral_delivery` custom entry to the Pi session before Pi sees the text. corral's pi_native
// adapter observes a message only from this entry: its idempotency key names the envelope and
// its text_sha256 must equal the envelope's SHA-256. The host deduplicates by key, so a resend
// after an uncertain send never writes a second entry.
import type { DeliveryInput, DeliveryModule } from "./types.ts";

export const ENTRY_TYPE = "corral_delivery";
export const ENTRY_VERSION = 1;
const RECENT_MAX = 8;

export interface DeliveryEntry {
  readonly version: number;
  readonly idempotency_key: string;
  readonly request_id: string;
  readonly origin: string;
  readonly busy_policy: DeliveryInput["busyPolicy"];
  readonly text_sha256: string;
  readonly text_bytes: number;
  readonly generation: number;
  readonly session_id: string;
}

let accepted = 0;
const recent: Pick<DeliveryEntry, "idempotency_key" | "origin" | "text_sha256">[] = [];

const delivery: DeliveryModule = {
  accept(input, host) {
    const entry: DeliveryEntry = {
      version: ENTRY_VERSION,
      idempotency_key: input.idempotencyKey,
      request_id: input.requestId,
      origin: input.origin,
      busy_policy: input.busyPolicy,
      text_sha256: input.textSha256,
      text_bytes: Buffer.byteLength(input.text),
      generation: input.generation,
      session_id: host.sessionId(),
    };
    host.appendEntry(ENTRY_TYPE, { ...entry });
    accepted++;
    recent.push({ idempotency_key: entry.idempotency_key, origin: entry.origin, text_sha256: entry.text_sha256 });
    if (recent.length > RECENT_MAX) recent.shift();
  },
  state() {
    return { module: ENTRY_TYPE, version: ENTRY_VERSION, accepted, recent: recent.map((entry) => ({ ...entry })) };
  },
};

export default delivery;
