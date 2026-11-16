// The build result card: a rounded card headed `Build finished · d/n slices · time`, then one row per slice (name,
// state and time, cost), the branch figures, edits outside a child's files and uncommitted changes in warning, the
// session's gate attempts and review rounds from its build record, and the parent's own note muted. On the latest
// exit card only, a running exit shows the owner's choice and its live progress; older exit cards say they were
// superseded. A pending exit choice is drawn by the header and the dock, not by the card; the card's header row opens it.
import type { BuildRecord, RecordReview } from "../../build/record-types.ts";
import type { BuildSummary, ChildState, ExitChoice, ExitOption, ExitRecord, ExitState, FleetSnapshot } from "../../build/types.ts";
import { agentStatus, duration, money, toneColor } from "../agents-model.ts";
import type { Tone } from "../agents-model.ts";
import { spinner } from "../frame.ts";
import { plainLabel, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";
import { childHue } from "./contract.ts";
import { BOLD, CARD_BODY, cardCell, cardInner, cardLine, cardWrap, conversationCard, UNBOLD } from "./task-card.ts";

/** The mock's `.sres` columns: name (11), state and time (9), cost, two cells apart. */
const GAP = "  ", NAME = 11, STATE = 9;
/** Below this many cells inside the card a flag or a finding keeps one row and ellipsizes instead of wrapping. */
const WRAP_FLAGS = 60;
/** Gate rows: attempt (4), tier (5), result (9), the failed command, the duration (7, right-aligned). */
const ATTEMPT = 4, TIER = 5, RESULT = 9, TIME = 7;
/** Below this many cells the failed command leaves the gate row for a row of its own. */
const COMMAND_MIN = 8;
/** Findings and the resolved count sit this far in, under their round. */
const FINDING_INDENT = 4;

/** The host's labels, used when the card has no option list to read them from. */
export const EXIT_LABELS: Readonly<Record<ExitChoice, string>> = {
  keep: "Keep on the branch", land: "Land on the base branch", iterate: "Keep iterating",
};

interface Slice { readonly id: string; readonly state: ChildState; readonly seconds?: number; readonly cost?: number }
export interface ResultHit { readonly child?: string; readonly exit?: true }
export interface ExitShown { readonly glyph: string; readonly tone: Tone; readonly text: string; readonly facts?: string }
type Landed = Pick<ExitRecord, "landed" | "baseBranch" | "gateAttempt">;

function slicesOf(summary: BuildSummary | undefined, fleet: FleetSnapshot | undefined, now: number): Slice[] {
  if (summary) return [...summary.slices];
  return (fleet?.children ?? []).map(child => ({
    id: child.id, state: child.state,
    ...(child.startedAt === undefined ? {} : { seconds: Math.max(0, ((child.endedAt ?? now) - child.startedAt) / 1000) }),
    ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }),
  }));
}
/** From the first child's creation to the last one's end, or to now while one is live. */
function elapsed(fleet: FleetSnapshot | undefined, now: number): string | undefined {
  if (!fleet?.children.length) return undefined;
  const start = fleet.startedAt ?? Math.min(...fleet.children.map(child => child.createdAt));
  const live = fleet.children.some(child => child.endedAt === undefined);
  const end = live ? now : Math.max(...fleet.children.map(child => child.endedAt ?? start));
  return duration(end - start);
}
export function shortSha(sha: string): string { return plainLabel(sha.slice(0, 7)); }
/** `3f2a9c1 on main · gate G3` for a landed exit; undefined before the base branch moved. */
export function landedFacts(record: Landed | undefined): string | undefined {
  if (!record?.landed) return undefined;
  const where = shortSha(record.landed) + (record.baseBranch ? ` on ${plainLabel(record.baseBranch)}` : "");
  return record.gateAttempt ? `${where} · gate ${plainLabel(record.gateAttempt)}` : where;
}
/** The glyph, tone and words of an exit phase after a choice; the host's detail wins over the default words. */
export function exitProgress(exit: Pick<ExitState, "phase" | "detail"> & { readonly record?: Landed }): ExitShown | undefined {
  const detail = exit.detail ? plainLabel(exit.detail) : undefined;
  switch (exit.phase) {
    case "rebasing": return { glyph: "●", tone: "accent", text: detail ?? "Rebasing onto the base branch." };
    case "verifying": return { glyph: "●", tone: "accent", text: detail ?? "Running the full gate on the rebased head." };
    case "landing": return { glyph: "●", tone: "accent", text: detail ?? "Fast-forwarding the base branch." };
    case "kept": return { glyph: "✓", tone: "success", text: detail ?? "Kept on the branch." };
    case "landed": {
      const facts = landedFacts(exit.record);
      return { glyph: "✓", tone: "success", text: detail ?? "Landed.", ...(facts ? { facts } : {}) };
    }
    case "failed": return { glyph: "✗", tone: "error", text: detail ?? "The exit failed." };
    default: return detail ? { glyph: "·", tone: "muted", text: detail } : undefined;
  }
}
/** The exit's glyph and words, then its facts muted, wrapped to `width`; a running phase spins on the shared clock. */
export function exitLines(shown: ExitShown, width: number, now?: number): string[] {
  const glyph = shown.tone === "accent" && now !== undefined ? spinner(now) : shown.glyph;
  return cardWrap(toneColor(shown.tone) + glyph + " " + shown.text + (shown.facts ? PANEL_MUTED + " · " + shown.facts : ""), "", width);
}

/** One wrapped line above WRAP_FLAGS cells, else one clipped row; `indent` cells in. */
function fold(text: string, inner: number, indent = 0): string[] {
  return inner >= WRAP_FLAGS ? cardWrap(text, "", inner, indent) : [cardLine(" ".repeat(indent) + text, inner)];
}

/** `verification`, then one row per gate attempt: attempt, tier, result in its tone, the failed command, the duration. */
function verificationLines(verification: BuildRecord["verification"], inner: number): string[] {
  if (!verification.length) return [];
  const lines = [cardLine(PANEL_MUTED + "verification", inner)];
  const command = inner - (ATTEMPT + TIER + RESULT + TIME + GAP.length * 4);
  for (const gate of verification) {
    const look = gate.result === "pass" ? { glyph: "✓", tone: PANEL_SUCCESS, word: "pass" }
      : gate.result === "fail" ? { glyph: "✗", tone: PANEL_ERROR, word: "fail" } : { glyph: "○", tone: PANEL_MUTED, word: "skipped" };
    const failed = gate.result === "fail" && gate.failed_command ? plainLabel(gate.failed_command) : "";
    const ms = Date.parse(gate.ended) - Date.parse(gate.started);
    const time = cardCell(PANEL_MUTED + (Number.isFinite(ms) ? duration(ms) : ""), TIME, true);
    const head = cardCell(PANEL_TEXT + plainLabel(gate.attempt), ATTEMPT) + GAP + cardCell(PANEL_MUTED + plainLabel(gate.tier), TIER) + GAP
      + cardCell(look.tone + look.glyph + " " + look.word, RESULT) + GAP;
    if (command >= COMMAND_MIN) {
      lines.push(cardLine(head + cardCell(PANEL_TEXT + failed, command) + GAP + time, inner));
      continue;
    }
    lines.push(cardLine(head + time, inner));
    if (failed) lines.push(cardLine(" ".repeat(ATTEMPT + GAP.length) + PANEL_TEXT + failed, inner));
  }
  return lines;
}

const SEVERITIES = ["blocker", "major", "minor"] as const;
const SEVERITY_TONE: Readonly<Record<(typeof SEVERITIES)[number], () => string>> = {
  blocker: () => PANEL_ERROR, major: () => PANEL_WARNING, minor: () => PANEL_MUTED,
};
/**
 * `review`, then per round its verdict, the findings counted by severity and the reviewer's model; under it each
 * unresolved finding (severity in its tone, `path:line`, the text) and the resolved ones as one muted count.
 */
function reviewLines(reviews: readonly RecordReview[], inner: number): string[] {
  if (!reviews.length) return [];
  const lines = [cardLine(PANEL_MUTED + "review", inner)];
  for (const review of reviews) {
    const counts = SEVERITIES.flatMap(severity => {
      const count = review.findings.filter(finding => finding.severity === severity).length;
      return count ? [`${count} ${severity}`] : [];
    });
    const verdict = review.verdict === "approve" ? PANEL_SUCCESS + "✓ approve" : PANEL_ERROR + "✗ changes";
    const model = plainLabel(review.model.slice(review.model.lastIndexOf("/") + 1));
    lines.push(...fold(PANEL_TEXT + `R${review.round}` + GAP + verdict + PANEL_TEXT + " · " + (counts.length ? counts.join(", ") : "no findings")
      + PANEL_MUTED + " · " + model, inner));
    const open = review.findings.filter(finding => finding.resolution === undefined || finding.resolution.state === "open");
    const resolved = review.findings.length - open.length;
    for (const finding of open) {
      const where = finding.path ? plainLabel(finding.path) + (finding.line === undefined ? "" : `:${finding.line}`) : "";
      lines.push(...fold(SEVERITY_TONE[finding.severity]() + finding.severity + GAP + (where ? PANEL_TEXT + where + GAP : "")
        + PANEL_TEXT + plainLabel(finding.text), inner, FINDING_INDENT));
    }
    if (resolved) lines.push(cardLine(" ".repeat(FINDING_INDENT) + PANEL_MUTED + `${resolved} resolved`, inner));
  }
  return lines;
}

/**
 * The result card for a row `width` cells wide; `hits` maps a line of the card to a slice, or the header to the pending
 * exit. `record` supplies the gate attempts and review rounds; `exitSince` is when the exit's current phase began.
 */
export function resultCardLines(input: {
  readonly summary: BuildSummary | undefined; readonly note?: string; readonly fleet: FleetSnapshot | undefined;
  readonly exit: ExitState; readonly options: readonly ExitOption[]; readonly live: boolean;
  readonly record?: Pick<BuildRecord, "verification" | "reviews">; readonly exitSince?: number;
}, width: number, color: (hue: number) => string, now: number): { readonly lines: string[]; readonly hits: ReadonlyMap<number, ResultHit> } {
  const { summary, fleet, exit } = input;
  const inner = cardInner(width), body: string[] = [], hits = new Map<number, ResultHit>();
  const slices = slicesOf(summary, fleet, now), done = slices.filter(slice => slice.state === "done").length;
  const time = elapsed(fleet, now);
  const title = slices.length ? `Build finished · ${done}/${slices.length} slices` + (time ? ` · ${time}` : "") : "Build finished";
  const header = BOLD + PANEL_TEXT + title + UNBOLD;
  if (input.live && exit.phase === "pending") hits.set(CARD_BODY - 1, { exit: true });

  slices.forEach((slice, index) => {
    const hue = fleet?.children.some(child => child.id === slice.id) ? childHue(fleet, slice.id) : index;
    const status = agentStatus(slice.state, slice.seconds === undefined ? undefined : duration(slice.seconds * 1000));
    const glyph = slice.state === "running" ? spinner(now, index) : status.glyph;
    const state = slice.state === "done" && status.time ? `${glyph} ${status.time}` : `${glyph} ${status.word}`;
    hits.set(CARD_BODY + body.length, { child: slice.id });
    body.push(cardCell(BOLD + color(hue) + plainLabel(slice.id) + UNBOLD, NAME) + GAP
      + cardCell(toneColor(status.tone) + state, STATE) + GAP + PANEL_TEXT + money(slice.cost));
  });

  const flag = (text: string) => body.push(...fold(PANEL_WARNING + text, inner));
  if (summary) {
    const files = `${summary.files} ${summary.files === 1 ? "file" : "files"} `, commits = `${summary.commits} ${summary.commits === 1 ? "commit" : "commits"}`;
    const branch = summary.branch ? ` on ${plainLabel(summary.branch)}` : "";
    body.push(...cardWrap(PANEL_TEXT + files + PANEL_SUCCESS + `+${summary.added}` + PANEL_TEXT + " " + PANEL_ERROR + `−${summary.removed}`
      + PANEL_TEXT + branch + ", " + commits, "", inner));
    for (const entry of summary.outside) {
      const by = entry.by.map(plainLabel);
      flag(`! ${plainLabel(entry.path)} was also edited by ${by.join(", ")}, outside ${by.length === 1 ? "its" : "their"} files`);
    }
    if (summary.dirty) flag("! Uncommitted changes in the worktree");
  }
  // The record is the session's as it stands now, so only the live card shows it.
  if (input.live && input.record) body.push(...verificationLines(input.record.verification, inner), ...reviewLines(input.record.reviews, inner));
  const note = input.note?.trim();
  if (note) for (const paragraph of note.split(/\r?\n/).map(plainLabel).filter(Boolean)) body.push(...cardWrap(paragraph, PANEL_MUTED, inner));

  if (!input.live) {
    body.push(cardLine(PANEL_MUTED + "superseded by a later exit choice", inner));
    return { lines: conversationCard(header, body, width), hits };
  }
  const choice = exit.record?.choice;
  // A settled exit with a record is posted as its own conversation entry (`you chose …` plus the outcome), so the
  // card leaves both to it. The steps of `land`, and a failure without a record, stay on the card.
  const settled = choice !== undefined && (exit.phase === "kept" || exit.phase === "landed" || exit.phase === "failed"
    || (exit.phase === "none" && choice === "iterate"));
  if (!settled && exit.phase !== "pending") {
    if (choice) {
      const label = input.options.find(option => option.choice === choice)?.label ?? EXIT_LABELS[choice];
      body.push(cardLine(PANEL_MUTED + "you chose " + PANEL_TEXT + plainLabel(label), inner));
    }
    const progress = choice || exit.phase !== "none" ? exitProgress(exit) : undefined;
    // While verifying, the card names the full gate and how long it has run.
    const shown = progress && exit.phase === "verifying"
      ? { ...progress, facts: "full gate" + (input.exitSince === undefined ? "" : ` · ${duration(now - input.exitSince)}`) } : progress;
    if (shown) body.push(...exitLines(shown, inner, now));
  }
  return { lines: conversationCard(header, body, width), hits };
}
