import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripTerminalSequences, Text, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { TaskStore } from "./task-store.ts";
import { renderProof, SPLIT_MIN } from "./proof-view.ts";
import { proofItems, type SectionId } from "./proof-parts.ts";
import { ReviewStore } from "./review-store.ts";
import { NativeWorkbench } from "./workbench.ts";
import { parseChangedPaths } from "./candidate-git.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
/** One `corral show --json` read of T-001 with run R1, a passing gate, two review rounds and one artifact. */
function data(run: Record<string, unknown> = {}) {
  return {
    task: { id: "T-001", title: "Fixture", body: "## Goal\nShip it.", status: "review", priority: "P2" },
    runs: [{ id: "R1", role: "worker", profile: "pi", worktree: "/repo", branch: "corral/T-001", base_sha: "a".repeat(40),
      started: "2026-10-14T12:00:00Z", summary: "worker says complete", ...run }],
    checks: [
      { kind: "gate", key: "b".repeat(40), result: "pass", notes: null, log: "T-001:2", by: "auto", at: "2026-10-14T12:10:00Z" },
      { kind: "review", key: "0", result: "fail", notes: "Fix the parser", by: "reviewer", at: "2026-10-14T12:20:00Z" },
      { kind: "review", key: "1", result: "pass", notes: "Looks right now", by: "reviewer", at: "2026-10-14T12:30:00Z" },
    ],
    artifacts: [{ task: "T-001", id: 1, sha256: "c".repeat(64), bytes: 12, kind: "report", title: "Proof log", at: "2026-10-14T12:05:00Z", by: "agent" }],
  };
}
const OPEN = new Set<SectionId>(["gate", "notes", "findings", "artifacts", "summary"]);
/** Every Proof section a reader can reach, each selected in turn and expanded. */
function text(store: TaskStore, width = 120): string {
  const count = Math.max(1, proofItems(store.current, identity.run).length);
  return Array.from({ length: count }, (_, i) => renderProof(store, identity.run, i, width, false, OPEN).lines).flat().map(line => stripTerminalSequences(line)).join("\n");
}

test("Proof shows the newest gate and review, every round, the artifacts and the summary; rendering never reads", async () => {
  let reads = 0;
  const store = new TaskStore(identity, async () => { reads++; return data(); }, () => {});
  await store.refresh();
  for (const width of [140, 52]) {
    const out = text(store, width);
    for (const part of ["gate ✓ pass · bbbbbbbb", "review ✓ approve · round 1", "aaaaaaaa → worktree", "Fix the parser", "Looks right now",
      "T-001:1 Proof log", "worker says complete", "T-001:2"]) expect(out).toContain(part);
    for (const row of text(store, width).split("\n")) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
  }
  const calls = reads; text(store); text(store, 52); await store.refresh();
  expect(reads).toBe(calls);
  store.dispose();
});

test("overlapping invalidation, another task's read and a malformed read fail closed; a failed reread keeps the proof", async () => {
  let release!: (value: unknown) => void;
  const store = new TaskStore(identity, () => new Promise(resolve => { release = resolve; }), () => {});
  const old = store.refresh(); store.invalidate(); release(data()); await old;
  expect(store.current).toBeUndefined(); expect(store.stale).toBeTrue();
  const other = store.refresh(); release({ ...data(), task: { ...data().task, id: "T-2" } }); await other;
  expect(store.error).toContain("identity mismatch");
  const good = store.refresh(); release(data()); await good;
  expect(store.state.phase).toBe("current");
  const malformed = store.refresh(true); release({ ...data(), checks: [{ kind: "gate" }] }); await malformed;
  expect(store.state.phase).toBe("retained"); expect(store.current?.task.id).toBe("T-001");
  expect(text(store)).toContain("worker says complete");
  store.dispose();
});

test("committed candidate files, binary/rename diffstats and captured hunks ignore later HEAD and dirty edits", async () => {
  const root = mkdtempSync(join(tmpdir(), "candidate-git-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" }).trim();
  const candidate = { task: "T-001", run: "R1", root, base: "", head: "" };
  const store = new ReviewStore(root, () => {}, async () => candidate);
  try {
    git("init", "-qb", "main"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "old name.ts"), "base\n"); git("add", "."); git("commit", "-qm", "base");
    candidate.base = git("rev-parse", "HEAD");
    git("mv", "old name.ts", "renamed name.ts"); writeFileSync(join(root, "added.ts"), "committed line\n");
    writeFileSync(join(root, "binary.dat"), new Uint8Array([0, 1, 2])); git("add", "."); git("commit", "-qm", "candidate");
    candidate.head = git("rev-parse", "HEAD");
    writeFileSync(join(root, "added.ts"), "dirty worktree\n");
    await store.refresh(); expect(store.comparison).toBe("candidate");
    expect(store.files.map(f => f.path)).toEqual(["added.ts", "binary.dat", "renamed name.ts"]);
    expect(store.stats.get("binary.dat")).toEqual({ added: null, removed: null });
    expect(store.files.find(f => f.path === "renamed name.ts")?.originalPath).toBe("old name.ts");
    await store.select("added.ts"); expect(store.patch).toContain("+committed line"); expect(store.patch).not.toContain("dirty worktree");
    const captured = store.patchSnapshot;
    git("add", "."); git("commit", "-qm", "later HEAD");
    await store.select("added.ts"); expect(store.patchSnapshot?.head).toBe(captured?.head); expect(store.patch).toBe(captured?.text);
    expect(store.patchSnapshot?.base).toBe(candidate.base);
    // Through the workbench: the bound run's worktree and commit range from `corral show`; without head_sha the worktree's HEAD.
    const wb = new NativeWorkbench({ cwd: root, identity, snapshot: () => null, changed() {}, canInteract: () => true,
      readTask: async () => data({ worktree: root, base_sha: candidate.base }) });
    try {
      const component = wb.mount(new Text("transcript", 0, 0)); wb.setView("changes"); await wb.review.refresh(); await wb.openFile("added.ts");
      const head = git("rev-parse", "HEAD");
      const visible = stripTerminalSequences(renderLayoutFrame(component, 80, 50, () => {}).lines.join("\n"));
      expect(visible).toContain(candidate.base.slice(0, 8)); expect(visible).toContain(head.slice(0, 8));
      expect(wb.review.patchSnapshot?.base).toBe(candidate.base); expect(wb.review.patchSnapshot?.head).toBe(head);
      expect(visible).toMatch(/\d \+dirty worktree/);
    } finally { wb.dispose(); }
    expect(parseChangedPaths("M\0literal\n界面.ts\0")[0]?.path).toBe("literal\n界面.ts");
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("a new candidate head shows only once its patch is applied", async () => {
  const root = mkdtempSync(join(tmpdir(), "candidate-refresh-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" }).trim();
  let current = data();
  const wb = new NativeWorkbench({ cwd: root, identity, snapshot: () => null, changed() {}, canInteract: () => true, readTask: async () => current });
  const component = wb.mount(new Text("transcript", 0, 0));
  const render = (width: number) => stripTerminalSequences(renderLayoutFrame(component, width, 50, () => {}).lines.join("\n"));
  try {
    git("init", "-qb", "main"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "a.txt"), "base\n"); git("add", "."); git("commit", "-qm", "base");
    const base = git("rev-parse", "HEAD");
    writeFileSync(join(root, "a.txt"), "first\n"); git("add", "."); git("commit", "-qm", "candidate A");
    const first = git("rev-parse", "HEAD");
    current = data({ worktree: root, base_sha: base, head_sha: first });
    wb.setView("changes"); await wb.review.refresh(); await wb.openFile("a.txt"); await wb.review.refresh();
    expect(render(80)).toContain(first.slice(0, 8)); expect(render(80)).toMatch(/\d \+first/);
    writeFileSync(join(root, "a.txt"), "second\n"); git("add", "."); git("commit", "-qm", "candidate B");
    const second = git("rev-parse", "HEAD");
    current = data({ worktree: root, base_sha: base, head_sha: second });
    wb.task.invalidate(); await wb.review.refresh();
    expect(wb.review.head).toBe(second);
    const retained = render(80);
    expect(retained).toContain(first.slice(0, 8)); expect(retained).not.toContain(second.slice(0, 8)); expect(retained).toContain("New diff available");
    expect(wb.handleInput("r")).toBeTrue(); await wb.review.refresh();
    const applied = render(80);
    expect(applied).toContain(second.slice(0, 8)); expect(applied).not.toContain("New diff available"); expect(applied).toMatch(/\d \+second/);
  } finally { wb.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("Proof keyboard/pointer navigation survives narrow/wide rendering and preserves the editor transcript", async () => {
  const wb = new NativeWorkbench({ cwd: tmpdir(), identity, snapshot: () => null, changed() {}, canInteract: () => true, readTask: async () => data() });
  const root = wb.mount(new Text("REAL_TRANSCRIPT", 0, 0));
  const lines = (width: number) => renderLayoutFrame(root, width, 200, () => {}).lines;
  try {
    wb.handleInput("\x1b[21~"); await wb.task.refresh(true);
    expect(wb.view).toBe("evidence");
    for (const width of [52, 80, 140]) for (const row of lines(width)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    // Proof splits into list and detail from SPLIT_MIN main-view columns.
    expect(stripTerminalSequences(lines(SPLIT_MIN).join("\n"))).toContain("┬");
    expect(stripTerminalSequences(lines(SPLIT_MIN - 1).join("\n"))).not.toContain("┬");
    expect(stripTerminalSequences(lines(SPLIT_MIN - 1).join("\n"))).toMatch(/‹ PROOF 1\/5 ›/);
    wb.handleInput("j"); expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^ ▌ ▸ Review notes/m);
    wb.handleInput("k"); expect(stripTerminalSequences(lines(52).join("\n"))).toMatch(/^  ‹ PROOF 1\/5 › /m);
    const wide = lines(140).map(row => stripTerminalSequences(row));
    // Key hints live in the footer, never in the Proof content.
    expect(wide.some(row => row.includes("j/k section"))).toBe(false);
    const line = wide.findIndex(row => row.startsWith("   ▸ Review findings"));
    const event: TuiMouseEvent = { type: "click", button: "left", x: 2, y: line, screenX: 2, screenY: line, width: 140, height: 200, shift: false, alt: false, ctrl: false };
    const frame = renderLayoutFrame(root, 140, 200, () => {});
    for (const box of getLayoutBoxesAt(frame, event.screenX, event.screenY)) {
      if (box.component.handleMouse?.({ ...event, x: event.screenX - box.rect.x, y: event.screenY - box.rect.y, width: box.rect.width, height: box.rect.height })) break;
    }
    expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^ ▌ ▸ Review findings/m);
    wb.handleInput("\r");
    expect(stripTerminalSequences(lines(140).join("\n"))).toMatch(/^ ▌ ▾ Review findings/m);
    expect(stripTerminalSequences(lines(140).join("\n"))).toContain("Fix the parser");
    wb.handleInput("\x1b[20~"); expect(stripTerminalSequences(lines(80).join("\n"))).toContain("REAL_TRANSCRIPT");
  } finally { wb.dispose(); }
});
