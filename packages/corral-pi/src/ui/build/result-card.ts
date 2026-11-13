// The build result card: what the session built (slices, branch figures, uncommitted changes, edits outside a
// child's files, the parent's own note) and, on the latest exit card only, the owner's choice and its live
// progress. Older exit cards say they were superseded. Lines use foreground colours only, like the task card.
import type { BuildSummary, ChildState, ExitChoice, ExitOption, ExitState, FleetSnapshot } from "../../build/types.ts";
import { agentStatus, duration, money, toneColor, type Tone } from "../agents-model.ts";
import { plainLabel, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";
import { childHue } from "./contract.ts";
import { BOLD, cardCell, cardLine, cardWrap, UNBOLD } from "./task-card.ts";

const GAP = "  ", NAME = 11, STATE = 9;
/** Below this width a flag keeps one row and ellipsizes instead of wrapping. */
const WRAP_FLAGS = 64;

/** The host's labels (`build/exit.ts`), used when the card has no option list to read them from. */
export const EXIT_LABELS: Readonly<Record<ExitChoice, string>> = {
  keep: "Keep on the branch", land: "Land after the gate", ship: "Ship through the board", iterate: "Keep iterating",
};

interface Slice { readonly id: string; readonly state: ChildState; readonly seconds?: number; readonly cost?: number }

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
/** The glyph, tone and words of an exit phase after a choice; the host's detail wins over the default words. */
export function exitProgress(exit: Pick<ExitState, "phase" | "detail">): { readonly glyph: string; readonly tone: Tone; readonly text: string } | undefined {
  const detail = exit.detail ? plainLabel(exit.detail) : undefined;
  switch (exit.phase) {
    case "running": return { glyph: "●", tone: "accent", text: detail ?? "Running the exit." };
    case "landing": return { glyph: "●", tone: "accent", text: detail ?? "Landing after the gate." };
    case "kept": return { glyph: "✓", tone: "success", text: detail ?? "Kept on the branch." };
    case "shipped": return { glyph: "✓", tone: "success", text: detail ?? "Handed to the board." };
    case "landed": return { glyph: "✓", tone: "success", text: detail ?? "Landed." };
    case "failed": return { glyph: "✗", tone: "error", text: detail ?? "The exit failed." };
    default: return detail ? { glyph: "·", tone: "muted", text: detail } : undefined;
  }
}

export function resultCardLines(input: {
  readonly summary: BuildSummary | undefined; readonly note?: string; readonly fleet: FleetSnapshot | undefined;
  readonly exit: ExitState; readonly options: readonly ExitOption[]; readonly live: boolean;
}, width: number, color: (hue: number) => string, now: number):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, { readonly child?: string; readonly exit?: true }> } {
  const { summary, fleet, exit } = input;
  const lines: string[] = [], hits = new Map<number, { readonly child?: string; readonly exit?: true }>();
  const slices = slicesOf(summary, fleet, now), done = slices.filter(slice => slice.state === "done").length;
  const time = elapsed(fleet, now);
  const title = slices.length ? `Build finished · ${done}/${slices.length} slices` + (time ? ` · ${time}` : "") : "Build finished";
  lines.push(cardLine(BOLD + PANEL_TEXT + title + UNBOLD, width));

  slices.forEach((slice, index) => {
    const hue = fleet?.children.some(child => child.id === slice.id) ? childHue(fleet, slice.id) : index;
    const status = agentStatus(slice.state, slice.seconds === undefined ? undefined : duration(slice.seconds * 1000));
    const state = slice.state === "done" && status.time ? `${status.glyph} ${status.time}` : `${status.glyph} ${status.word}`;
    hits.set(lines.length, { child: slice.id });
    lines.push(cardLine(cardCell(BOLD + color(hue) + plainLabel(slice.id) + UNBOLD, NAME) + GAP
      + cardCell(toneColor(status.tone) + state, STATE) + GAP + PANEL_TEXT + money(slice.cost), width));
  });

  const flag = (text: string) => {
    if (width >= WRAP_FLAGS) lines.push(...cardWrap(text, PANEL_WARNING, width));
    else lines.push(cardLine(PANEL_WARNING + text, width));
  };
  if (summary) {
    const files = `${summary.files} ${summary.files === 1 ? "file" : "files"} `, commits = `${summary.commits} ${summary.commits === 1 ? "commit" : "commits"}`;
    const branch = summary.branch ? PANEL_TEXT + ` on ${plainLabel(summary.branch)}` : "";
    lines.push(...cardWrap(PANEL_TEXT + files + PANEL_SUCCESS + `+${summary.added}` + PANEL_TEXT + " " + PANEL_ERROR + `−${summary.removed}`
      + branch + PANEL_MUTED + " · " + PANEL_TEXT + commits, "", width));
    if (summary.dirty) flag("! Uncommitted changes in the worktree");
    for (const entry of summary.outside) {
      const by = entry.by.map(plainLabel);
      flag(`! ${plainLabel(entry.path)} was also edited by ${by.join(", ")}, outside ${by.length === 1 ? "its" : "their"} files`);
    }
  } else if (input.live) {
    lines.push(cardLine(PANEL_MUTED + "Reading the branch…", width));
  }
  const note = input.note?.trim();
  if (note) for (const paragraph of note.split(/\r?\n/).map(plainLabel).filter(Boolean)) lines.push(...cardWrap(paragraph, PANEL_MUTED, width));

  if (!input.live) {
    lines.push(cardLine(PANEL_MUTED + "superseded by a later exit choice", width));
    return { lines, hits };
  }
  if (exit.phase === "pending") {
    hits.set(lines.length, { exit: true });
    lines.push(cardLine(PANEL_WARNING + "▲ Exit choice pending", width));
    return { lines, hits };
  }
  const choice = exit.record?.choice;
  // A settled exit with a record is posted as its own conversation entry (`you chose …` plus the outcome), so the
  // card leaves both to it. Running and landing exits, and a failure without a record, stay on the card.
  const settled = choice !== undefined && (exit.phase === "kept" || exit.phase === "shipped" || exit.phase === "landed"
    || exit.phase === "failed" || (exit.phase === "none" && choice === "iterate"));
  if (settled) return { lines, hits };
  if (choice) {
    const label = input.options.find(option => option.choice === choice)?.label ?? EXIT_LABELS[choice];
    lines.push(cardLine(PANEL_MUTED + "you chose " + PANEL_TEXT + plainLabel(label), width));
  }
  const shown = choice || exit.phase !== "none" ? exitProgress(exit) : undefined;
  if (shown) lines.push(...cardWrap(toneColor(shown.tone) + shown.glyph + " " + shown.text, "", width));
  return { lines, hits };
}
