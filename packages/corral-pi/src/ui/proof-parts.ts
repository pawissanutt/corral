// Proof (F10) building blocks: the I T R A ladder, its chips and the selectable row model.
// Pure presentation of the canonical inspection; nothing here reads or infers freshness.
import type { Inspection, InspectionStore } from "./inspection.ts";
import { plainLabel, PANEL_TEXT } from "./panels-view.ts";
import { chip, toneColor, type Tone } from "./panel-parts.ts";

export type Criterion = Inspection["criteria"][number];
export interface Step { readonly letter: "I" | "T" | "R" | "A"; readonly glyph: string; readonly word: string; readonly tone: Tone }
export type SectionId = "notes" | "findings" | "artifacts" | "summary";
/** One selectable row of the Proof list: criteria first, then the MORE sections. */
export type ProofItem = { readonly kind: "criterion"; readonly index: number } | { readonly kind: "section"; readonly id: SectionId };

export const short = (sha: string) => sha.slice(0, 8);
/** The four steps for one criterion. Glyph and word always travel together. */
export function ladder(store: InspectionStore, data: Inspection, c: Criterion): readonly Step[] {
  const invalid = c.failures.length > 0 || data.manifest_failures.length > 0;
  const status = c.observation?.status;
  const tested: Step = store.stale ? { letter: "T", glyph: "~", word: "retained", tone: "warn" }
    : invalid ? { letter: "T", glyph: "!", word: "invalid", tone: "bad" }
    : status === "pass" ? { letter: "T", glyph: "✓", word: "pass · worker", tone: "ok" }
    : status === "fail" ? { letter: "T", glyph: "✗", word: "fail · worker", tone: "bad" }
    : status === "not_run" ? { letter: "T", glyph: "○", word: "not run", tone: "none" }
    : { letter: "T", glyph: "·", word: "missing", tone: "none" };
  const review = data.review;
  const reviewed: Step = !review ? { letter: "R", glyph: "·", word: "missing", tone: "none" }
    : store.stale || !review.fresh ? { letter: "R", glyph: "~", word: `${plainLabel(review.verdict ?? "pending")} stale*`, tone: "warn" }
    : review.verdict === "approve" ? { letter: "R", glyph: "✓", word: "approve*", tone: "ok" }
    : review.verdict ? { letter: "R", glyph: "✗", word: `${plainLabel(review.verdict)}*`, tone: "bad" }
    : { letter: "R", glyph: "…", word: "pending*", tone: "warn" };
  return [
    c.implemented ? { letter: "I", glyph: "✓", word: "implemented", tone: "ok" } : { letter: "I", glyph: "○", word: "not claimed", tone: "none" },
    tested, reviewed,
    { letter: "A", glyph: "—", word: "not recorded", tone: "none" },
  ];
}
/** Glyph-only ladder for a list row; the `I T R A` column header above it carries the letters. */
export const glyphs = (steps: readonly Step[]) => steps.map(s => `${toneColor(s.tone)}${s.glyph}`).join(" ") + PANEL_TEXT;
export const stepChips = (steps: readonly Step[]) => steps.map(s => chip(`${s.letter} ${s.glyph} ${s.word}`, s.tone));

/** Top chip row: gate, review, manifest, candidate range and worktree. Stale or missing states stay inline as chips. */
export function topChips(store: InspectionStore, data: Inspection): string[] {
  const gate = data.gate, review = data.review, fresh = (f: boolean) => f && !store.stale;
  const gateChip = !gate ? chip("gate · missing")
    : chip(`gate ${!fresh(gate.fresh) && gate.result !== "running" ? "~" : gate.result === "pass" ? "✓" : gate.result === "fail" ? "✗" : gate.result === "running" ? "…" : "~"} ${gate.result} · ${fresh(gate.fresh) ? "fresh" : "STALE"}${gate.tier ? ` · ${gate.tier} tier` : ""}`,
      gate.result === "fail" ? "bad" : !fresh(gate.fresh) ? "warn" : gate.result === "pass" ? "ok" : "none");
  const reviewChip = !review ? chip("review · missing")
    : !fresh(review.fresh) ? chip(`review ~ ${review.verdict ?? "pending"} · STALE`, "warn")
    : review.verdict === "approve" ? chip("review ✓ approve", "ok")
    : review.verdict ? chip(`review ✗ ${review.verdict}`, "bad") : chip("review … pending", "warn");
  return [
    gateChip, reviewChip,
    data.manifest_digest ? chip(`manifest ${short(data.manifest_digest)} · ${data.audited ? "audited" : "not audited"}`, data.audited ? "none" : "warn") : chip("manifest ! legacy", "warn"),
    chip(`${short(data.base)} → ${short(data.head)}`),
    data.dirty ? chip("! dirty", "warn") : chip("clean", "ok"),
  ];
}

export function proofItems(data: Inspection | undefined): ProofItem[] {
  if (!data) return [];
  const items: ProofItem[] = data.criteria.map((_, index) => ({ kind: "criterion", index }));
  const sections: SectionId[] = [...(data.review?.notes ? ["notes" as const] : []), "findings", "artifacts", ...(data.worker_summary ? ["summary" as const] : [])];
  return [...items, ...sections.map(id => ({ kind: "section" as const, id }))];
}
export const SECTION_LABEL: Record<SectionId, string> = { notes: "Review notes", findings: "Review findings", artifacts: "Run artifacts", summary: "Worker summary" };
export function sectionCount(data: Inspection, id: SectionId): string {
  switch (id) {
    case "notes": return "candidate-wide";
    case "findings": {
      const superseded = data.findings.filter(f => f.superseded).length;
      return `${data.findings.length}${data.findings_omitted ? ` +${data.findings_omitted} omitted` : ""}${superseded ? ` · ${superseded} superseded` : ""}`;
    }
    case "artifacts": return `${data.artifacts.length}${data.artifacts_omitted ? ` +${data.artifacts_omitted} omitted` : ""}`;
    case "summary": return "assertion";
  }
}
