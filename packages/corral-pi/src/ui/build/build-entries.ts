// Conversation entries a build session posts for the owner's decisions: the exit outcome, the board filing and
// an owner-opened finish card. Their content reaches the parent model (custom messages become user content), so
// it states the decision and its outcome in plain sentences. The renderer draws exit and board entries as a
// `you chose …` bubble (the user message surface) plus an outcome line, and finish entries as the live result card.
import type { MessageRenderer } from "@earendil-works/pi-coding-agent";
import { wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { BoardTaskStatus, ExitChoice, ExitOption, ExitPhase, ExitRecord, ExitState, LandCandidate, LandProgress } from "../../build/types.ts";
import { onSurface } from "../frame.ts";
import { plainLabel, PANEL_BG, PANEL_BUBBLE, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT } from "../panels-view.ts";
import type { BuildCardSource } from "./contract.ts";
import { candidateLines, landLines } from "./land-steps.ts";
import { EXIT_LABELS, exitLines, exitProgress, resultCardLines, shortSha } from "./result-card.ts";
import type { ResultHit } from "./result-card.ts";
import { cardCell, cardLine, cardWrap } from "./task-card.ts";

/** The widest user message bubble (the mock's `.msg`). */
const BUBBLE_MAX = 96;

export const BUILD_EXIT_ENTRY = "corral_build_exit";
export const BUILD_BOARD_ENTRY = "corral_build_board";
export const BUILD_FINISH_ENTRY = "corral_build_finish";

export interface BuildEntry<T> { readonly customType: string; readonly content: string; readonly display: true; readonly details: T }
export type ExitEntryDetails = { readonly choice: ExitChoice; readonly label: string; readonly phase: ExitPhase; readonly detail?: string;
  /** Land's steps as they ended, with a failed gate's last log lines. */
  readonly land?: LandProgress;
  /** The candidate the host prepared for this Land, as it stood when Land ended. */
  readonly candidate?: LandCandidate }
  & Pick<ExitRecord, "landed" | "baseBranch" | "gateAttempt" | "overrideReason">;
export interface BoardEntryDetails { readonly filed: readonly { readonly id: string; readonly title: string; readonly depends: readonly string[] }[]; readonly started: boolean }
export interface FinishEntryDetails { readonly id: string }

/**
 * A card drawn from live data on every render. `draw` returns the lines and the rows a click acts on; a plain
 * left click on such a row calls `act` with its hit. `inset` cells are kept free on both sides: tool cards draw
 * their own frame and conversation entries get the full row, so both pass Pi's output padding here.
 */
export class BuildCard<T> implements Component {
  private hits: ReadonlyMap<number, T> = new Map();
  constructor(
    private readonly draw: (width: number) => { readonly lines: string[]; readonly hits?: ReadonlyMap<number, T> },
    private readonly act?: (hit: T) => void,
    private readonly inset = 0,
  ) {}
  render(width: number): string[] {
    const pad = Math.max(0, Math.min(this.inset, Math.floor((width - 1) / 2)));
    const { lines, hits } = this.draw(Math.max(1, width - pad * 2));
    this.hits = hits ?? new Map();
    return pad ? lines.map(line => " ".repeat(pad) + line) : lines;
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.act || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const hit = this.hits.get(event.y);
    if (hit === undefined) return undefined;
    this.act(hit);
    return { handled: true };
  }
  invalidate(): void {}
}

/**
 * Exit card `id` (a `build_finish` tool call or a finish entry) shows live progress when the source names it as
 * the latest exit card, or names none: a resumed session then reads the current exit.
 */
export function finishLive(cards: BuildCardSource | undefined, id: string): boolean {
  const latest = cards?.latestFinish();
  return latest === undefined || latest === id;
}
/** The result card of exit card `id`, drawn from live data. Clicks open a slice's conversation or the pending exit choice. */
export function resultCard(source: () => BuildCardSource | undefined, id: string, note?: string, inset = 0): BuildCard<ResultHit> {
  return new BuildCard<ResultHit>(width => {
    const cards = source(), state = cards?.state(), live = finishLive(cards, id), since = cards?.exitSince?.();
    return resultCardLines({
      summary: live ? cards?.summary() : undefined, ...(note === undefined ? {} : { note }),
      fleet: state?.fleet, exit: state?.exit ?? { phase: "none" }, options: [], live,
      ...(state?.record ? { record: state.record } : {}), ...(since === undefined ? {} : { exitSince: since }),
    }, width, hue => cards?.color(hue) ?? "", cards?.now() ?? Date.now());
  }, hit => {
    const cards = source();
    if (hit.child) cards?.openChild(hit.child);
    else if (hit.exit) cards?.exit();
  }, inset);
}

function sentence(text: string): string {
  const line = plainLabel(text);
  return !line || /[.!?]$/.test(line) ? line : line + ".";
}
function outcome(details: Pick<ExitEntryDetails, "choice" | "phase" | "detail">): string {
  if (details.detail) return sentence(details.detail);
  switch (details.phase) {
    case "kept": return "The branch stays in its worktree; nothing was merged.";
    case "landed": return "The branch landed.";
    case "failed": return "The exit failed.";
    case "rebasing": case "verifying": case "repairing": case "landing": return "The exit is still running.";
    default: return details.choice === "iterate" ? "The build session continues; nothing was landed." : "No exit ran.";
  }
}

/** Content is model-visible (custom messages convert to user content): `The owner chose "Land on the base branch" for this build branch. Landed. main now points at 3f2a9c1 (gate G3).` */
export function exitEntry(option: ExitOption, state: ExitState): BuildEntry<ExitEntryDetails> {
  const label = plainLabel(option.label), record = state.record, detail = state.detail ? plainLabel(state.detail) : undefined;
  const details: ExitEntryDetails = {
    choice: option.choice, label, phase: state.phase, ...(detail ? { detail } : {}),
    ...(record?.landed ? { landed: plainLabel(record.landed) } : {}), ...(record?.baseBranch ? { baseBranch: plainLabel(record.baseBranch) } : {}),
    ...(record?.gateAttempt ? { gateAttempt: plainLabel(record.gateAttempt) } : {}),
    ...(record?.overrideReason ? { overrideReason: plainLabel(record.overrideReason) } : {}),
    ...(option.choice === "land" && state.land ? { land: state.land } : {}),
    ...(option.choice === "land" && state.candidate ? { candidate: state.candidate } : {}),
  };
  // The base fast-forwarded from the tip the candidate was prepared on, when the host says so.
  const from = details.landed && details.candidate?.baseTip && details.candidate.head === details.landed ? ` (fast-forwarded from ${shortSha(details.candidate.baseTip)})` : "";
  const sentences = [
    `The owner chose "${label}" for this build branch.`, outcome(details),
    ...(details.landed ? [`${details.baseBranch ?? "The base branch"} now points at ${shortSha(details.landed)}${from}${details.gateAttempt ? ` (gate ${details.gateAttempt})` : ""}.`] : []),
    ...(details.overrideReason ? [`The owner landed over open review findings: "${details.overrideReason}".`] : []),
  ];
  return { customType: BUILD_EXIT_ENTRY, display: true, details, content: sentences.join(" ") };
}

export function boardEntry(filed: readonly BoardTaskStatus[], started: boolean): BuildEntry<BoardEntryDetails> {
  const tasks = filed.map(task => ({ id: plainLabel(task.id), title: plainLabel(task.title), depends: task.depends.map(plainLabel) }));
  const list = tasks.map(task => `${task.id} ${task.title}` + (task.depends.length ? ` (after ${task.depends.join(", ")})` : "")).join("; ");
  const count = `${tasks.length} board ${tasks.length === 1 ? "task" : "tasks"}`;
  return {
    customType: BUILD_BOARD_ENTRY, display: true, details: { filed: tasks, started },
    content: `The owner filed ${count}${started ? " and queued them; they start as slots free" : " in Backlog; the owner starts them from the board"}`
      + (list ? `: ${list}.` : "."),
  };
}

export function finishEntry(id: string): BuildEntry<FinishEntryDetails> {
  return {
    customType: BUILD_FINISH_ENTRY, display: true, details: { id },
    content: "The owner opened the exit choice for this build branch. The owner chooses the exit; do not land or merge the branch yourself.",
  };
}

/** `who` muted then `text`, wrapped on the user message surface at most BUBBLE_MAX cells wide with one cell of padding each side. */
function bubble(who: string, text: string, width: number): string[] {
  const size = Math.min(BUBBLE_MAX, width);
  if (size < 3) return [cardLine(PANEL_MUTED + who + " " + PANEL_TEXT + text, width)];
  return wrapTextWithAnsi(PANEL_MUTED + who + " " + PANEL_TEXT + text, size - 2)
    .map(row => PANEL_BUBBLE + PANEL_TEXT + " " + onSurface(cardCell(row, size - 2), PANEL_BUBBLE) + " " + PANEL_BG);
}
function exitEntryLines(details: ExitEntryDetails, width: number): string[] {
  // A stored entry may name a choice or a phase this build does not know; its own words are shown then.
  const label = details.label || (EXIT_LABELS[details.choice] ?? details.choice);
  // A stored entry is drawn as it was written: only steps of the shape this build knows are drawn.
  const land = details.land && Array.isArray(details.land.steps) && typeof details.land.base === "string" ? details.land : undefined;
  // The steps name the gate's log, so the outcome under them does not repeat its path.
  const log = typeof land?.gate?.log === "string" ? `; log: ${plainLabel(land.gate.log)}` : undefined;
  const detail = log ? details.detail?.replace(log, "") : details.detail;
  const shown = exitProgress({ phase: details.phase, ...(detail ? { detail } : {}), record: details });
  const candidate = details.candidate && typeof details.candidate.state === "string" ? details.candidate : undefined;
  return [
    ...bubble("you chose", plainLabel(label), width),
    ...(land ? landLines(land, width, undefined, undefined, candidate) : []),
    // A Land that ended with its candidate held back (the base moved, say) shows why, apart from a failed step.
    ...(candidate && candidate.state !== "ready" && details.phase !== "landed" ? candidateLines(candidate, width) : []),
    ...(shown ? exitLines(shown, width) : cardWrap(PANEL_MUTED + "· " + outcome(details), "", width)),
    ...(details.choice === "land" && details.phase === "failed" ? cardWrap("/finish to choose an exit again", PANEL_MUTED, width, 2) : []),
    ...(details.overrideReason ? cardWrap(PANEL_MUTED + "landed over open findings: " + PANEL_TEXT + details.overrideReason, "", width, 2) : []),
  ];
}
function boardLines(details: BoardEntryDetails, width: number): string[] {
  const ids = details.filed.map(task => task.id), count = `${ids.length} ${ids.length === 1 ? "task" : "tasks"}`;
  return [
    ...bubble("you chose", details.started ? "File and start" : "File only", width),
    ...cardWrap(`✓ Filed ${count}` + (ids.length ? ": " + ids.join(", ") : "")
      + (details.started ? " · queued, they start as slots free" : " · in Backlog"), PANEL_SUCCESS, width),
  ];
}
/** Renders exit and board entries as a `you chose …` bubble plus an outcome line; finish entries as the live result card. */
export function buildEntryRenderer(source: () => BuildCardSource | undefined): MessageRenderer {
  return (message, options) => {
    const details = typeof message.details === "object" && message.details !== null ? message.details as Record<string, unknown> : undefined;
    if (!details) return undefined;
    // Pi hands a custom renderer the full row; the inset matches the output padding of tool cards and messages.
    const inset = Math.max(0, options.outputPad);
    switch (message.customType) {
      case BUILD_EXIT_ENTRY:
        if (typeof details["choice"] !== "string" || typeof details["phase"] !== "string") return undefined;
        return new BuildCard(width => ({ lines: exitEntryLines(details as unknown as ExitEntryDetails, width) }), undefined, inset);
      case BUILD_BOARD_ENTRY:
        if (!Array.isArray(details["filed"])) return undefined;
        return new BuildCard(width => ({ lines: boardLines(details as unknown as BoardEntryDetails, width) }), undefined, inset);
      case BUILD_FINISH_ENTRY: {
        const id = details["id"];
        return typeof id === "string" ? resultCard(source, id, undefined, inset) : undefined;
      }
      default:
        return undefined;
    }
  };
}
