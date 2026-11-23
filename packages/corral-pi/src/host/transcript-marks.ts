// Facts the transcript shows between messages: why a run ended and what it waits for, a model change, a host notice,
// what the owner told a child. Each is one persisted custom entry (`pi.appendEntry(MARK_ENTRY, mark)`), so it is drawn
// live and again on resume, and it never enters the model's context. `src/ui/mark-rows.ts` draws them.

import type { SessionManager } from "@earendil-works/pi-coding-agent";

export const MARK_ENTRY = "corral_mark";

/** Something the agent waits on after its run ended; `label` is the plain words the row shows. */
export interface TurnWait {
  readonly kind: "job" | "gate" | "owner" | "child" | "cell" | "event";
  readonly id: string;
  readonly label: string;
}

export interface TurnEndMark {
  readonly kind: "turn_end";
  /**
   * `done`: the agent stopped by itself with nothing open. `waiting`: it stopped and `waits` is not empty.
   * `stopped`: the owner stopped it. `error`: the provider or host failed. `length`: the reply was cut.
   * `continue_stopped`: automatic continuation gave up; `note` says why.
   */
  readonly reason: "done" | "waiting" | "stopped" | "error" | "length" | "continue_stopped";
  /** Epoch ms. */
  readonly at: number;
  readonly durationMs: number;
  readonly toolCalls: number;
  /** Tool calls that did not run because the run was stopped. */
  readonly notRun?: number;
  /** Cost of this run in USD, when the host recorded it. */
  readonly cost?: number;
  /** Tokens billed again after a cache miss during this run. */
  readonly rebilledTokens?: number;
  readonly waits: readonly TurnWait[];
  readonly note?: string;
}

export interface ModelMark {
  readonly kind: "model";
  readonly at: number;
  readonly model: string;
  readonly effort?: string;
  readonly was?: { readonly model: string; readonly effort?: string };
}

/** A host notice that used to be a transient `ui.notify` line. */
export interface NoteMark {
  readonly kind: "note";
  readonly at: number;
  readonly tone: "text" | "warning" | "error";
  readonly text: string;
}

/** What passed between the owner and a child agent in a build session. */
export interface ChildMark {
  readonly kind: "child";
  readonly at: number;
  readonly act: "asked" | "answered" | "steered";
  readonly child: string;
  readonly text: string;
}

export type TranscriptMark = TurnEndMark | ModelMark | NoteMark | ChildMark;

/** Appends marks to the session journal; false means the caller should retain its transient fallback. */
export interface MarkSink { add(mark: TranscriptMark): boolean }

export type TurnStopCause = "owner" | "host";

/** Start-up/replay has no attachment; a bound headless host supplies its journal sink directly. */
export function recordTranscriptMark(sink: MarkSink | undefined, mark: TranscriptMark, state: {
  readonly attached: "interactive" | "headless" | undefined;
  readonly plain: boolean;
  readonly bound: boolean;
  readonly build: boolean;
  readonly headless?: MarkSink;
}): boolean {
  if (!state.attached || !state.bound && !state.build) return false;
  sink = state.attached === "headless" ? state.headless : state.plain ? undefined : sink;
  if (!sink) return false;
  try { return sink.add(mark); }
  catch { return false; }
}

const VISIBLE_CUSTOM = new Set([MARK_ENTRY, "corral_code_cell", "corral_code_call", "corral_owner_question",
  "corral_command", "corral_command_monitor", "corral_job_control"]);

/**
 * Publish only after an append succeeded, including appends performed inside the SDK or an extension. `user` is
 * told which user message it was, before `entry`.
 */
export function observeTranscriptEntries(manager: SessionManager, entry: () => void, changed: () => void,
  user?: (message: object) => void): () => void {
  const { appendMessage, appendCustomEntry, appendCustomMessageEntry, appendCompaction, branchWithSummary,
    appendLabelChange, appendModelChange, appendThinkingLevelChange, appendContextEdit, appendUsage } = manager;
  const after = <A extends unknown[], R>(method: (...args: A) => R, notify: (...args: A) => void) =>
    (...args: A): R => {
      const result = method.apply(manager, args);
      notify(...args);
      return result;
    };
  manager.appendMessage = after(appendMessage, message => {
    if (message.role === "user") user?.(message);
    if (message.role === "user" || message.role === "bashExecution") entry();
    else changed();
  });
  manager.appendCustomEntry = after(appendCustomEntry, kind => { if (VISIBLE_CUSTOM.has(kind)) entry(); });
  manager.appendCustomMessageEntry = after(appendCustomMessageEntry, (_kind, _content, display) => {
    if (display) entry();
    else changed();
  });
  // appendCompaction is generic over its details, which defeats inference of the argument tuple.
  manager.appendCompaction = after<Parameters<typeof appendCompaction>, string>(appendCompaction, entry);
  manager.branchWithSummary = after(branchWithSummary, entry);
  manager.appendLabelChange = after(appendLabelChange, entry);
  manager.appendModelChange = after(appendModelChange, entry);
  manager.appendThinkingLevelChange = after(appendThinkingLevelChange, entry);
  manager.appendContextEdit = after(appendContextEdit, entry);
  manager.appendUsage = after(appendUsage, kind => { if (kind === "cache_warm") entry(); else changed(); });
  return () => { Object.assign(manager, { appendMessage, appendCustomEntry, appendCustomMessageEntry, appendCompaction,
    branchWithSummary, appendLabelChange, appendModelChange, appendThinkingLevelChange, appendContextEdit, appendUsage }); };
}
