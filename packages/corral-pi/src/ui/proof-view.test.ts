import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { InspectionStore, type Inspection } from "./inspection.ts";
import { renderProof, proofSplit, LIST_WIDTH } from "./proof-view.ts";
import { proofItems } from "./proof-parts.ts";
import { PANEL_SELECTED } from "./panels-view.ts";
import { clock } from "./panel-parts.ts";

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
    review: { verdict: "approve", fresh: false, head: "a".repeat(40), reviewer: "reviewer", notes: null },
    findings: [{ run: "R1", attempt: "RV1", head: "a".repeat(40), verdict: "changes", superseded: true, notes: "Historical finding" }],
    findings_omitted: 0, artifacts: [{ path: "proof.log", title: "Proof", sha256: "c".repeat(64) }], artifacts_omitted: 0,
    worker_summary: "worker says complete",
  };
}
const strip = (line: string) => stripTerminalSequences(line);
const listCell = (line: string) => strip(line).slice(0, LIST_WIDTH);

test("wide Proof: one chip row, selectable criteria and MORE rows with glyph ladders, boxed observation", async () => {
  const store = new InspectionStore(identity, async () => data(), () => {});
  await store.refresh();
  const now = Date.parse("2026-10-14T12:02:00Z");
  store.checkedAt = now - 20_000;
  const items = proofItems(store.current);
  expect(items.map(item => item.kind === "criterion" ? item.index : item.id)).toEqual([0, 1, "findings", "artifacts", "summary"]);
  for (const width of [105, 140, 200]) {
    const frame = proofSplit(store, 0, width, false, new Set(), now);
    for (const row of frame.lines) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    const rule = frame.lines.findIndex(line => strip(line).includes("┬"));
    // The review's own staleness is its chip's word; the read itself is current, so no source-stale chip shows.
    const head = strip(frame.lines[0] ?? "");
    expect(head).toMatch(/^ gate ✓ /); expect(head).toMatch(/ review ~ approve · stale /); expect(head).not.toContain("! stale");
    expect(strip(frame.lines[rule - 1] ?? "")).toMatch(/ r reread\s*$/);
    expect(frame.lines.some(line => strip(line).includes("j/k criterion"))).toBe(false);
    const rows = [...frame.hits.entries()].sort(([a], [b]) => a - b);
    expect(rows.map(([, item]) => item)).toEqual([0, 1, 2, 3, 4]);
    const [c1Row = -1, c2Row = -1, findingsRow = -1] = rows.map(([line]) => line);
    expect(listCell(frame.lines[c1Row - 1] ?? "")).toMatch(/CRITERIA 2\s+I T R A $/);
    expect(listCell(frame.lines[c1Row] ?? "")).toMatch(/^▌ C1 Scenario C1 req·live\s+✓ ✓ ~ — $/);
    expect(listCell(frame.lines[c2Row] ?? "")).toMatch(/^  C2 Scenario C2 req·live\s+○ ! ~ — $/);
    expect(listCell(frame.lines[findingsRow] ?? "")).toMatch(/^  ▸ Review findings\s+1 · 1 superseded $/);
    const selected = frame.lines[c1Row] ?? "";
    expect(selected.startsWith(PANEL_SELECTED)).toBe(true);
    expect(selected.slice(0, selected.indexOf("│")).includes("\x1b[0m")).toBe(false);
    const detail = frame.lines.map(line => strip(line).slice(LIST_WIDTH + 2));
    for (const step of [" I ✓ implemented ", " T ✓ pass · worker ", " R ~ approve stale* ", " A — not recorded "]) expect(detail.some(line => line.includes(step))).toBe(true);
    const top = detail.findIndex(line => line.startsWith("╭")), bottom = detail.findIndex(line => line.startsWith("╰"));
    expect(top).toBeGreaterThan(0); expect(bottom - top).toBeGreaterThanOrEqual(5);
    expect(detail.slice(top + 1, bottom).every(line => line.startsWith("│ ") && line.trimEnd().endsWith("│"))).toBe(true);
    expect(detail[top + 1]).toContain("captured 2m ago");
    expect(detail.some(line => line.startsWith("▤ proof.log") && line.includes("sha256 cccccccc…"))).toBe(true);
  }
  store.dispose();
});

test("Proof sections open in the detail pane and invalid proof stays inline", async () => {
  const store = new InspectionStore(identity, async () => data(), () => {});
  await store.refresh();
  const closed = proofSplit(store, 2, 140, false, new Set()).lines.map(strip).join("\n");
  expect(closed).toContain("▸ Review findings"); expect(closed).not.toContain("Historical finding");
  const open = proofSplit(store, 2, 140, false, new Set(["findings"])).lines.map(strip).join("\n");
  expect(open).toContain("▾ Review findings"); expect(open).toContain("~ changes"); expect(open).toContain("Historical finding");
  const invalid = proofSplit(store, 1, 140, false, new Set()).lines.map(strip).join("\n");
  expect(invalid).toContain("! evidence_missing:C2"); expect(invalid).toContain("· missing");
  for (const width of [52, 80]) {
    const narrow = renderProof(store, 1, width, false, new Set());
    const rows = narrow.lines.map(strip);
    expect(narrow.wide).toBe(false);
    for (const row of narrow.lines) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    expect(rows.find(row => row.includes("‹"))).toMatch(/^ ‹ C2 2\/2 › .* I○ T! R~ A— $/);
    expect(rows.some(row => row.startsWith("╭"))).toBe(true);
    expect(rows.some(row => /[0-9a-f]{40}/.test(row))).toBe(false);
  }
  const section = renderProof(store, 2, 80, false, new Set(["findings"])).lines.map(strip);
  expect(section.find(row => row.includes("‹"))).toMatch(/^ ‹ MORE 1\/3 › ▾ Review findings/);
  expect(section).toContain("  Historical finding");
  store.dispose();
});

test("a failed read shows no evidence; a failed reread keeps the retained proof, its read time and the reason", async () => {
  let fail = true;
  const store = new InspectionStore(identity, async () => { if (fail) throw new Error("offline"); return data(); }, () => {});
  await store.refresh();
  expect(store.state.phase).toBe("failed");
  const failed = proofSplit(store, 0, 140, false, new Set()).lines.map(strip);
  expect(failed.join("\n")).toContain("offline"); expect(failed.join("\n")).toContain("acceptance is not inferred");
  expect(failed.some(line => line.includes("┬"))).toBe(false);
  fail = false; await store.refresh(true);
  const readAt = store.checkedAt ?? 0, now = readAt + 120_000;
  // Rows above the list or picker: the chip header and the problem line.
  const top = (width: number) => {
    const rows = renderProof(store, 0, width, false, new Set(), now).lines.map(strip);
    return rows.slice(0, rows.findIndex(row => row.includes("┬") || row.includes("‹"))).join("\n");
  };
  for (const width of [44, 58, 101]) expect(top(width)).toContain("2m ago");
  fail = true; await store.refresh(true);
  expect(store.state.phase).toBe("retained"); expect(store.state.readAt).toBe(readAt);
  for (const width of [44, 58, 101, 140]) {
    const frame = renderProof(store, 0, width, false, new Set(), now);
    for (const row of frame.lines) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    expect(top(width)).toContain(clock(readAt)); expect(top(width)).toContain("offline");
    expect(frame.lines.map(strip).some(row => row.includes("C1"))).toBe(true);
  }
  store.dispose();
});
