// Observe the real editor path; never submit a second prompt or interpret model completion as admission.
import type { AgentSession } from "@earendil-works/pi-coding-agent";
import type { EventLog } from "../bridge/events.ts";
import type { SubmitResult } from "../bridge/protocol.ts";

export interface InputObservation extends SubmitResult {
  readonly key: string;
  readonly policy?: string;
}

export function observeTerminalInput(session: AgentSession, events: EventLog): void {
  const prompt = session.prompt.bind(session);
  let sequence = 0;
  session.prompt = async (text, options) => {
    if (options?.source && options.source !== "interactive") return prompt(text, options);
    const inputSequence = ++sequence, key = `terminal:${inputSequence}`;
    let admitted = false;
    const report = (result: SubmitResult) => events.emit("terminal_input", {
      key, input_sequence: inputSequence, policy: options?.streamingBehavior ?? "send", ...result,
    });
    report({ disposition: "pending", reason: "preflight_pending" });
    try {
      await prompt(text, { ...options, preflightResult: disposition => {
        admitted = true;
        report({ disposition });
        options?.preflightResult?.(disposition);
      } });
    } catch (error) {
      // A provider/run error after `started` is not a rejected submission.
      if (!admitted) report({ disposition: "rejected", reason: (error instanceof Error ? error.message : String(error)).slice(0, 256) });
      throw error;
    }
  };
}
