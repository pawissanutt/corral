import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { InspectionStore, type Inspection } from "./inspection.ts";
import { proofSplit } from "./proof-view.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
function data(): Inspection {
  return {
    version: 1, task: "T-001", run: "R1", root: "/repo", base: "a".repeat(40), head: "b".repeat(40), dirty: false,
    manifest_digest: "m1", audited: true, manifest_failures: [], task_status: "review",
    criteria: ["C1", "C2"].map((id, i) => ({ id, required: true, live: true, scenario: "Scenario " + id, expected: "expected", implemented: i === 0,
      failures: i === 0 ? [] : ["evidence_missing:C2"], observation: i === 0 ? {
        key: "obs1", authority: "worker_assertion" as const, status: "pass" as const, observed: "worker reported output", action: "run", environment: "isolated",
        captured: "2026-10-14T12:00:00Z", head: "b".repeat(40), manifest_digest: "m1", artifacts: ["proof.log"],
      } : null })),
    gate: { result: "pass", tier: "quick", fresh: true, log_passed: true, head: "b".repeat(40), attempt: "G1" },
    review: { verdict: "approve", fresh: false, head: "a".repeat(40), reviewer: "reviewer", notes: "Candidate-wide review notes" },
    findings: [{ run: "R1", attempt: "RV1", head: "a".repeat(40), verdict: "changes", superseded: true, notes: "Historical finding" }],
    findings_omitted: 0, artifacts: [{ path: "proof.log", title: "Proof", sha256: "c".repeat(64) }], artifacts_omitted: 0,
    worker_summary: "worker says complete",
  };
}

test("wide Proof shows an I T R A ladder with glyphs, keeps acceptance unrecorded and folds history", async () => {
  const store = new InspectionStore(identity, async () => data(), () => {});
  await store.refresh();
  for (const width of [105, 140, 200]) {
    const frame = proofSplit(store, 1, width, false, false);
    for (const row of frame.lines) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    const text = stripTerminalSequences(frame.lines.join("\n"));
    expect(text).toContain("I✓ T✓ R~ A—"); // C1: implemented, tested (assertion), review stale, never accepted.
    expect(text).toContain("I○ T! R~ A—"); // C2: not claimed, invalid proof.
    expect(text).toContain("Inspect C2"); expect(text).toContain("evidence_missing:C2");
    expect(text).toContain("accepted —"); expect(text.replaceAll("\n", " ")).toContain("review ~ approve · STALE");
    expect(text).toContain("▸ Review findings"); expect(text).not.toContain("SUPERSEDED");
    expect([...frame.hits.values()].sort()).toEqual([0, 1]);
  }
  const open = stripTerminalSequences(proofSplit(store, 0, 140, true, true).lines.join("\n"));
  expect(open).toContain("SUPERSEDED"); expect(open).toContain("worker_assertion"); expect(open).toContain("Implemented ≠ tested");
  expect(open).toContain("sha256 cccccccccccc");
  store.dispose();
});

test("an unavailable read stays inline and infers no acceptance", async () => {
  const store = new InspectionStore(identity, async () => { throw new Error("offline"); }, () => {});
  await store.refresh();
  const text = stripTerminalSequences(proofSplit(store, 0, 140, false, false).lines.join("\n"));
  expect(text).toContain("STALE / unavailable: offline"); expect(text).toContain("no acceptance inferred");
  store.dispose();
});
