// An owner `!command` / `!!command` as a tool row. Pi keeps the block's state (its output, status, exit code); this
// draws it in the tool row grammar of the agent's own `bash` row: `<glyph> bash <command> · <outcome>` with `you` (and
// for `!!` that the output is not sent to the agent) and the time at the right, the last two output lines while it
// runs, the last three after a failure. The owner is always named: at the right of the row line, or, when the kit
// dropped that label for lack of room, in one muted body row. The expanded row (a click or Ctrl+O, the transcript's
// own expansion) holds the whole command, every output line Pi kept and the exact path of the saved full output.
// Presentation only: nothing here runs, stops or records a command, and the control characters of the output are made
// harmless at draw time, never in the state.
//
// The state carries a start time only for a command Pi ran live; a block redrawn from the session file has none and
// shows no time. A running block holds the shared clock (`Ticker.hold`) and releases it on the first draw that sees it
// settled. Pi gives a presenter no disposal call, so a block that never draws again (the transcript was cleared while
// it ran) is released by the next draw of any block once it has not been drawn for `STALE_MS`.
import type { BashExecutionState, BashPresentation } from "@earendil-works/pi-coding-agent";

import type { Ticker } from "./activity.ts";
import { commandLine } from "./tool-lines-jobs.ts";
import { firstLine, lineCount, rowBlock, tailLines, textLines } from "./tool-row.ts";
import type { BodyLine, RowModel, RowState, Tone } from "./tool-row.ts";

/** What `!!` means, in the row's words. */
const NOT_SENT = "not sent to the agent";
/** A held clock not asked for this long belongs to a block that is no longer drawn. */
const STALE_MS = 10_000;
const RUNNING_TAIL = 2, FAILED_TAIL = 3;

interface Held { readonly release: () => void; seen: number }
interface Drawn { readonly width: number; readonly lines: readonly string[] }

/** Identifies one running block across the states Pi replaces as output arrives. */
function blockKey(state: BashExecutionState): string {
  return `owner-bash:${state.startedAt ?? "-"}:${state.command.slice(0, 64)}`;
}

/** The last line worth reading after a success: at least three letters or digits, never a `set -x` trace nor JSON. */
function lastReadable(lines: readonly string[]): string | undefined {
  for (let index = lines.length - 1; index >= 0; index--) {
    const line = lines[index]!;
    if (/^\s*\++ /.test(line) || /^\s*[[{]/.test(line)) continue;
    if ((line.match(/[\p{L}\p{N}]/gu)?.length ?? 0) >= 3) return firstLine(line);
  }
  return undefined;
}

interface Ending { readonly state: RowState; readonly outcome?: string; readonly tone?: Tone }

/** How the block stands, in words. An absent exit code is never read as 0, and a failure to run never invents one. */
function ending(state: BashExecutionState, empty: boolean): Ending {
  if (state.status === "running") return { state: "running" };
  const code = state.exitCode !== undefined && Number.isFinite(state.exitCode) ? state.exitCode : undefined;
  if (state.status === "cancelled") {
    return { state: "waiting", outcome: code === undefined ? "stopped by you" : `stopped by you · exit ${code}`, tone: "warning" };
  }
  if (code === undefined) {
    return state.status === "error" ? { state: "failed", outcome: "failed" } : { state: "unknown", outcome: "no exit code", tone: "warning" };
  }
  if (code === 0) return { state: "ok", outcome: empty ? "exit 0 · no output" : "exit 0" };
  return { state: "failed", outcome: `exit ${code}` };
}

/**
 * The row for one state. `how`: `row` is the row with its right label; `note` adds the owner's own body row because the
 * row line had no room for the label; `bare` is the row without right label and count, the line the kit draws once it
 * has dropped both (see `draw`).
 */
function modelOf(state: BashExecutionState, cwd: string, how: "row" | "note" | "bare"): RowModel {
  const total = lineCount(state.lines.join("\n"));
  const end = ending(state, total === 0);
  const saved = state.fullOutputPath;
  const target = commandLine(state.command, cwd);
  const noteRows: BodyLine[] = how === "note" ? [{ text: state.excludeFromContext ? `you ran this · ${NOT_SENT}` : "you ran this" }] : [];
  const cutRows: BodyLine[] = state.truncated ? [{ text: saved ? "output cut · full output saved" : "output cut", tone: "warning" }] : [];
  const dataRows = end.state === "running" ? tailLines(state.lines.join("\n"), RUNNING_TAIL)
    : end.state === "failed" ? tailLines(state.lines.join("\n"), FAILED_TAIL) : [];
  const body = [...noteRows, ...cutRows, ...dataRows];
  // A count belongs to the output rows it follows; under a note alone it would read as the note's count.
  const countable = noteRows.length + cutRows.length === 0 || dataRows.length > 0;
  const hidden = end.state === "running" || !countable ? undefined
    : end.state === "failed" ? (total > FAILED_TAIL ? `${total - FAILED_TAIL} more lines` : undefined)
      : total > 0 ? `${total} ${total === 1 ? "line" : "lines"}` : undefined;
  const last = end.state === "ok" ? lastReadable(state.lines) : undefined;
  const duration: Pick<RowModel, "duration"> = state.status === "running"
    ? state.startedAt === undefined ? {} : { duration: { since: state.startedAt } }
    : state.startedAt !== undefined && state.endedAt !== undefined && state.endedAt >= state.startedAt ? { duration: state.endedAt - state.startedAt } : {};
  const output = state.lines.join("\n");
  return {
    state: end.state, verb: "bash", ...(target ? { target } : {}),
    ...(end.outcome ? { outcome: end.outcome } : {}), ...(end.tone ? { outcomeTone: end.tone } : {}),
    ...(last ? { detail: last } : {}),
    ...(how === "bare" ? {} : { right: state.excludeFromContext ? `you · ${NOT_SENT}` : "you" }),
    ...duration,
    ...(body.length ? { body } : {}), ...(hidden && how !== "bare" ? { hiddenHint: hidden } : {}),
    expanded: (): readonly BodyLine[] => [
      ...noteRows,
      ...textLines(state.command, "text"),
      ...(output.trim() ? [{ text: "" }, ...textLines(output)] : []),
      ...(state.truncated
        ? [{ text: "" }, { text: "output cut", tone: "warning" as const },
          ...(saved ? [{ text: "full output saved:", tone: "warning" as const }, { text: saved, tone: "text" as const }] : [])]
        : []),
    ],
  };
}

/** The block at `width`: a blank line, then the row inset like the transcript's other tool rows. */
function draw(state: BashExecutionState, width: number, cwd: string, now: number): string[] {
  const pad = Math.max(0, Math.min(1, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
  const block = (how: "row" | "note") => rowBlock(modelOf(state, cwd, how), room, now, state.expanded).lines;
  let lines = block("row");
  // The kit drops the count, then the right label, when the line is too long; what is left is exactly the line of the
  // model without either. So the owner's label was dropped when the row line equals that bare line. This reads the
  // layout, never the text, which holds the untrusted command and output.
  if (lines[0] === rowBlock(modelOf(state, cwd, "bare"), room, now, state.expanded).lines[0]) lines = block("note");
  const inset = " ".repeat(pad);
  return ["", ...lines.map(line => inset + line)];
}

/** An owner `!command` as a tool row; holds the shared clock while a block runs. */
export function ownerBashPresenter(deps: { cwd: string; ticker(): Ticker | undefined }): BashPresentation {
  const holds = new Map<string, Held>();
  // A settled block draws the same lines until its width changes; Pi replaces the state object when the block does.
  const settled = new WeakMap<BashExecutionState, Drawn>();

  function keepClock(state: BashExecutionState, ticker: Ticker | undefined, now: number): void {
    const key = blockKey(state);
    for (const [id, held] of holds) {
      if (id !== key && now - held.seen > STALE_MS) { held.release(); holds.delete(id); }
    }
    if (state.status !== "running") {
      holds.get(key)?.release();
      holds.delete(key);
      return;
    }
    if (!ticker) return;
    const held = holds.get(key);
    if (held) held.seen = now;
    else holds.set(key, { release: ticker.hold(key), seen: now });
  }

  return (state, width) => {
    const ticker = deps.ticker(), now = ticker?.now() ?? Date.now();
    keepClock(state, ticker, now);
    if (state.status === "running") return draw(state, width, deps.cwd, now);
    const known = settled.get(state);
    if (known?.width === width) return [...known.lines];
    const lines = draw(state, width, deps.cwd, now);
    settled.set(state, { width, lines });
    return [...lines];
  };
}
