// Where a status, warning or error line lands in rich panels. Pi offers every line it would add to the transcript
// (its own, and an extension's `ui.notify`) to the `statusPresentation` hook of the patched InteractiveMode; corral
// sends it to the notice system (header slot, toast, F2 Recent notices) and, when it must stay in the record, draws
// one quiet transcript row in the run-mark geometry (`quietNote`, mark-rows.ts). The words are a plain sentence
// without Pi's or corral's framing prefix; whatever the sentence leaves out travels exactly as the notice's `detail`
// and in the row's expanded text. With no sink (panels disposed), or nothing to say, Pi keeps its own line.
// Presentation only: Pi's callers, their retry and error paths, and what the model receives are unchanged.
import type { StatusNotice, StatusPresentation } from "@earendil-works/pi-coding-agent";
import type { Component } from "@earendil-works/pi-tui";

export type NoticeTone = "text" | "success" | "warning" | "error";
/** The notice system: `text` is the sentence shown; `detail` is the exact raw text, kept for Recent notices. */
export interface NoticeSink { notice(text: string, tone?: NoticeTone, detail?: string): void }
/** `notice` only; `notice+row` both; `row` the transcript row only; `pi` Pi's own line. */
export type NoticeRoute = "notice" | "notice+row" | "row" | "pi";
export interface RouteContext { /** Chat is the open view. */ readonly inChat: boolean }
type RowTone = "text" | "warning" | "error";

/** `[in Chat, away from Chat]`. */
type Places = readonly [NoticeRoute, NoticeRoute];
const NOTICE: Places = ["notice", "notice"], RECORD: Places = ["notice+row", "notice+row"], OUTCOME: Places = ["row", "notice+row"];
/**
 * The whole policy. Transient words are a notice. An error also leaves a row, so a loss stays in the record. A model
 * or effort line is a notice only: the transcript's model mark already records a change, and a second row would
 * claim one that may not have happened. A compaction outcome is a row where the compaction divider would be, and a
 * notice as well when the owner is looking at another view.
 */
const POLICY: Readonly<Record<"general" | "selection" | "compaction", Readonly<Record<StatusNotice["level"], Places>>>> = {
  general: { info: NOTICE, warning: NOTICE, error: RECORD },
  selection: { info: NOTICE, warning: NOTICE, error: NOTICE },
  compaction: { info: OUTCOME, warning: OUTCOME, error: OUTCOME },
};

/** The words of one line: the notice sentence and tone, the exact raw text when the sentence is not all of it, the row. */
interface Words {
  readonly notice: string;
  readonly tone: NoticeTone;
  readonly detail?: string;
  /** The row's text: its first line is the row, the rest shows expanded. */
  readonly row: string;
  readonly rowTone: RowTone;
}

const TONES: Readonly<Record<StatusNotice["level"], RowTone>> = { info: "text", warning: "warning", error: "error" };
/** Pi drops its own `Error: `/`Warning: ` before the hook; an extension's text may still carry it, or corral's `corral: `. */
const FRAMES: Readonly<Record<StatusNotice["level"], RegExp>> = {
  info: /^\s*corral:\s*/u,
  warning: /^\s*(?:corral:\s*|Warning:\s*)+/u,
  error: /^\s*(?:corral:\s*|Error:\s*)+/u,
};

// Pi's own texts (interactive-mode `compaction_end` and `queueCompactionMessage`; agent-session compaction errors).
const CANCELLED: Readonly<Record<string, string>> = {
  "Compaction cancelled": "compaction cancelled",
  "Auto-compaction cancelled": "automatic compaction cancelled",
};
const FAILED = /^(Compaction failed|Auto-compaction failed|Context overflow recovery failed): /u;
/** Reasons a second `/compact` cannot change. */
const SETTLED: Readonly<Record<string, true>> = { "Already compacted": true, "Nothing to compact (session too small)": true };
const RETRY = "Send /compact to try again.";
const PHRASES: Readonly<Record<string, string>> = { "Queued message for after compaction": "Queued · sends after compaction" };

/** A compaction outcome: what happened, that the context is unchanged, the exact text and a next step when one helps. */
function compactionWords(notice: StatusNotice, body: string): Words | undefined {
  const cancelled = CANCELLED[notice.text.trim()];
  if (cancelled) {
    const text = `${cancelled} · context unchanged`;
    return { notice: text.charAt(0).toUpperCase() + text.slice(1), tone: "text", row: text, rowTone: "text" };
  }
  if (notice.level !== "error") return undefined;
  const raw = body.trim(), failed = FAILED.exec(raw);
  const what = failed && failed[1] !== "Compaction failed" ? "automatic compaction failed" : "compaction failed";
  const text = `${what} · context unchanged`, retry = failed !== null && !SETTLED[raw.slice(failed[0].length).trim()];
  return {
    notice: text.charAt(0).toUpperCase() + text.slice(1), tone: "error", detail: notice.text,
    row: [text, raw, ...(retry ? [RETRY] : [])].join("\n"), rowTone: "error",
  };
}

/** The route and words of one line, or undefined when it has nothing to say past its framing (Pi's line). */
function routed(notice: StatusNotice, inChat: boolean): { readonly route: Exclude<NoticeRoute, "pi">; readonly words: Words } | undefined {
  const body = notice.text.replace(FRAMES[notice.level], "");
  const kind = notice.topic === "compaction" ? "compaction" : notice.topic === "model" || notice.topic === "effort" ? "selection" : "general";
  let words = kind === "compaction" ? compactionWords(notice, body) : undefined;
  if (!words) {
    const first = body.split("\n").find(line => line.trim() !== "")?.trim() ?? "";
    if (first === "") return undefined;
    // A dropped prefix is framing, not content: the detail is kept when the sentence leaves out any of the text itself.
    const detail = body.trim() === first ? undefined : notice.text, tone = TONES[notice.level];
    words = { notice: PHRASES[first] ?? first, tone, ...(detail === undefined ? {} : { detail }), row: body, rowTone: tone };
  }
  const [chat, away] = POLICY[kind][notice.level], route = inChat ? chat : away;
  return route === "pi" ? undefined : { route, words };
}

/** The route of one line; `pi` when it has nothing to say past its framing. */
export function noticeRoute(notice: StatusNotice, context: RouteContext): NoticeRoute {
  return routed(notice, context.inChat)?.route ?? "pi";
}

/**
 * The `statusPresentation` hook. Reads its dependencies per call; `sink()` undefined (panels disposed) keeps Pi's
 * line. A notice the sink fails to take is never lost: a notice-only line falls back to Pi's line, and a recorded
 * one keeps its row.
 */
export function statusPresenter(deps: {
  sink(): NoticeSink | undefined;
  inChat(): boolean;
  /** The quiet transcript row (`quietNote` in mark-rows.ts, the drawer of a NoteMark). */
  row(text: string, tone: RowTone, at: number): Component;
}): StatusPresentation {
  return notice => {
    const sink = deps.sink(), placed = sink ? routed(notice, deps.inChat()) : undefined;
    if (!sink || !placed) return undefined;
    const { route, words } = placed, at = Date.now();
    let told = route === "row";
    if (!told) {
      try {
        sink.notice(words.notice, words.tone, words.detail);
        told = true;
      } catch {
        // The sink lost it; the row or Pi's own line still carries the message.
      }
    }
    if (route === "notice") return told ? true : undefined;
    return deps.row(words.row, words.rowTone, at);
  };
}
