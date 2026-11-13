import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripTerminalSequences, Text, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { InspectionStore, evidenceLines, type Inspection } from "./inspection.ts";
import { ReviewStore } from "./review-store.ts";
import { NativeWorkbench } from "./workbench.ts";
import { parseChangedPaths } from "./candidate-git.ts";
import { CorralClient } from "../tools/corral.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
function data(): Inspection {
  return {
    version: 1, task: "T-001", run: "R1", root: "/repo", base: "a".repeat(40), head: "b".repeat(40), dirty: false,
    manifest_digest: "m1", audited: true, manifest_failures: [], task_status: "review",
    criteria: ["C1", "C2"].map((id, i) => ({ id, required: true, live: true, scenario: "Scenario " + id, expected: "expected", implemented: i === 0,
      failures: i === 0 ? [] : ["evidence_missing:C2"], observation: i === 0 ? {
        key: "obs1", authority: "worker_assertion", status: "pass", observed: "worker reported output", action: "run", environment: "isolated",
        captured: "2026-10-14T12:00:00Z", head: "b".repeat(40), manifest_digest: "m1", artifacts: ["proof.log"],
      } : null })),
    gate: { result: "pass", tier: "quick", fresh: false, log_passed: true, head: "a".repeat(40), attempt: "G1" },
    review: { verdict: "approve", fresh: false, head: "a".repeat(40), reviewer: "reviewer", notes: "Candidate-wide review notes" },
    findings: [{ run: "R1", attempt: "RV1", head: "a".repeat(40), verdict: "changes", superseded: true, notes: "Historical finding" }],
    findings_omitted: 2, artifacts: [{ path: "proof.log", title: "Proof", sha256: "c".repeat(64) }], artifacts_omitted: 0,
    worker_summary: "worker says complete",
  };
}
const text = (store: InspectionStore, width = 120) => evidenceLines(store, 0, width).map(row => row.text).join("\n");

test("evidence stays canonical, explicit and read-only across candidate/manifest/artifact identity changes", async () => {
  let current = data(), reads = 0;
  const store = new InspectionStore(identity, async () => { reads++; return current; }, () => {});
  await store.refresh();
  const first = text(store);
  expect(first).toContain("worker_assertion"); expect(first).toContain("Gate receipt: pass · STALE");
  expect(first).toContain("approve · STALE"); expect(first).toContain("SUPERSEDED");
  expect(first).toContain("Accepted"); expect(first).toContain("not recorded");
  expect(first).toContain("evidence_missing:C2"); expect(first).toContain("SHA256 " + "c".repeat(64));
  for (const defect of ["candidate", "manifest", "artifact"] as const) {
    current = data();
    if (defect === "candidate") current.head = "d".repeat(40);
    if (defect === "manifest") current.manifest_digest = "m2";
    if (defect === "artifact") current.artifacts[0]!.sha256 = "e".repeat(64);
    // Only corral decides freshness: even an equal SHA doesn't overrule this result.
    current.criteria[0]!.failures = [defect === "artifact" ? "evidence_artifact_invalid:C1" : "evidence_stale:C1"];
    store.invalidate(); await store.refresh();
    expect(text(store)).toContain(current.criteria[0]!.failures[0]!);
    expect(text(store)).not.toContain("admitted at read*");
  }
  const calls = reads; text(store); text(store, 52); text(store, 140); await store.refresh();
  expect(reads).toBe(calls); // render and fresh cache hit never hydrate a task or spawn a read.
  store.dispose();
});

test("legacy, failed/not-run, invalid manifest and unavailable read are not empty success", async () => {
  let current = data(), fail = false;
  const store = new InspectionStore(identity, async () => { if (fail) throw new Error("offline"); return current; }, () => {});
  current.manifest_digest = null; current.criteria = [];
  await store.refresh(); expect(text(store)).toContain("Legacy / no criterion manifest");
  for (const status of ["fail", "not_run"] as const) {
    current = data(); current.criteria[0]!.observation!.status = status;
    current.criteria[0]!.failures = ["evidence_" + (status === "fail" ? "failed" : "not_run") + ":C1"];
    current.manifest_failures = ["manifest_spec_stale"];
    await store.refresh(true); expect(text(store)).toContain(status); expect(text(store)).toContain("manifest_spec_stale");
  }
  fail = true; await store.refresh(true);
  expect(store.stale).toBeTrue(); expect(text(store)).toContain("STALE / unavailable: offline");
  expect(text(store)).toContain("worker says complete"); // retained with assertion provenance, never accepted.
  store.dispose();
});

test("overlapping invalidation, wrong-run responses and malformed projections fail closed", async () => {
  let release!: (value: unknown) => void;
  const store = new InspectionStore(identity, () => new Promise(resolve => { release = resolve; }), () => {});
  const old = store.refresh(); store.invalidate(); release(data()); await old;
  expect(store.current).toBeUndefined(); expect(store.stale).toBeTrue();
  const read = store.refresh(); release({ ...data(), run: "R2" }); await read;
  expect(store.error).toContain("identity mismatch");
  const malformed = store.refresh(); release({ ...data(), gate: undefined }); await malformed;
  expect(store.stale).toBeTrue(); expect(store.current).toBeUndefined(); store.dispose();
});

test("canonical client sends only an explicit bounded run inspection read, without receipts or secrets", async () => {
  const root = mkdtempSync(join(tmpdir(), "inspection-client-"));
  try {
    const fake = writeFakeCorral(root); writeFileSync(join(root, "inspection.json"), JSON.stringify(data()));
    const client = new CorralClient({ corral: fake.path, root, identity, hostCap: "secret", sessionId: () => "session" });
    expect(await client.inspectionSnapshot()).toEqual(data()); expect(fake.calls()).toHaveLength(0);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("committed candidate files, binary/rename diffstats and captured hunks ignore later HEAD and dirty edits", async () => {
  const root = mkdtempSync(join(tmpdir(), "candidate-git-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" }).trim();
  const current = data();
  const store = new ReviewStore(root, () => {}, async () => current);
  try {
    git("init", "-qb", "main"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "old name.ts"), "base\n"); git("add", "."); git("commit", "-qm", "base");
    current.base = git("rev-parse", "HEAD"); current.root = root;
    git("mv", "old name.ts", "renamed name.ts"); writeFileSync(join(root, "added.ts"), "committed line\n");
    writeFileSync(join(root, "binary.dat"), new Uint8Array([0, 1, 2])); git("add", "."); git("commit", "-qm", "candidate");
    current.head = git("rev-parse", "HEAD");
    writeFileSync(join(root, "added.ts"), "dirty worktree\n");
    await store.refresh(); expect(store.comparison).toBe("candidate");
    expect(store.files.map(f => f.path)).toEqual(["added.ts", "binary.dat", "renamed name.ts"]);
    expect(store.stats.get("binary.dat")).toEqual({ added: null, removed: null });
    expect(store.files.find(f => f.path === "renamed name.ts")?.originalPath).toBe("old name.ts");
    await store.select("added.ts"); expect(store.patch).toContain("+committed line"); expect(store.patch).not.toContain("dirty worktree");
    const captured = store.patchSnapshot;
    git("add", "."); git("commit", "-qm", "later HEAD");
    await store.select("added.ts"); expect(store.patchSnapshot?.head).toBe(captured?.head); expect(store.patch).toBe(captured?.text);
    expect(store.patchSnapshot?.base).toBe(current.base);
    const wb = new NativeWorkbench({ cwd: root, identity, snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => current });
    try {
      const component = wb.mount(new Text("transcript", 0, 0)); wb.setView("changes"); await wb.review.refresh(); await wb.openFile("added.ts");
      const visible = stripTerminalSequences(renderLayoutFrame(component, 80, 50, () => {}).lines.join("\n"));
      expect(visible).toContain(current.base.slice(0, 8)); expect(visible).toContain(current.head.slice(0, 8)); expect(visible).toMatch(/added\.ts\s+added\s+\+1 −0/);
      expect(visible).toContain("+ committed line"); expect(visible).not.toContain("dirty worktree");
    } finally { wb.dispose(); }
    expect(parseChangedPaths("M\0literal\n界面.ts\0")[0]?.path).toBe("literal\n界面.ts");
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("retained candidate status/diffstat and hunks advance together only when the new patch is applied", async () => {
  for (const next of ["edit", "delete"] as const) {
    const root = mkdtempSync(join(tmpdir(), "candidate-refresh-"));
    const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" }).trim();
    const current = data();
    const wb = new NativeWorkbench({ cwd: root, identity, snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => current });
    const component = wb.mount(new Text("transcript", 0, 0));
    const capture = process.env.CORRAL_F1_CAPTURE_DIR;
    const render = (width: number, stage: string) => {
      const ansi = renderLayoutFrame(component, width, 50, () => {}).lines.join("\n");
      if (capture) { mkdirSync(capture, { recursive: true }); writeFileSync(join(capture, `${next}-${stage}-${width}.ansi`), ansi + "\n"); }
      return stripTerminalSequences(ansi);
    };
    try {
      git("init", "-qb", "main"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
      writeFileSync(join(root, "a.txt"), "base\n"); git("add", "."); git("commit", "-qm", "base");
      current.root = root; current.base = git("rev-parse", "HEAD");
      writeFileSync(join(root, "a.txt"), "first\n"); git("add", "."); git("commit", "-qm", "candidate A");
      const first = current.head = git("rev-parse", "HEAD");
      wb.setView("changes"); await wb.review.refresh(); await wb.openFile("a.txt"); await wb.review.refresh();
      for (const width of [80, 140]) {
        const initial = render(width, "initial");
        expect(initial).toContain(first.slice(0, 8)); expect(initial).toMatch(/a\.txt\s+modified\s+\+1 −1/); expect(initial).toContain("+ first");
      }
      if (next === "edit") writeFileSync(join(root, "a.txt"), "second\nsecond2\nsecond3\n");
      else rmSync(join(root, "a.txt"));
      git("add", "."); git("commit", "-qm", "candidate B");
      const second = current.head = git("rev-parse", "HEAD");
      if (capture) writeFileSync(join(capture, `${next}-identities.json`), JSON.stringify({ input: "isolated real Git fixture through production inspection seam", base: current.base, first, second }, null, 2));
      wb.inspection.invalidate(); await wb.review.refresh();
      expect(wb.review.head).toBe(second);
      expect(wb.review.stats.get("a.txt")).toEqual({ added: next === "edit" ? 3 : 0, removed: 1 });
      for (const width of [80, 140]) {
        const retained = render(width, "retained");
        expect(retained).toContain(first.slice(0, 8)); expect(retained).not.toContain(second.slice(0, 8)); expect(retained).toContain(current.base.slice(0, 8));
        expect(retained).toContain("+ first"); expect(retained).toMatch(/a\.txt\s+modified\s+\+1 −1/);
        expect(retained).toContain("New diff available"); expect(retained).not.toMatch(/a\.txt\s+deleted/);
      }
      expect(wb.handleInput("r")).toBeTrue(); await wb.review.refresh();
      for (const width of [80, 140]) {
        const applied = render(width, "applied");
        expect(applied).toContain(second.slice(0, 8)); expect(applied).not.toContain(first.slice(0, 8)); expect(applied).toContain(current.base.slice(0, 8));
        expect(applied).toMatch(next === "edit" ? /a\.txt\s+modified\s+\+3 −1/ : /a\.txt\s+deleted\s+\+0 −1/);
        expect(applied).not.toContain("+ first"); expect(applied).toContain(next === "edit" ? "+ second3" : "− base");
      }
    } finally { wb.dispose(); rmSync(root, { recursive: true, force: true }); }
  }
});

test("Evidence keyboard/pointer navigation survives narrow/wide rendering and preserves the editor transcript", async () => {
  const wb = new NativeWorkbench({ cwd: tmpdir(), identity, snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => data() });
  const root = wb.mount(new Text("REAL_TRANSCRIPT", 0, 0));
  const lines = (width: number) => renderLayoutFrame(root, width, 200, () => {}).lines;
  try {
    wb.handleInput("\x1b[21~"); await wb.inspection.refresh();
    expect(wb.view).toBe("evidence");
    for (const width of [52, 80, 140]) for (const row of lines(width)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    wb.handleInput("j"); expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^▌ C2 /m);
    wb.handleInput("k"); expect(stripTerminalSequences(lines(52).join("\n"))).toMatch(/^ ‹ C1 1\/2 › /m);
    const wide = lines(140).map(row => stripTerminalSequences(row));
    // Key hints live in the footer, never in the Proof content.
    expect(wide.some(row => row.includes("j/k criterion"))).toBe(false);
    const line = wide.findIndex(row => row.startsWith("  C2 "));
    const event: TuiMouseEvent = { type: "click", button: "left", x: 1, y: line, screenX: 1, screenY: line, width: 140, height: 200, shift: false, alt: false, ctrl: false };
    const frame = renderLayoutFrame(root, 140, 200, () => {});
    for (const box of getLayoutBoxesAt(frame, event.screenX, event.screenY)) {
      if (box.component.handleMouse?.({ ...event, x: event.screenX - box.rect.x, y: event.screenY - box.rect.y, width: box.rect.width, height: box.rect.height })) break;
    }
    expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^▌ C2 /m);
    wb.handleInput("j"); wb.handleInput("\r");
    expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^▌ ▾ Review notes/m);
    wb.handleInput("\x1b[20~"); expect(stripTerminalSequences(lines(80).join("\n"))).toContain("REAL_TRANSCRIPT");
  } finally { wb.dispose(); }
});
