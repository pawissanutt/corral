// Proof (F10) split layout: the task's newest gate and review checks, review rounds, artifacts and the worker's
// summary as a section list beside the selected section's detail. Below SPLIT_MIN columns the list collapses to a
// one-line picker above the same detail. Read-only presentation of `corral show --json`; every read state draws the
// shared chip header first (its state chip and read note), then the shared problem line, then the body.
import { stripTerminalSequences, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { TaskStore, TaskView } from "./task-store.ts";
import { artifactRef } from "./task-store.ts";
import { pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_RULE, RESET } from "./panels-view.ts";
import { splitRow } from "./panels-chrome.ts";
import { gateCheck, proofItems, proofRun, reviewChecks, sectionCount, SECTION_LABEL, short, topChips, type SectionId } from "./proof-parts.ts";
import { ago, bold, chip, chipHeader, field, listRow as paneRow, sectionRule, stateBanner, stateRow, toneColor, wrap } from "./panel-parts.ts";

/** Main-area width at which Proof splits into list + detail; below it a one-line picker is used. */
export const SPLIT_MIN = 105;
export const LIST_WIDTH = 42;

export interface ProofFrame { readonly lines: string[]; readonly hits: Map<number, number>; readonly wide: boolean }

const plainFit = (text: string, width: number) => stripTerminalSequences(truncateToWidth(plainLabel(text), Math.max(0, width), "…"));

const listRow = (left: string, right: string, selected: boolean) => paneRow(left, right, selected, LIST_WIDTH);

/** Why Proof does not apply, completing the shared `Does not apply:` sentence. */
const NOT_APPLICABLE: Record<string, string> = {
  no_reader: "no task reader is attached to this session.",
  no_task: "this session has no task.",
};

/** Chip header, problem line and the reading rules: the rows above the body in every read state. */
function header(store: TaskStore, data: TaskView | undefined, run: string | undefined, width: number, legend: boolean, now: number): string[] {
  const state = store.state;
  const lines = [...chipHeader(data ? topChips(data, run) : [], width, state, now), ...stateBanner(state, "proof", width)];
  if (legend) {
    lines.push(...wrap(`${PANEL_MUTED}A gate result belongs to the commit it ran on; a review belongs to its round. The newest check of each decides. Artifacts are registered files and the summary is the worker's own words, neither is acceptance. ✓ pass or approve · ✗ fail or changes${PANEL_TEXT}`, width));
  }
  return lines;
}

function sectionDetail(data: TaskView, run: string | undefined, id: SectionId, open: boolean, width: number, now: number): string[] {
  const out = [`${PANEL_TEXT}${bold(SECTION_LABEL[id])}  ${chip(sectionCount(data, id))}`];
  const text = (value: string) => out.push(...wrap(PANEL_TEXT + plainLabel(value), width));
  const note = (value: string) => out.push(...wrap(PANEL_MUTED + value + PANEL_TEXT, width));
  const when = (at: string) => ago(Date.parse(at), now) ?? plainLabel(at);
  if (!open) { note("▸ collapsed · Enter expand"); return out; }
  switch (id) {
    case "gate": {
      const gate = gateCheck(data);
      note("The gate's result on one commit, not acceptance");
      if (!gate) break;
      out.push(...field("result", gate.result, width, 8), ...field("commit", short(gate.key), width, 8), ...field("ran", `${when(gate.at)} · by ${gate.by}`, width, 8));
      if (gate.notes) out.push(...field("failed", gate.notes, width, 8));
      if (gate.log) out.push(...field("log", gate.log, width, 8));
      break;
    }
    case "notes": note("The newest round's review notes, not acceptance"); text(reviewChecks(data)[0]?.notes ?? ""); break;
    case "summary": note("Worker assertion, not acceptance"); text(proofRun(data, run)?.summary ?? ""); break;
    case "findings":
      note("Every recorded review round, newest first");
      if (!reviewChecks(data).length) note("No review recorded");
      for (const check of reviewChecks(data)) {
        const [glyph, tone, word] = check.result === "pass" ? ["✓", "ok", "approve"] as const : ["✗", "bad", "changes"] as const;
        out.push(...wrap(`${toneColor(tone)}${glyph} ${word}${PANEL_MUTED}  round ${plainLabel(check.key)} · ${when(check.at)} · by ${check.by}${PANEL_TEXT}`, width));
        if (check.notes) out.push(...wrap(plainLabel(check.notes), width - 2).map(row => `  ${PANEL_TEXT}${row}`));
      }
      break;
    case "artifacts":
      note("Registered files, not proof");
      if (!data.artifacts.length) note("No artifacts registered");
      for (const a of data.artifacts) {
        const meta = `${a.kind ? `${plainLabel(a.kind)} · ` : ""}sha256 ${a.sha256.slice(0, 8)}…`;
        out.push(`${PANEL_BLUE}▤${PANEL_TEXT} ${plainFit(`${artifactRef(a)} ${a.title}`, width - 4 - visibleWidth(meta))}  ${PANEL_MUTED}${plainFit(meta, width - 4)}${PANEL_TEXT}`);
      }
      break;
  }
  return out;
}

/** Below SPLIT_MIN: `‹ PROOF 1/4 › ▸ Gate result   pass`. */
function pickerLine(data: TaskView, items: readonly SectionId[], index: number, open: ReadonlySet<SectionId>, width: number): string {
  const item = items[index];
  if (!item) return `${PANEL_BG}${PANEL_MUTED}${pad(" No checks recorded · no acceptance inferred", width)}${RESET}`;
  const left = `${PANEL_BLUE} ‹ ${PANEL_TEXT}${bold("PROOF")} ${PANEL_MUTED}${index + 1}/${items.length}${PANEL_BLUE} › ${PANEL_TEXT}${open.has(item) ? "▾" : "▸"} ${SECTION_LABEL[item]}`;
  const right = `${PANEL_MUTED}${plainFit(sectionCount(data, item), Math.max(0, width - 34))}${PANEL_TEXT} `;
  return `${PANEL_BG}${splitRow(left, right, width)}${RESET}`;
}

/** Wide Proof. `selected` indexes `proofItems`; `open` lists expanded sections. */
export function proofSplit(store: TaskStore, run: string | undefined, selected: number, width: number, legend: boolean, open: ReadonlySet<SectionId>, now = Date.now()): ProofFrame {
  return renderProof(store, run, selected, width, legend, open, now, true);
}

/**
 * The Proof body at any width: list + detail at `SPLIT_MIN`+, otherwise a one-line picker above the same detail.
 * The selected row's bar is accent while the view has the keys (`focused`), muted otherwise. Without data the chip
 * header and one state row stand in for the body: a source with nothing shown never implies acceptance. `run` is
 * this session's run, whose commit range and summary Proof shows.
 */
export function renderProof(store: TaskStore, run: string | undefined, selected: number, width: number, legend: boolean, open: ReadonlySet<SectionId>,
  now = Date.now(), wide = width >= SPLIT_MIN, focused = true): ProofFrame {
  const lines: string[] = [], hits = new Map<number, number>();
  const state = store.state, data = store.current;
  if (state.phase === "not_applicable") {
    lines.push(...stateRow(state, "proof", width, NOT_APPLICABLE[state.reason ?? ""]));
    return { lines, hits, wide };
  }
  lines.push(...header(store, data, run, width, legend, now));
  if (!data) {
    lines.push(...stateRow(state, "task checks", width, "No checks shown · acceptance is not inferred"));
    return { lines, hits, wide };
  }
  const items = proofItems(data, run), index = Math.max(0, Math.min(selected, items.length - 1)), current = items[index];
  const detail = (detailWidth: number) => current ? sectionDetail(data, run, current, open.has(current), detailWidth, now) : [];

  if (!wide) {
    lines.push(pickerLine(data, items, index, open, width), `${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`, ...detail(width));
    return { lines, hits, wide };
  }

  lines.push(`${PANEL_RULE}${"─".repeat(LIST_WIDTH)}┬${"─".repeat(Math.max(0, width - LIST_WIDTH - 1))}${PANEL_TEXT}`);
  const left: { text: string; item?: number }[] = [{ text: listRow(`${PANEL_MUTED}SECTIONS ${items.length}${PANEL_TEXT}`, "", false) }];
  items.forEach((item, at) => {
    const count = plainFit(sectionCount(data, item), 18);
    left.push({ text: paneRow(`${PANEL_TEXT}${open.has(item) ? "▾" : "▸"} ${SECTION_LABEL[item]}`, `${PANEL_MUTED}${count}${PANEL_TEXT}`, at === index, LIST_WIDTH, focused), item: at });
  });
  const right = detail(Math.max(20, width - LIST_WIDTH - 2));
  const start = lines.length;
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const l = left[i];
    if (l?.item !== undefined) hits.set(start + i, l.item);
    lines.push(`${l?.text ?? " ".repeat(LIST_WIDTH)}${PANEL_RULE}│${PANEL_TEXT} ${right[i] ?? ""}`);
  }
  return { lines, hits, wide };
}
