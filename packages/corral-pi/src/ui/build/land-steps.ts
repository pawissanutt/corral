// Land as the owner follows it: one row per step (rebase, gate, fast-forward) with its state, a short fact and its
// time; under the gate step, while it runs and once it failed, the last lines of its log and where the log is.
// Before that, the candidate the host prepared: the exact base commit and head Land would fast-forward between, or
// what holds it back (conflicts, a missing review or gate, a failed gate, a base that moved). A host that prepared
// none reports none, and nothing here claims one.
import { homedir } from "node:os";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { ExitState, LandCandidate, LandGate, LandProgress, LandStep } from "../../build/types.ts";
import { duration } from "../agents-model.ts";
import { spinner } from "../frame.ts";
import { plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";
import { cardLine, cardWrap } from "./task-card.ts";

/** Log rows and the log's path sit this far in, under their step. */
const LOG_INDENT = 2;
/** Conflicted paths listed under a candidate; the rest are counted. */
const CONFLICTS_SHOWN = 5;

const short = (sha: string) => plainLabel(sha.slice(0, 7));

/** `main 1a2b3c4 → 5d6e7f8`: the fast-forward `candidate` stands for; undefined without both commits. */
export function fastForward(candidate: LandCandidate | undefined): string | undefined {
  if (!candidate?.baseTip || !candidate.head) return undefined;
  return `${plainLabel(candidate.base ?? "the base branch")} ${short(candidate.baseTip)} → ${short(candidate.head)}`;
}

/** The candidate's glyph, colour and words, then its facts; a running preparation spins on `now`. */
function candidateHead(candidate: LandCandidate, now: number | undefined): { readonly look: string; readonly text: string; readonly facts: string[] } {
  const base = plainLabel(candidate.base ?? "the base branch"), head = candidate.head ? short(candidate.head) : undefined;
  const on = head ? ` ${head}` : "", gate = candidate.gateAttempt ? plainLabel(candidate.gateAttempt) : undefined;
  const move = fastForward(candidate);
  switch (candidate.state) {
    case "preparing": return { look: PANEL_BLUE + (now === undefined ? "●" : spinner(now)), text: `Preparing the Land candidate on ${base}`, facts: [] };
    case "ready": return { look: PANEL_SUCCESS + "✓", text: move ? `Ready: Land fast-forwards ${move}` : `Candidate${on} ready on ${base}`,
      facts: gate ? [`gate ${gate} passed`] : [] };
    case "conflicts": return { look: PANEL_ERROR + "✗", text: `Rebasing onto ${base} conflicts; the branch is unchanged`,
      facts: candidate.conflicts?.length ? [`${candidate.conflicts.length} ${candidate.conflicts.length === 1 ? "path" : "paths"}`] : [] };
    case "needs_review": return { look: PANEL_WARNING + "▲", text: `Candidate${on} on ${base} needs an approving review`, facts: move ? [move] : [] };
    case "needs_gate": return { look: PANEL_WARNING + "▲", text: `Candidate${on} on ${base} has no passing gate`, facts: move ? [move] : [] };
    case "gate_failed": return { look: PANEL_ERROR + "✗", text: `Gate${gate ? ` ${gate}` : ""} failed on candidate${on}`, facts: [] };
    case "base_moved": return { look: PANEL_WARNING + "▲",
      text: `${base} moved${candidate.movedTo ? ` to ${short(candidate.movedTo)}` : ""} since the candidate was prepared`
        + (candidate.baseTip ? ` on ${short(candidate.baseTip)}` : ""), facts: [] };
    case "failed": return { look: PANEL_ERROR + "✗", text: "Preparing the Land candidate failed", facts: [] };
    default: return { look: PANEL_MUTED + "·", text: `Land candidate${on} on ${base}`, facts: [] };
  }
}

/**
 * The candidate at `width` cells: its state line first (it alone stands when one row is left), then the conflicted
 * paths and the host's reason. A ready candidate needs no reason. `now` spins a preparation; without it nothing moves.
 */
export function candidateLines(candidate: LandCandidate, width: number, now?: number): string[] {
  const { look, text, facts } = candidateHead(candidate, now);
  const lines = cardWrap(look + " " + PANEL_TEXT + text + (facts.length ? PANEL_MUTED + " · " + facts.map(plainLabel).join(" · ") : ""), "", width);
  const conflicts = candidate.state === "conflicts" ? candidate.conflicts ?? [] : [];
  for (const path of conflicts.slice(0, CONFLICTS_SHOWN)) lines.push(cardLine(" ".repeat(LOG_INDENT) + PANEL_MUTED + "│ " + PANEL_TEXT + plainLabel(path), width));
  if (conflicts.length > CONFLICTS_SHOWN) lines.push(cardLine(" ".repeat(LOG_INDENT) + PANEL_MUTED + `│ and ${conflicts.length - CONFLICTS_SHOWN} more`, width));
  if (candidate.detail && candidate.state !== "ready") lines.push(...cardWrap(plainLabel(candidate.detail), PANEL_MUTED, width, LOG_INDENT));
  return lines;
}

function label(step: LandStep, base: string, candidate: LandCandidate | undefined): string {
  switch (step.id) {
    case "rebase": return `Rebase onto ${base}`;
    case "gate": return "Gate";
    // The fast-forward names the commits it moves between, when the host prepared them.
    default: {
      const move = fastForward(candidate);
      return move ? `Fast-forward ${move}` : `Fast-forward ${base}`;
    }
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
 * nothing moves and no running time is shown. `repair` adds the repair round under the failed gate; `candidate`, the
 * one the host prepared, names the commits the fast-forward moves between.
 */
export function landLines(land: LandProgress, width: number, now?: number, repair?: ExitState["repair"], candidate?: LandCandidate): string[] {
  const base = plainLabel(land.base), lines: string[] = [];
  for (const step of land.steps) {
    const look = step.state === "done" ? PANEL_SUCCESS + "✓" : step.state === "failed" ? PANEL_ERROR + "✗"
      : step.state === "running" ? PANEL_BLUE + (now === undefined ? "●" : spinner(now)) : PANEL_MUTED + "○";
    const active = step.state === "running" || step.state === "failed";
    const facts = [step.note ? plainLabel(step.note) : undefined, step.id === "gate" && active ? commandOf(land.gate) : undefined,
      elapsed(step, now)].filter(Boolean);
    lines.push(cardLine(look + " " + (step.state === "waiting" ? PANEL_MUTED : PANEL_TEXT) + label(step, base, candidate)
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
