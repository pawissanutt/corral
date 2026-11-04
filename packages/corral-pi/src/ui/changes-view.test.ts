import { test, expect } from "bun:test";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { ChangesView, changeTree, changesModel, diffLines, miniBar, type ChangesModel } from "./changes-view.ts";
import { ReviewStore, comparisonLabel } from "./review-store.ts";

const PATCH = [
  "diff --git a/src/a/one.rs b/src/a/one.rs", "--- a/src/a/one.rs", "+++ b/src/a/one.rs",
  "@@ -10,3 +10,4 @@ fn first() {", " keep", "-old", "+new", "+more", " tail",
  "@@ -40,2 +41,2 @@ fn second()", "-gone", "+came", " end", "",
].join("\n");
const MODEL: ChangesModel = {
  comparison: "candidate", range: "aaaaaaaa → bbbbbbbb", state: { text: "committed · clean", tone: "ok" }, scope: "repo @ task/T-1",
  updated: "updated 1s ago", query: "", notes: [], hunk: 1, selected: "src/a/one.rs",
  files: [{ path: "src/a/one.rs", letter: "M", added: 3, removed: 2 }, { path: "src/a/two.rs", letter: "A", added: 1, removed: 0 },
    { path: "README.md", letter: "M", added: 1, removed: 1 }],
  others: [{ label: "working tree", count: 0 }, { label: "staged", count: 0 }],
  diff: { kind: "patch", path: "src/a/one.rs", status: "M", text: PATCH },
};
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
function frame(width: number): string[] {
  const view = new ChangesView({ model: () => MODEL, click: () => false, focus() {}, select() {}, step() {}, pickHunk() {} });
  renderLayoutFrame(view.root, width, 20, () => {});
  return renderLayoutFrame(view.root, width, 20, () => {}).lines;
}

test("tree groups files by directory in path order, root files under ./", () => {
  expect(changeTree(MODEL.files).map(group => [group.dir, group.files.map(file => file.path)])).toEqual([
    ["src/a/", ["src/a/one.rs", "src/a/two.rs"]], ["./", ["README.md"]]]);
});

test("mini bar scales to the largest file and keeps one cell for any non-zero side", () => {
  expect(miniBar(15, 3, 18)).toEqual({ plus: 5, minus: 1 });
  expect(miniBar(3, 1, 18)).toEqual({ plus: 1, minus: 1 });
  expect(miniBar(0, 0, 18)).toEqual({ plus: 0, minus: 0 });
});

test("diff rows carry old/new gutter numbers and hunk bars carry n/m, with sources at the @@ lines", () => {
  const result = diffLines(MODEL, 90), rows = plain(result.lines);
  const cells = (text: string) => rows.find(row => row.includes(text))?.split("│")[0]?.replace(/^[▏ ]/, "").trim().split(/\s+/);
  expect(cells("keep")).toEqual(["10", "10"]);
  expect(cells("old")).toEqual(["11"]);
  expect(cells("more")).toEqual(["12"]);
  expect(cells("tail")).toEqual(["12", "13"]);
  expect(cells("end")).toEqual(["41", "42"]);
  expect(result.hunkRows.map(row => rows[row]?.match(/hunk (\d+)\/(\d+)/)?.slice(1))).toEqual([["1", "2"], ["2", "2"]]);
  expect(result.hunkSources.map(index => PATCH.split("\n")[index]?.startsWith("@@"))).toEqual([true, true]);
  // The focus edge follows the selected hunk (index 1), not the first one.
  expect(rows[result.hunkRows[1] ?? -1]?.startsWith("▌")).toBeTrue();
  expect(rows[result.hunkRows[0] ?? -1]?.startsWith("▌")).toBeFalse();
});

test("at >=105 cols the tree sits beside the diff; below it a one-line picker replaces the tree", () => {
  const wide = plain(frame(140));
  expect(wide.some(row => row.includes("src/a/") && row.includes("│"))).toBeTrue();
  expect(wide.some(row => row.includes("‹ 1/3 ›"))).toBeFalse();
  for (const width of [80, 52]) {
    const narrow = plain(frame(width));
    expect(narrow[1]).toContain("‹ 1/3 ›");
    expect(narrow.some(row => row.trim() === "src/a/")).toBeFalse();
  }
  for (const width of [140, 104, 80, 52, 30]) for (const line of frame(width)) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
});

test("other-comparison counts come from the worktree status, not the current comparison's file list", async () => {
  const root = mkdtempSync(join(tmpdir(), "changes-view-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  const store = new ReviewStore(root, () => {});
  try {
    git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "a.txt"), "one\n"); writeFileSync(join(root, "b.txt"), "one\n"); git("add", "."); git("commit", "-qm", "base");
    writeFileSync(join(root, "a.txt"), "two\n"); writeFileSync(join(root, "b.txt"), "two\n"); git("add", "b.txt");
    await store.refresh();
    const snap = store.patchSnapshot;
    const model = changesModel(store, { root: snap?.root, path: snap?.path, comparison: comparisonLabel(store.comparison), text: snap?.text ?? "",
      head: snap?.head, base: snap?.base, file: snap?.file }, "fixture", 0, Date.now());
    expect(model.others).toEqual([{ label: "unstaged", count: 1 }, { label: "staged", count: 1 }]);
    expect(model.files.map(file => file.letter)).toEqual(["M", "M"]);
    expect(model.diff.kind).toBe("patch");
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});
