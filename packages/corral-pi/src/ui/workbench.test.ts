import { test, expect } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { ReviewStore } from "./review-store.ts";
import { readPanelFiles, readPanelDiff, parseNumstat } from "./panels-git.ts";
import { activityLines } from "./activity-view.ts";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { Text, stripTerminalSequences, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { NativeWorkbench, plainClick } from "./workbench.ts";
import type { PanelSnapshot } from "./panels-view.ts";
test("public SDK body seam switches main views while retaining the actual transcript instance and editor dock", () => {
  const wb = new NativeWorkbench({ cwd: tmpdir(), identity: { store: "s", project: "p", role: "assistant", assistant: "A-test", generation: 1 }, snapshot: () => null, changed() {}, canInteract: () => true });
  let supplied: Component | undefined;
  const viewport = createChatViewport({ document: new Text("REAL_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
    editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: new Text("REAL_FOOTER", 0, 0), mainView: transcript => { supplied = transcript; return wb.mount(transcript); } });
  try {
    expect(supplied).toBe(viewport.transcript);
    const screen = () => stripTerminalSequences(renderLayoutFrame(viewport.root, 80, 24, () => {}).lines.join("\n"));
    expect(screen()).toContain("REAL_TRANSCRIPT"); expect(screen()).toContain("REAL_EDITOR_DRAFT");
    wb.setView("activity"); expect(screen()).toContain("snapshot disconnected"); expect(screen()).not.toContain("REAL_TRANSCRIPT"); expect(screen()).toContain("REAL_EDITOR_DRAFT");
    wb.setView("conversation"); expect(screen()).toContain("REAL_TRANSCRIPT");
  } finally { wb.dispose(); }
});
test("modified/multiclick/selection gestures are not body actions", () => {
  const click: TuiMouseEvent = { type: "click", button: "left", x: 0, y: 0, screenX: 0, screenY: 0, width: 80, height: 24, shift: false, alt: false, ctrl: false, clickCount: 1 };
  expect(plainClick(click)).toBeTrue();
  for (const event of [{ ...click, shift: true }, { ...click, ctrl: true }, { ...click, alt: true }, { ...click, clickCount: 2 }, { ...click, type: "drag" as const }, { ...click, button: "right" as const }]) expect(plainClick(event)).toBeFalse();
});
function repo() {
  const root = mkdtempSync(join(tmpdir(), "native-review-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
  mkdirSync(join(root, "src")); writeFileSync(join(root, "src/a.ts"), "one\n"); git("add", "."); git("commit", "-qm", "base");
  return { root, git, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}
test("nested launch paths, staged/unstaged comparisons and new-file patches use actual Git root", async () => {
  const r = repo();
  try {
    writeFileSync(join(r.root, "src/a.ts"), "two\n"); r.git("add", "."); writeFileSync(join(r.root, "src/a.ts"), "three\n");
    writeFileSync(join(r.root, "src/new.ts"), "new content\n");
    const files = await readPanelFiles(join(r.root, "src"));
    const tracked = files.find(file => file.path === "src/a.ts"), fresh = files.find(file => file.path === "src/new.ts");
    if (!tracked || !fresh) throw new Error("missing fixture changes");
    const before = r.git("status", "--porcelain=v1");
    expect(await readPanelDiff(join(r.root, "src"), tracked)).toContain("+three");
    const staged = await readPanelDiff(r.root, tracked, undefined, "staged"); expect(staged).toContain("+two"); expect(staged).not.toContain("+three");
    const unstaged = await readPanelDiff(r.root, tracked, undefined, "unstaged"); expect(unstaged).toContain("-two"); expect(unstaged).toContain("+three");
    expect(await readPanelDiff(r.root, fresh)).toContain("+new content");
    expect(r.git("status", "--porcelain=v1")).toBe(before);
  } finally { r.cleanup(); }
});
test("review refresh detects external edits without changing selected path or filter", async () => {
  const r = repo(), store = new ReviewStore(join(r.root, "src"), () => {});
  try {
    writeFileSync(join(r.root, "src/a.ts"), "first\n"); await store.refresh();
    expect(store.root).toBe(r.root); expect(store.selected).toBe("src/a.ts"); expect(store.patch).toContain("+first");
    store.query = "a.ts"; writeFileSync(join(r.root, "src/a.ts"), "second\n"); await store.refresh();
    expect(store.query).toBe("a.ts"); expect(store.selected).toBe("src/a.ts"); expect(store.patch).toContain("+second"); expect(store.checkedAt).toBeDefined();
    await store.setComparison("staged"); expect(store.visible).toHaveLength(0);
    await store.setComparison("unstaged"); expect(store.visible).toHaveLength(1);
  } finally { store.dispose(); r.cleanup(); }
});
test("patch provenance captures scope and base atomically through overlapping comparison refresh", async () => {
  const r = repo(), store = new ReviewStore(r.root, () => {});
  try {
    writeFileSync(join(r.root, "src/a.ts"), "two\n"); r.git("add", "."); writeFileSync(join(r.root, "src/a.ts"), "three\n");
    await store.refresh(); const first = store.patchSnapshot;
    expect(first?.comparison).toBe("all"); expect(first?.text).toContain("-one");
    const old = store.refresh(); const next = store.setComparison("unstaged"); await old; await next;
    expect(store.patchSnapshot?.comparison).toBe("unstaged"); expect(store.patchSnapshot?.text).toContain("-two");
    expect(first?.comparison).toBe("all"); expect(first?.text).toContain("-one"); // Prior snapshot never relabeled.
    const base = first?.head; if (!base) throw new Error("missing captured HEAD");
    r.git("add", "."); r.git("commit", "-qm", "new HEAD"); writeFileSync(join(r.root, "src/a.ts"), "four\n");
    const file = (await readPanelFiles(r.root))[0]; if (!file) throw new Error("missing changed file");
    expect(await readPanelDiff(r.root, file, undefined, "all", base)).toContain("-one");
  } finally { store.dispose(); r.cleanup(); }
});
test("numstat retains binary/rename identities without fabricated numeric zero", () => {
  const rows = parseNumstat("2\t3\tsrc/a.ts\0-\t-\timage.png\0" + "1\t0\t\0old.ts\0new.ts\0");
  expect(rows.get("src/a.ts")).toEqual({ added: 2, removed: 3 });
  expect(rows.get("image.png")).toEqual({ added: null, removed: null });
  expect(rows.get("new.ts")).toEqual({ added: 1, removed: 0 });
});
test("activity is explicit about unavailable data and local monitor observations, not canonical success", () => {
  expect(activityLines(null, 0).join("\n")).toContain("unavailable");
  const snapshot: PanelSnapshot = { bound: true, model: "fixture", effort: "high", streaming: false, context: undefined, tokens: 0,
    tools: [], contextFiles: [], skills: [], ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null,
    jobs: [{ job: "j1-1", state: "running", detail: { job: "j1-1", command: "build", state: "running", startedAt: 1000, endedAt: undefined,
      exitCode: null, signal: null, groupClear: false, bytes: 30, truncated: false, tail: "READY\n", monitorAttempts: 2,
      watches: [{ wait: "w1", pattern: "READY", armed: true, matched: "READY", reportQueued: true, deadline: new Date(5000).toISOString() }] } }] };
  const text = activityLines(snapshot, 0, 3000).join("\n");
  expect(text).toContain("2s elapsed"); expect(text).toContain("match observed"); expect(text).toContain("settlement unknown");
  expect(text).toContain("2 attempts (not active-wait count)"); expect(text).not.toContain("100%");
  expect(activityLines({ ...snapshot, jobs: null }, 0).join("\n")).toContain("runtime unavailable");
});
