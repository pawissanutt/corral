// Proof (F10) split layout: criterion list with an I T R A ladder beside the selected criterion's detail.
// Read-only presentation of the canonical inspection; freshness comes only from corral.
import { visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Inspection, InspectionStore } from "./inspection.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR } from "./panels-view.ts";

/** Main-area width at which Proof splits into list + detail; below it the stacked layout is used. */
export const SPLIT_MIN = 105;
export const LIST_WIDTH = 42;

type Criterion = Inspection["criteria"][number];
type Tone = "ok" | "bad" | "warn" | "none";
interface Step { readonly letter: "I" | "T" | "R" | "A"; readonly glyph: string; readonly word: string; readonly tone: Tone }

const tone = (t: Tone) => t === "ok" ? PANEL_SUCCESS : t === "bad" ? PANEL_ERROR : t === "warn" ? PANEL_WARNING : PANEL_MUTED;
const short = (sha: string) => sha.slice(0, 8);
const bold = (text: string) => `\x1b[1m${text}\x1b[22m`;

/** The four steps for one criterion. Glyph and word always travel together, so colour is never the only signal. */
export function ladder(store: InspectionStore, data: Inspection, c: Criterion): readonly Step[] {
  const invalid = c.failures.length > 0 || data.manifest_failures.length > 0;
  const status = c.observation?.status;
  const tested: Step = store.stale ? { letter: "T", glyph: "~", word: "retained", tone: "warn" }
    : invalid ? { letter: "T", glyph: "!", word: "invalid", tone: "bad" }
    : status === "pass" ? { letter: "T", glyph: "✓", word: "pass*", tone: "ok" }
    : status === "fail" ? { letter: "T", glyph: "✗", word: "fail", tone: "bad" }
    : status === "not_run" ? { letter: "T", glyph: "○", word: "not run", tone: "warn" }
    : { letter: "T", glyph: "·", word: "missing", tone: "none" };
  const review = data.review;
  const reviewed: Step = !review ? { letter: "R", glyph: "·", word: "missing", tone: "none" }
    : store.stale || !review.fresh ? { letter: "R", glyph: "~", word: `${review.verdict ?? "pending"} stale*`, tone: "warn" }
    : review.verdict === "approve" ? { letter: "R", glyph: "✓", word: "approve*", tone: "ok" }
    : review.verdict ? { letter: "R", glyph: "✗", word: `${review.verdict}*`, tone: "bad" }
    : { letter: "R", glyph: "…", word: "pending*", tone: "none" };
  return [
    c.implemented ? { letter: "I", glyph: "✓", word: "done*", tone: "ok" } : { letter: "I", glyph: "○", word: "not claimed", tone: "none" },
    tested, reviewed,
    { letter: "A", glyph: "—", word: "not recorded", tone: "none" },
  ];
}
const compact = (steps: readonly Step[]) => steps.map(s => `${tone(s.tone)}${s.letter}${s.glyph}${PANEL_TEXT}`).join(" ");
const chip = (label: string, glyph: string, word: string, t: Tone) => `${tone(t)}${label} ${glyph} ${word}${PANEL_TEXT}`;

/** Top chip row: candidate identity, gate, review and manifest. Stale or missing states are inline chips. */
function chips(store: InspectionStore, data: Inspection): string[] {
  const gate = data.gate, review = data.review, fresh = (f: boolean) => f && !store.stale;
  return [
    `${short(data.base)}→${short(data.head)}`,
    data.dirty ? chip("worktree", "!", "dirty", "warn") : "committed · clean",
    !gate ? chip("gate", "·", "missing", "none")
      : chip("gate", !fresh(gate.fresh) && gate.result !== "running" ? "~" : gate.result === "pass" ? "✓" : gate.result === "fail" ? "✗" : gate.result === "running" ? "…" : "~",
        `${gate.result} · ${fresh(gate.fresh) ? "fresh" : "STALE"}${gate.tier ? " · " + gate.tier : ""}`,
        gate.result === "fail" ? "bad" : !fresh(gate.fresh) ? "warn" : gate.result === "pass" ? "ok" : "none"),
    !review ? chip("review", "·", "missing", "none")
      : chip("review", !fresh(review.fresh) ? "~" : review.verdict === "approve" ? "✓" : review.verdict ? "✗" : "…",
        `${review.verdict ?? "pending"} · ${fresh(review.fresh) ? "fresh" : "STALE"}`, !fresh(review.fresh) ? "warn" : review.verdict === "approve" ? "ok" : "none"),
    !data.manifest_digest ? chip("manifest", "!", "legacy", "warn") : `manifest ${data.audited ? "audited" : "not audited"}`,
    "accepted —",
  ];
}

export interface ProofFrame { readonly lines: string[]; readonly hits: Map<number, number> }

/** Wide Proof: `expanded` opens the findings, artifacts and worker-summary sections. */
export function proofSplit(store: InspectionStore, selected: number, width: number, legend: boolean, expanded: boolean): ProofFrame {
  const lines: string[] = [], hits = new Map<number, number>();
  const full = (text: string) => { for (const row of wrapTextWithAnsi(PANEL_TEXT + text, width)) lines.push(row); };
  const data = store.current;
  const hint = `${PANEL_MUTED}j/k criterion · e ${expanded ? "fold" : "open"} sections · a artifacts · r reconcile · ? ${legend ? "hide legend" : "legend"} · F9 chat${PANEL_TEXT}`;
  if (store.stale || store.error) full(`${PANEL_WARNING}STALE / unavailable: ${plainLabel(store.error ?? "canonical read required; retained rows are historical")}${PANEL_TEXT}`);
  if (store.refreshing) full(`${PANEL_MUTED}Checking canonical records… retained results are not a new check${PANEL_TEXT}`);
  if (!data) { full("Evidence unavailable · no acceptance inferred"); full(hint); return { lines, hits }; }
  full(`${bold(`${data.task} / ${data.run}`)} · task ${plainLabel(data.task_status)} · ${chips(store, data).join(`${PANEL_MUTED} · ${PANEL_TEXT}`)}`);
  for (const failure of data.manifest_failures) full(`${PANEL_ERROR}Manifest: ${plainLabel(failure)}${PANEL_TEXT}`);
  if (legend) {
    full(`${PANEL_MUTED}I implemented · T tested · R reviewed · A accepted. Implemented ≠ tested ≠ reviewed ≠ accepted; no per-criterion acceptance record exists.${PANEL_TEXT}`);
    full(`${PANEL_MUTED}Tests are worker_assertion, never machine PASS. Review is candidate-wide. * marks a self-report or retained value. ✓ done · ✗ failed · ~ stale · ! invalid · ○ not run · · missing · — not recorded${PANEL_TEXT}`);
  }
  const rule = `${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`;
  lines.push(rule);

  // Left: one row per criterion. Right: the selected criterion.
  const left: { text: string; criterion?: number }[] = [{ text: `${PANEL_BLUE}${bold(`CRITERIA · ${data.criteria.length}`)}${PANEL_TEXT}` }];
  if (!data.manifest_digest) left.push({ text: `${PANEL_WARNING}Legacy / no criterion manifest${PANEL_TEXT}` }, { text: `${PANEL_MUTED}acceptance unknown, not zero passed${PANEL_TEXT}` });
  const idWidth = Math.min(16, Math.max(4, ...data.criteria.map(c => visibleWidth(plainLabel(c.id)))));
  data.criteria.forEach((c, index) => {
    const mark = index === selected ? `${PANEL_BLUE}▌${PANEL_TEXT}` : " ";
    left.push({ text: `${mark}${pad(plainLabel(c.id), idWidth)} ${compact(ladder(store, data, c))}${c.required ? "" : `${PANEL_MUTED} opt${PANEL_TEXT}`}`, criterion: index });
  });
  const detailWidth = Math.max(20, width - LIST_WIDTH - 3);
  const right: string[] = [];
  const add = (text: string) => right.push(...wrapTextWithAnsi(PANEL_TEXT + text, detailWidth));
  const head = (text: string) => add(`${PANEL_BLUE}${bold(text)}${PANEL_TEXT}`);
  const c = data.criteria[selected];
  if (!c) add("No criteria recorded");
  else {
    head(`Inspect ${plainLabel(c.id)} · ${c.required ? "required" : "optional"}${c.live ? " · live proof required" : ""}`);
    add(`${PANEL_MUTED}Scenario${PANEL_TEXT} ${plainLabel(c.scenario)}`);
    add(`${PANEL_MUTED}Expected${PANEL_TEXT} ${plainLabel(c.expected)}`);
    const steps = ladder(store, data, c), names = { I: "implemented", T: "tested", R: "reviewed", A: "accepted" } as const;
    add(steps.map(s => `${tone(s.tone)}${s.letter}${s.glyph} ${names[s.letter]} ${s.word}${PANEL_TEXT}`).join(`${PANEL_MUTED} · ${PANEL_TEXT}`));
    const obs = c.observation;
    add("");
    head("OBSERVATION · worker_assertion");
    if (!obs) add("Missing proof · no observation recorded");
    else {
      add(`${plainLabel(obs.key)} · ${obs.status} · captured ${plainLabel(obs.captured)} · HEAD ${short(obs.head)}`);
      add(`${PANEL_MUTED}Action${PANEL_TEXT} ${plainLabel(obs.action)}`);
      add(`${PANEL_MUTED}Observed${PANEL_TEXT} ${plainLabel(obs.observed)}`);
      add(`${PANEL_MUTED}Environment${PANEL_TEXT} ${plainLabel(obs.environment)}`);
      if (!obs.artifacts.length) add("Artifacts: missing");
      for (const path of obs.artifacts) {
        const artifact = data.artifacts.find(a => a.path === path);
        add(`▪ ${plainLabel(path)} · ${artifact ? `${plainLabel(artifact.title)} · sha256 ${artifact.sha256.slice(0, 12)}` : "metadata unavailable / omitted"}`);
      }
    }
    for (const failure of c.failures) add(`${PANEL_ERROR}Proof: ${plainLabel(failure)}${PANEL_TEXT}`);
  }
  const listRows = left.length, rows = Math.max(listRows, right.length), start = lines.length;
  for (let i = 0; i < rows; i++) {
    const l = left[i];
    if (l?.criterion !== undefined) hits.set(start + i, l.criterion);
    lines.push(`${pad(l?.text ?? "", LIST_WIDTH)} ${PANEL_RULE}│${PANEL_TEXT} ${right[i] ?? ""}`);
  }
  lines.push(rule);

  // Collapsed sections: one summary line each until `e` opens them.
  const review = data.review;
  const section = (label: string, count: string, body: () => void) => {
    full(`${PANEL_BLUE}${expanded ? "▾" : "▸"} ${bold(label)}${PANEL_TEXT}${PANEL_MUTED} · ${count}${PANEL_TEXT}`);
    if (expanded) body();
  };
  if (review?.notes) section("Review notes", "candidate-wide, not criterion acceptance", () => full(plainLabel(review.notes!)));
  section("Review findings", `${data.findings.length}${data.findings_omitted ? ` + ${data.findings_omitted} omitted` : ""} · historical candidates`, () => {
    if (!data.findings.length) full("No retained review findings");
    for (const f of data.findings) full(`${plainLabel(f.run)}/${plainLabel(f.attempt)} · ${f.superseded ? "SUPERSEDED" : "recorded; applicability requires review"} · ${f.verdict ?? "pending"} · ${short(f.head)} · ${plainLabel(f.notes)}`);
  });
  section("Run artifacts", `${data.artifacts.length}${data.artifacts_omitted ? ` + ${data.artifacts_omitted} omitted` : ""} · registered metadata, not proof`, () => {
    if (!data.artifacts.length) full("No run artifacts registered");
    for (const a of data.artifacts) full(`${plainLabel(a.title)} · ${plainLabel(a.path)} · sha256 ${a.sha256.slice(0, 12)}`);
  });
  if (data.worker_summary) section("Worker summary", "assertion, not acceptance", () => full(plainLabel(data.worker_summary!)));
  full(hint);
  return { lines, hits };
}
