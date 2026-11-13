// Proof (F10) split layout: criterion list with an I T R A ladder and MORE sections beside the selected row's detail.
// Below SPLIT_MIN columns the list collapses to a one-line picker above the same detail.
// Read-only presentation of the canonical inspection; freshness comes only from corral.
import { stripTerminalSequences, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Inspection, InspectionStore } from "./inspection.ts";
import { box, pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_WARNING, PANEL_ERROR, RESET } from "./panels-view.ts";
import { splitRow } from "./panels-chrome.ts";
import { glyphs, ladder, proofItems, sectionCount, SECTION_LABEL, short, stepChips, topChips, type Criterion, type ProofItem, type SectionId } from "./proof-parts.ts";
import { ago, bold, chip, chipRows, clock, field, listRow as paneRow, sectionRule, toneColor, wrap } from "./panel-parts.ts";

/** Main-area width at which Proof splits into list + detail; below it a one-line picker is used. */
export const SPLIT_MIN = 105;
export const LIST_WIDTH = 42;

export interface ProofFrame { readonly lines: string[]; readonly hits: Map<number, number>; readonly wide: boolean }

const plainFit = (text: string, width: number) => stripTerminalSequences(truncateToWidth(plainLabel(text), Math.max(0, width), "…"));

const listRow = (left: string, right: string, selected: boolean) => paneRow(left, right, selected, LIST_WIDTH);
const tagOf = (c: Criterion) => c.required ? (c.live ? "req·live" : "req") : "opt";

function header(store: InspectionStore, data: Inspection, width: number, legend: boolean, now: number): string[] {
  const lines: string[] = [];
  const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
  const at = store.checkedAt;
  const reads = at ? [`read ${clock(at)} (${ago(at, now)}) · r reread`, `read ${ago(at, now)} · r reread`, "r reread"] : ["not read · r reread", "r reread"];
  const chips = chipRows(topChips(store, data), width), last = chips.at(-1) ?? "";
  const read = reads.find(r => visibleWidth(last) + 2 + visibleWidth(r) <= width);
  lines.push(...chips.slice(0, -1), read ? splitRow(last, PANEL_MUTED + read + PANEL_TEXT, width) : last);
  if (!read) lines.push(pad(PANEL_MUTED + "r reread" + PANEL_TEXT, width));
  for (const failure of data.manifest_failures) full(`${PANEL_ERROR}! Manifest: ${plainLabel(failure)}${PANEL_TEXT}`);
  if (legend) {
    full(`${PANEL_MUTED}I implemented · T tested · R reviewed · A accepted. Implemented ≠ tested ≠ reviewed ≠ accepted; no per-criterion acceptance record exists.${PANEL_TEXT}`);
    full(`${PANEL_MUTED}Tests are worker assertions, never machine PASS. Review is candidate-wide. * marks a self-report or retained value. ✓ done · ✗ failed · ~ stale · ! invalid · ○ not run · · missing · — not recorded${PANEL_TEXT}`);
  }
  return lines;
}

function criterionDetail(store: InspectionStore, data: Inspection, c: Criterion, width: number, now: number): string[] {
  const out: string[] = [];
  const tags = [chip(c.required ? "required" : "optional"), ...(c.live ? [chip("live proof")] : [])].join(" ");
  out.push(`${PANEL_TEXT}${bold(plainFit(`${c.id} · ${c.scenario}`, width - visibleWidth(tags) - 2))}  ${tags}`);
  out.push(...field("Scenario", c.scenario, width, 9), ...field("Expected", c.expected, width, 9));
  out.push(sectionRule("Evidence", width), ...chipRows(stepChips(ladder(store, data, c)), width));
  for (const failure of c.failures) out.push(...wrap(`${PANEL_ERROR}! ${plainLabel(failure)}${PANEL_TEXT}`, width));
  const inner = width - 4, obs = c.observation;
  const card: string[] = [];
  if (!obs) card.push(`${PANEL_MUTED}· missing${PANEL_TEXT}  no observation recorded · no proof inferred`);
  else {
    const [glyph, tone] = obs.status === "pass" ? ["✓", "ok"] as const : obs.status === "fail" ? ["✗", "bad"] as const : ["○", "none"] as const;
    const captured = ago(Date.parse(obs.captured), now) ?? plainLabel(obs.captured);
    const retained = store.stale ? ` · ${PANEL_WARNING}~ retained${PANEL_MUTED}` : "";
    card.push(...wrap(`${toneColor(tone)}${glyph} ${obs.status.replace("_", " ")}${PANEL_MUTED}  worker assertion · captured ${captured} · HEAD ${short(obs.head)}${retained}${PANEL_TEXT}`, inner));
    card.push(...field("action", obs.action, inner, 8), ...field("observed", obs.observed, inner, 8), ...field("env", obs.environment, inner, 8));
  }
  out.push(...box("", card, width, card.length + 2));
  out.push(sectionRule("Artifacts", width));
  if (!obs?.artifacts.length) out.push(`${PANEL_MUTED}· no artifacts recorded for this criterion${PANEL_TEXT}`);
  obs?.artifacts.forEach((path, i) => {
    const artifact = data.artifacts.find(a => a.path === path);
    const meta = artifact ? `sha256 ${artifact.sha256.slice(0, 8)}… · registered` : "metadata unavailable / omitted";
    const open = i === 0 && artifact && width >= 72 ? "   Enter open" : "";
    const name = plainFit(path, width - 4 - visibleWidth(meta + open));
    out.push(`${artifact ? PANEL_BLUE + "▤" : PANEL_WARNING + "!"}${PANEL_TEXT} ${name}  ${PANEL_MUTED}${meta}${open}${PANEL_TEXT}`);
  });
  return out;
}

function sectionDetail(data: Inspection, id: SectionId, open: boolean, width: number): string[] {
  const out = [`${PANEL_TEXT}${bold(SECTION_LABEL[id])}  ${chip(sectionCount(data, id))}`];
  const text = (value: string) => out.push(...wrap(PANEL_TEXT + plainLabel(value), width));
  const note = (value: string) => out.push(...wrap(PANEL_MUTED + value + PANEL_TEXT, width));
  if (!open) { note("▸ collapsed · Enter expand"); return out; }
  switch (id) {
    case "notes": note("Candidate-wide review notes, not criterion acceptance"); text(data.review?.notes ?? ""); break;
    case "summary": note("Worker assertion, not acceptance"); text(data.worker_summary ?? ""); break;
    case "findings":
      note("Historical candidates; applicability requires review");
      if (!data.findings.length) note("No retained review findings");
      for (const f of data.findings) {
        const [glyph, tone] = f.superseded ? ["~", "none"] as const : f.verdict === "approve" ? ["✓", "ok"] as const : f.verdict ? ["✗", "bad"] as const : ["…", "warn"] as const;
        out.push(...wrap(`${toneColor(tone)}${glyph} ${plainLabel(f.verdict ?? "pending")}${PANEL_MUTED}  ${plainLabel(f.run)}/${plainLabel(f.attempt)} · HEAD ${short(f.head)}${f.superseded ? " · superseded" : ""}${PANEL_TEXT}`, width));
        out.push(...wrap(plainLabel(f.notes), width - 2).map(row => `  ${f.superseded ? PANEL_MUTED : PANEL_TEXT}${row}${PANEL_TEXT}`));
      }
      if (data.findings_omitted) note(`+${data.findings_omitted} omitted`);
      break;
    case "artifacts":
      note("Registered metadata, not proof");
      if (!data.artifacts.length) note("No run artifacts registered");
      for (const a of data.artifacts) {
        const meta = `${plainLabel(a.title)} · sha256 ${a.sha256.slice(0, 8)}…`;
        out.push(`${PANEL_BLUE}▤${PANEL_TEXT} ${plainFit(a.path, width - 4 - visibleWidth(meta))}  ${PANEL_MUTED}${plainFit(meta, width - 4)}${PANEL_TEXT}`);
      }
      if (data.artifacts_omitted) note(`+${data.artifacts_omitted} omitted`);
      break;
  }
  return out;
}

function detail(store: InspectionStore, data: Inspection, item: ProofItem | undefined, open: ReadonlySet<SectionId>, width: number, now: number): string[] {
  if (item?.kind === "section") return sectionDetail(data, item.id, open.has(item.id), width);
  const c = item ? data.criteria[item.index] : undefined;
  return c ? criterionDetail(store, data, c, width, now) : [`${PANEL_MUTED}No criteria recorded · no acceptance inferred${PANEL_TEXT}`];
}

/** Below SPLIT_MIN: `‹ c1 1/3 › targeted receipts test… req  I✓ T✓ R… A—`; sections follow as `‹ MORE 1/3 ›`. */
function pickerLine(store: InspectionStore, data: Inspection, items: readonly ProofItem[], index: number, open: ReadonlySet<SectionId>, width: number): string {
  const item = items[index];
  if (!item) return `${PANEL_BG}${PANEL_MUTED}${pad(" No criteria recorded · no acceptance inferred", width)}${RESET}`;
  const group = items.filter(i => i.kind === item.kind), position = group.indexOf(item) + 1;
  const arrows = (label: string) => `${PANEL_BLUE} ‹ ${PANEL_TEXT}${bold(label)} ${PANEL_MUTED}${position}/${group.length}${PANEL_BLUE} › `;
  let left: string, right: string;
  if (item.kind === "criterion") {
    const c = data.criteria[item.index], steps = c ? ladder(store, data, c) : [];
    right = steps.map(s => `${toneColor(s.tone)}${s.letter}${s.glyph}`).join(" ") + PANEL_TEXT + " ";
    const head = arrows(plainFit(c?.id ?? "", 12)), tag = c ? tagOf(c) : "";
    const room = width - visibleWidth(head) - visibleWidth(tag) - 1 - visibleWidth(right) - 2;
    left = `${head}${PANEL_TEXT}${plainFit(c?.scenario ?? "", room)} ${PANEL_MUTED}${tag}`;
  } else {
    right = `${PANEL_MUTED}${plainFit(sectionCount(data, item.id), Math.max(0, width - 34))}${PANEL_TEXT} `;
    left = `${arrows("MORE")}${PANEL_TEXT}${open.has(item.id) ? "▾" : "▸"} ${SECTION_LABEL[item.id]}`;
  }
  return `${PANEL_BG}${splitRow(left, right, width)}${RESET}`;
}

/** Wide Proof. `selected` indexes `proofItems` (criteria, then sections); `open` lists expanded sections. */
export function proofSplit(store: InspectionStore, selected: number, width: number, legend: boolean, open: ReadonlySet<SectionId>, now = Date.now()): ProofFrame {
  return renderProof(store, selected, width, legend, open, now, true);
}

/** The Proof body at any width: list + detail at `SPLIT_MIN`+, otherwise a one-line picker above the same detail. */
export function renderProof(store: InspectionStore, selected: number, width: number, legend: boolean, open: ReadonlySet<SectionId>, now = Date.now(), wide = width >= SPLIT_MIN): ProofFrame {
  const lines: string[] = [], hits = new Map<number, number>();
  const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
  const data = store.current;
  if (store.stale || store.error) full(`${PANEL_WARNING}~ STALE / unavailable: ${plainLabel(store.error ?? "canonical read required; retained rows are historical")}${PANEL_TEXT}`);
  if (store.refreshing) full(`${PANEL_MUTED}Checking canonical records… retained results are not a new check${PANEL_TEXT}`);
  if (!data) { full("Evidence unavailable · no acceptance inferred"); return { lines, hits, wide }; }
  lines.push(...header(store, data, width, legend, now));
  const items = proofItems(data), index = Math.max(0, Math.min(selected, items.length - 1)), current = items[index];

  if (!wide) {
    lines.push(pickerLine(store, data, items, index, open, width), `${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`);
    if (!data.manifest_digest) full(`${PANEL_WARNING}! legacy · no criterion manifest${PANEL_MUTED} · acceptance unknown, not zero passed${PANEL_TEXT}`);
    lines.push(...detail(store, data, current, open, width, now));
    return { lines, hits, wide };
  }

  lines.push(`${PANEL_RULE}${"─".repeat(LIST_WIDTH)}┬${"─".repeat(Math.max(0, width - LIST_WIDTH - 1))}${PANEL_TEXT}`);
  const left: { text: string; item?: number }[] = [{ text: listRow(`${PANEL_MUTED}CRITERIA ${data.criteria.length}`, `I T R A${PANEL_TEXT}`, false) }];
  if (!data.manifest_digest) left.push({ text: listRow(`${PANEL_WARNING}! legacy · no criterion manifest${PANEL_TEXT}`, "", false) }, { text: listRow(`${PANEL_MUTED}acceptance unknown, not zero passed${PANEL_TEXT}`, "", false) });
  items.forEach((item, at) => {
    const on = at === index;
    if (item.kind === "criterion") {
      const c = data.criteria[item.index];
      if (!c) return;
      const tag = tagOf(c), label = plainFit(`${c.id} ${c.scenario}`, LIST_WIDTH - 3 - 9 - visibleWidth(tag) - 1);
      left.push({ text: listRow(`${PANEL_TEXT}${label} ${PANEL_MUTED}${tag}${PANEL_TEXT}`, glyphs(ladder(store, data, c)), on), item: at });
    } else {
      if (items[at - 1]?.kind === "criterion") left.push({ text: " ".repeat(LIST_WIDTH) }, { text: listRow(`${PANEL_MUTED}MORE${PANEL_TEXT}`, "", false) });
      const count = plainFit(sectionCount(data, item.id), 18);
      left.push({ text: listRow(`${PANEL_TEXT}${open.has(item.id) ? "▾" : "▸"} ${SECTION_LABEL[item.id]}`, `${PANEL_MUTED}${count}${PANEL_TEXT}`, on), item: at });
    }
  });
  const right = detail(store, data, current, open, Math.max(20, width - LIST_WIDTH - 2), now);
  const start = lines.length;
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const l = left[i];
    if (l?.item !== undefined) hits.set(start + i, l.item);
    lines.push(`${l?.text ?? " ".repeat(LIST_WIDTH)}${PANEL_RULE}│${PANEL_TEXT} ${right[i] ?? ""}`);
  }
  return { lines, hits, wide };
}
