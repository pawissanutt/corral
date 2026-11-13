// Conversation entries a build session posts for the owner's decisions: the exit outcome, the board filing and
// an owner-opened finish card. Their content reaches the parent model (custom messages become user content), so
// it states the decision and its outcome in plain sentences. The renderer draws exit and board entries as a
// `you chose …` row plus an outcome row, and finish entries as the live result card.
import type { MessageRenderer } from "@earendil-works/pi-coding-agent";
import type { Component, TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { BoardTaskStatus, ExitChoice, ExitOption, ExitPhase, ExitState } from "../../build/types.ts";
import { toneColor } from "../agents-model.ts";
import { plainLabel, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT } from "../panels-view.ts";
import type { BuildCardSource } from "./contract.ts";
import { EXIT_LABELS, exitProgress, resultCardLines } from "./result-card.ts";
import { cardLine, cardWrap } from "./task-card.ts";

export const BUILD_EXIT_ENTRY = "corral_build_exit";
export const BUILD_BOARD_ENTRY = "corral_build_board";
export const BUILD_FINISH_ENTRY = "corral_build_finish";

export interface BuildEntry<T> { readonly customType: string; readonly content: string; readonly display: true; readonly details: T }
export interface ExitEntryDetails { readonly choice: ExitChoice; readonly label: string; readonly phase: ExitPhase; readonly detail?: string; readonly task?: string }
export interface BoardEntryDetails { readonly filed: readonly { readonly id: string; readonly title: string; readonly depends: readonly string[] }[]; readonly started: boolean }
export interface FinishEntryDetails { readonly id: string }

/**
 * A card drawn from live data on every render. `draw` returns the lines and the rows a click acts on; a plain
 * left click on such a row calls `act` with its hit. `inset` cells are kept free on both sides, as Pi's tool box
 * does with its output padding; conversation entries get the full row and pass that padding here.
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

type ResultHit = { readonly child?: string; readonly exit?: true };
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
    const cards = source(), state = cards?.state(), live = finishLive(cards, id);
    return resultCardLines({
      summary: live ? cards?.summary() : undefined, ...(note === undefined ? {} : { note }),
      fleet: state?.fleet, exit: state?.exit ?? { phase: "none" }, options: [], live,
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
function outcome(details: Pick<ExitEntryDetails, "choice" | "phase" | "detail" | "task">): string {
  if (details.detail) return sentence(details.detail);
  switch (details.phase) {
    case "kept": return "The branch stays in its worktree; nothing was filed or merged.";
    case "shipped": return details.task ? `${details.task} is on the board for the gate and independent review.` : "The branch was handed to the board.";
    case "landed": return details.task ? `${details.task} landed.` : "The branch landed.";
    case "failed": return "The exit failed.";
    case "running": case "landing": return "The exit is still running.";
    default: return details.choice === "iterate" ? "The build session continues; nothing was filed or landed." : "No exit ran.";
  }
}

/** Content is model-visible (custom messages convert to user content): `The owner chose "Land after the gate" for this build branch. T-962 landed at 3f2a9c1.` */
export function exitEntry(option: ExitOption, state: ExitState): BuildEntry<ExitEntryDetails> {
  const label = plainLabel(option.label), task = state.record?.task, detail = state.detail ? plainLabel(state.detail) : undefined;
  const details: ExitEntryDetails = {
    choice: option.choice, label, phase: state.phase, ...(detail ? { detail } : {}), ...(task ? { task } : {}),
  };
  return {
    customType: BUILD_EXIT_ENTRY, display: true, details,
    content: `The owner chose "${label}" for this build branch. ${outcome(details)}`,
  };
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
    content: "The owner opened the exit choice for this build branch. The owner chooses the exit; do not land, submit or ship the branch yourself.",
  };
}

function exitLines(details: ExitEntryDetails, width: number): string[] {
  // A stored entry may name a choice this build does not know; its own words are shown then.
  const label = details.label || (EXIT_LABELS[details.choice] ?? details.choice);
  const lines = [cardLine(PANEL_MUTED + "you chose " + PANEL_TEXT + plainLabel(label), width)];
  const shown = exitProgress(details);
  const text = shown ? toneColor(shown.tone) + shown.glyph + " " + shown.text : PANEL_MUTED + "· " + outcome(details);
  return [...lines, ...cardWrap(text, "", width)];
}
function boardLines(details: BoardEntryDetails, width: number): string[] {
  const ids = details.filed.map(task => task.id), count = `${ids.length} ${ids.length === 1 ? "task" : "tasks"}`;
  return [
    cardLine(PANEL_MUTED + "you chose " + PANEL_TEXT + (details.started ? "File and start" : "File only"), width),
    ...cardWrap(PANEL_SUCCESS + "✓ " + PANEL_TEXT + `Filed ${count}` + (ids.length ? ": " + ids.join(", ") : "")
      + PANEL_MUTED + (details.started ? " · queued, they start as slots free" : " · in Backlog"), "", width),
  ];
}
/** Renders exit and board entries as one `you chose …` row plus one outcome row; finish entries as the live result card. */
export function buildEntryRenderer(source: () => BuildCardSource | undefined): MessageRenderer {
  return (message, options) => {
    const details = typeof message.details === "object" && message.details !== null ? message.details as Record<string, unknown> : undefined;
    if (!details) return undefined;
    // Pi hands a custom renderer the full row; the inset matches the output padding of tool cards and messages.
    const inset = Math.max(0, options.outputPad);
    switch (message.customType) {
      case BUILD_EXIT_ENTRY:
        if (typeof details["choice"] !== "string" || typeof details["phase"] !== "string") return undefined;
        return new BuildCard(width => ({ lines: exitLines(details as unknown as ExitEntryDetails, width) }), undefined, inset);
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
