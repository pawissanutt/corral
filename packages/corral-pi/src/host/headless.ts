// Headless sessions bind the role extensions without creating a terminal UI. Owner and engine
// input both arrive on the session socket; the process log shows turn boundaries.
import type { BridgeEvent, EventLog } from "../bridge/events.ts";
import type { CapabilitySet } from "../roles/types.ts";
import type { HostSession } from "./session.ts";

export const HEADLESS = "headless";

export function headlessEnabled(capabilities: CapabilitySet | undefined): boolean {
  return capabilities?.enabled.includes(HEADLESS) === true;
}

/** The pane's log line for one host event; events the owner need not see return undefined. */
export function headlessLine(event: BridgeEvent): string | undefined {
  const at = event.at.slice(11, 19);
  switch (event.kind) {
    case "headless":
      return `${at} headless: input arrives on the session socket`;
    case "ready":
      return `${at} session socket ready`;
    case "turn_started":
      return `${at} turn ${event.turn} started`;
    case "turn_ended":
      return `${at} turn ${event.turn} ended${event.cancelled === true ? " (cancelled)" : ""}`;
    case "tool_denied":
      return `${at} denied ${event.tool}`;
    case "network_refused":
      return `${at} network refused: ${JSON.stringify(event.destination)}`;
    case "usage_report_failed":
      return `${at} usage report failed: ${event.reason}`;
    case "shutdown":
      return `${at} shutting down`;
    default:
      return undefined;
  }
}

/**
 * Attaches a bound host without a terminal: logs to `write`, then binds extensions, which starts a
 * fresh session's kickoff turn. Returns the log subscription's release; the bridge socket keeps
 * the process alive until a `shutdown` command or a signal ends it.
 */
export async function attachHeadless(
  host: HostSession, events: EventLog, write: (line: string) => void,
): Promise<() => void> {
  const off = events.on((event) => {
    const line = headlessLine(event);
    if (line !== undefined) write(`${line}\n`);
  });
  host.markAttached("headless");
  await host.session.bindExtensions({
    mode: "print",
    onError: (error) => write(`extension error (${error.extensionPath}): ${String(error.error)}\n`),
  });
  return off;
}
