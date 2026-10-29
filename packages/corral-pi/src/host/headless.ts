// Headless host mode (T-577; plan T-460 menu 24). When the generation's snapshot enables
// `headless`, the host binds the role's extensions itself instead of starting InteractiveMode, so
// no terminal UI exists and nothing reads the terminal. The kickoff still starts when extensions
// bind; answers, wakes and nudges arrive only through the authenticated bridge (`submit_input`)
// from corral's durable inbox, as they do for an interactive host. The pane shows a plain log.
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
      return `${at} headless: no terminal input; corral delivers prompts, answers and wakes through the inbox`;
    case "turn_started":
      return `${at} turn ${event.turn} started`;
    case "turn_ended":
      return `${at} turn ${event.turn} ended${event.cancelled === true ? " (cancelled)" : ""}`;
    case "tool_denied":
      return `${at} denied ${event.tool}`;
    case "network_refused":
      return `${at} network refused: ${JSON.stringify(event.destination)}`;
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
