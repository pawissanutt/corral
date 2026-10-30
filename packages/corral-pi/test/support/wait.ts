import type { BridgeEvent, EventLog } from "../../src/bridge/events.ts";

export async function within<T>(promise: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`timed out after ${ms} ms waiting for ${what}`)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/** Subscribe first, then act: resolves on the first event matching `match`. */
export function nextEvent(log: EventLog, match: (event: BridgeEvent) => boolean, ms = 30_000): Promise<BridgeEvent> {
  let off: (() => void) | undefined;
  const seen = new Promise<BridgeEvent>((resolve) => {
    off = log.on((event) => {
      if (match(event)) resolve(event);
    });
  });
  return within(seen, ms, "a host event").finally(() => off?.());
}
