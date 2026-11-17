// Pi delivers steering input only between tool batches, so a model-visible wait would hold
// queued owner or corral input back for its whole budget. Such waits end early instead.
import { nestedChild } from "./codemode/child.ts";

export const INPUT_QUEUED_NOTICE = "Wait ended early: new input is queued and arrives right after this result. "
  + "The awaited work continues; read the input, then wait again if it is still needed.";

export class InputWake {
  private controller = new AbortController();
  constructor(private readonly queued: () => boolean) {}

  /** Aborted at once while steering input waits, else at the next steering enqueue. */
  signal(): AbortSignal {
    return this.queued() ? AbortSignal.abort() : this.controller.signal;
  }

  /** Steering input was enqueued: end every wait observing the current signal. */
  notify(): void {
    const ended = this.controller;
    this.controller = new AbortController();
    ended.abort();
  }
}

export interface WakeableWait {
  readonly signal: AbortSignal | undefined;
  /** True when queued input, not the caller's own cancellation, ended the wait. */
  interrupted(): boolean;
}

/** Top-level waits observe queued input; a Code Mode cell's nested wait keeps its budget, so a loop never spins. */
export function wakeableWait(wake: InputWake | undefined, tool: string, toolCallId: string, signal?: AbortSignal): WakeableWait {
  if (!wake || nestedChild(tool, toolCallId)) return { signal, interrupted: () => false };
  const input = wake.signal();
  return { signal: signal ? AbortSignal.any([signal, input]) : input, interrupted: () => input.aborted && signal?.aborted !== true };
}
