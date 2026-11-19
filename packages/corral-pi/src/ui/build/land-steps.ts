// Land as the owner follows it: one row per step (rebase, gate, fast-forward) with its state, a short fact and its
// time; under the gate step, while it runs and once it failed, the last lines of its log and where the log is.
import { homedir } from "node:os";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { ExitState, LandGate, LandProgress, LandStep } from "../../build/types.ts";
import { duration } from "../agents-model.ts";
import { spinner } from "../frame.ts";
import { plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT } from "../panels-view.ts";
import { cardLine, cardWrap } from "./task-card.ts";

/** Log rows and the log's path sit this far in, under their step. */
const LOG_INDENT = 2;

function label(step: LandStep, base: string): string {
  switch (step.id) {
    case "rebase": return `Rebase onto ${base}`;
    case "gate": return "Gate";
    default: return `Fast-forward ${base}`;
  }
}
/** `scripts/gate.sh`, with `(2/3)` when the gate has several commands. */
function commandOf(gate: LandGate | undefined): string | undefined {
  if (!gate?.command) return undefined;
  const place = gate.index !== undefined && gate.of !== undefined && gate.of > 1 ? ` (${gate.index}/${gate.of})` : "";
  return plainLabel(gate.command) + place;
}
function elapsed(step: LandStep, now: number | undefined): string | undefined {
  if (step.startedAt === undefined) return undefined;
  const end = step.state === "running" ? now : step.endedAt;
  // A step that took under a second says nothing about time.
  return end === undefined || (step.state !== "running" && end - step.startedAt < 1000) ? undefined : duration(end - step.startedAt);
}
/** The path with the home directory as `~`, in at most `room` cells: a longer one keeps its end, which names the file. */
function shortPath(path: string, room: number): string {
  const home = homedir();
  const text = plainLabel(home && path.startsWith(home + "/") ? "~" + path.slice(home.length) : path);
  if (visibleWidth(text) <= room) return text;
  const kept = [...text];
  while (kept.length && visibleWidth(kept.join("")) > room - 1) kept.shift();
  return "…" + kept.join("");
}

/**
 * The rows of `land` at `width` cells. A running step spins on `now`, the shared clock; without it (a stored entry)
 * nothing moves and no running time is shown. `repair` adds the repair round under the failed gate.
 */
export function landLines(land: LandProgress, width: number, now?: number, repair?: ExitState["repair"]): string[] {
  const base = plainLabel(land.base), lines: string[] = [];
  for (const step of land.steps) {
    const look = step.state === "done" ? PANEL_SUCCESS + "✓" : step.state === "failed" ? PANEL_ERROR + "✗"
      : step.state === "running" ? PANEL_BLUE + (now === undefined ? "●" : spinner(now)) : PANEL_MUTED + "○";
    const active = step.state === "running" || step.state === "failed";
    const facts = [step.note ? plainLabel(step.note) : undefined, step.id === "gate" && active ? commandOf(land.gate) : undefined,
      elapsed(step, now)].filter(Boolean);
    lines.push(cardLine(look + " " + (step.state === "waiting" ? PANEL_MUTED : PANEL_TEXT) + label(step, base)
      + (facts.length ? PANEL_MUTED + " · " + facts.join(" · ") : ""), width));
    if (step.id !== "gate" || !active || !land.gate) continue;
    for (const row of land.gate.tail ?? []) lines.push(cardLine(" ".repeat(LOG_INDENT) + PANEL_MUTED + "│ " + plainLabel(row), width));
    if (land.gate.log) lines.push(cardLine(" ".repeat(LOG_INDENT) + PANEL_MUTED + "log " + shortPath(land.gate.log, width - LOG_INDENT - 4), width));
    if (repair && step.state === "failed") {
      lines.push(...cardWrap(PANEL_BLUE + (now === undefined ? "●" : spinner(now)) + PANEL_TEXT + ` Repair ${repair.round} of ${repair.of}`
        + PANEL_MUTED + " · the session is fixing it; Land runs again when its turn ends", "", width));
    }
  }
  return lines;
}
