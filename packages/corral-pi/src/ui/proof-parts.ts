// Proof (F10) building blocks: the chips and the selectable section model over the task's newest checks.
// Pure presentation of `corral show --json`; nothing here reads or infers freshness. A gate result belongs to the
// commit it ran on and a review to its round; data kept from an earlier read is `retained`, stated once by the
// shared chip header.
import type { TaskCheck, TaskView } from "./task-store.ts";
import { chip } from "./panel-parts.ts";

export type SectionId = "gate" | "notes" | "findings" | "artifacts" | "summary";

export const short = (sha: string) => sha.slice(0, 8);
const newest = (checks: readonly TaskCheck[]) => checks.reduce<TaskCheck | undefined>((last, check) => !last || check.at >= last.at ? check : last, undefined);
export const gateCheck = (data: TaskView) => newest(data.checks.filter(check => check.kind === "gate"));
export const reviewChecks = (data: TaskView) => data.checks.filter(check => check.kind === "review").sort((a, b) => b.at.localeCompare(a.at));
/** The run Proof speaks for: this session's, else the newest worker run. */
export const proofRun = (data: TaskView, run: string | undefined) =>
  data.runs.find(item => item.id === run) ?? data.runs.filter(item => item.role === "worker").at(-1);

/** Top chip row: the newest gate, the newest review and the run's commit range. Missing records stay inline as chips. */
export function topChips(data: TaskView, run: string | undefined): string[] {
  const gate = gateCheck(data), review = reviewChecks(data)[0], current = proofRun(data, run);
  const gateChip = !gate ? chip("gate · missing")
    : chip(`gate ${gate.result === "pass" ? "✓ pass" : "✗ fail"} · ${short(gate.key)}`, gate.result === "pass" ? "ok" : "bad");
  const reviewChip = !review ? chip("review · missing")
    : review.result === "pass" ? chip(`review ✓ approve · round ${review.key}`, "ok") : chip(`review ✗ changes · round ${review.key}`, "bad");
  return [
    gateChip, reviewChip,
    ...(current ? [chip(`${short(current.base_sha)} → ${current.head_sha ? short(current.head_sha) : "worktree"}`)] : []),
  ];
}

export function proofItems(data: TaskView | undefined, run: string | undefined): SectionId[] {
  if (!data) return [];
  return [
    ...(gateCheck(data) ? ["gate" as const] : []),
    ...(reviewChecks(data)[0]?.notes ? ["notes" as const] : []),
    "findings", "artifacts",
    ...(proofRun(data, run)?.summary ? ["summary" as const] : []),
  ];
}
export const SECTION_LABEL: Record<SectionId, string> = {
  gate: "Gate result", notes: "Review notes", findings: "Review findings", artifacts: "Task artifacts", summary: "Worker summary",
};
export function sectionCount(data: TaskView, id: SectionId): string {
  switch (id) {
    case "gate": return gateCheck(data)?.result ?? "missing";
    case "notes": return "newest round";
    case "findings": return String(reviewChecks(data).length);
    case "artifacts": return String(data.artifacts.length);
    case "summary": return "assertion";
  }
}
