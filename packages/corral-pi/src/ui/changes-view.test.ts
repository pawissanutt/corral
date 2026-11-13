import { test, expect } from "bun:test";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { ChangesView, LIST_WIDTH, changesModel, diffLines, filterRow, listLines, type ChangeAuthor, type ChangesHost, type ChangesModel } from "./changes-view.ts";
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
  total: 3, authors: [], filter: "all",
  others: [{ label: "working tree", count: 0 }, { label: "staged", count: 0 }],
  diff: { kind: "patch", path: "src/a/one.rs", status: "M", text: PATCH },
};
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const host = (model: () => ChangesModel, extra: Partial<ChangesHost> = {}): ChangesHost =>
  ({ model, click: () => false, focus() {}, select() {}, step() {}, pickHunk() {}, ...extra });
function frame(width: number, view = new ChangesView(host(() => MODEL)), height = 20): string[] {
  renderLayoutFrame(view.root, width, height, () => {});
  return renderLayoutFrame(view.root, width, height, () => {}).lines;
}
const gutterCells = (rows: readonly string[], text: string) => rows.find(row => row.includes(text))?.split("│")[0]?.replace(/^[▏ ]/, "").trim().split(/\s+/);

test("diff rows carry old/new gutter numbers and hunk bars carry n/m, with sources at the @@ lines", () => {
  const result = diffLines(MODEL, 90), rows = plain(result.lines);
  expect(gutterCells(rows, "keep")).toEqual(["10", "10"]);
  expect(gutterCells(rows, "old")).toEqual(["11"]);
  expect(gutterCells(rows, "more")).toEqual(["12"]);
  expect(gutterCells(rows, "tail")).toEqual(["12", "13"]);
  expect(gutterCells(rows, "end")).toEqual(["41", "42"]);
  expect(result.hunkRows.map(row => rows[row]?.match(/hunk (\d+)\/(\d+)/)?.slice(1))).toEqual([["1", "2"], ["2", "2"]]);
  expect(result.hunkSources.map(index => PATCH.split("\n")[index]?.startsWith("@@"))).toEqual([true, true]);
  // The focus edge follows the selected hunk (index 1), not the first one.
  expect(rows[result.hunkRows[1] ?? -1]?.startsWith("▌")).toBeTrue();
  expect(rows[result.hunkRows[0] ?? -1]?.startsWith("▌")).toBeFalse();
});

test("below 80 columns the diff keeps one gutter: new line numbers, and the old number on removed lines", () => {
  const rows = plain(diffLines(MODEL, 60).lines);
  expect(gutterCells(rows, "keep")).toEqual(["10"]);
  expect(gutterCells(rows, "old")).toEqual(["11"]);
  expect(gutterCells(rows, "new")).toEqual(["11"]);
  expect(gutterCells(rows, "more")).toEqual(["12"]);
  expect(gutterCells(rows, "tail")).toEqual(["13"]);
  expect(gutterCells(rows, "gone")).toEqual(["40"]);
  expect(gutterCells(rows, "end")).toEqual(["42"]);
});

test("at 90+ columns the list sits beside the diff; below 90 the list stacks above it and keeps the selected file in view", () => {
  for (const width of [140, 90]) {
    const rows = plain(frame(width)), selected = rows.find(row => row.startsWith("▌ M src/a/one.rs"));
    expect(selected?.indexOf("│")).toBe(LIST_WIDTH - 1);
    expect(rows.some(row => row.startsWith("─ "))).toBeFalse();
  }
  for (const width of [89, 52]) {
    // Below 60 columns paths keep their last two segments.
    const rows = plain(frame(width)), list = rows.findIndex(row => row.startsWith(width < 60 ? "▌ M a/one.rs" : "▌ M src/a/one.rs"));
    const rule = rows.findIndex(row => row.startsWith("─ 3 files +5 −3")), header = rows.findIndex(row => row.includes("modified"));
    expect(list).toBe(1);
    expect(rows[list]).not.toContain("│");
    expect(rule).toBe(4);
    expect(header).toBeGreaterThan(rule);
  }
  // Twenty files, three list rows on a very short terminal: the selection is shown wherever it sits.
  const files = Array.from({ length: 20 }, (_, i) => ({ path: `src/deep/f${String(i).padStart(2, "0")}.rs`, letter: "M", added: i, removed: 1 }));
  let selected = "src/deep/f15.rs";
  const view = new ChangesView(host(() => ({ ...MODEL, files, total: 20, selected }), { rows: () => 14 }));
  for (const [path, range] of [["src/deep/f15.rs", "14–16/20"], ["src/deep/f00.rs", "1–3/20"], ["src/deep/f19.rs", "18–20/20"]] as const) {
    selected = path;
    const rows = plain(frame(52, view)), rule = rows.findIndex(row => row.startsWith("─ 20 files"));
    expect(rule).toBe(4);
    expect(rows.slice(1, rule).some(row => row.startsWith("▌ M deep/" + path.slice(9)))).toBeTrue();
    expect(rows[rule]).toContain(range);
  }
});

test("every Changes row stays within the pane, with and without authors", () => {
  const authors: ChangeAuthor[] = [
    { id: "slots", label: "host-slots-with-a-long-name", hue: 0, paths: new Map([["src/a/one.rs", false], ["README.md", true]]) },
    { id: "ui", label: "picker-ui", hue: 1, paths: new Map([["src/a/two.rs", false], ["README.md", false]]) },
  ];
  const withAuthors: ChangesModel = { ...MODEL,
    files: [{ ...MODEL.files[0]!, who: ["slots"] }, { ...MODEL.files[1]!, who: ["ui"] }, { ...MODEL.files[2]!, who: ["slots", "ui"], outside: ["slots"] }],
    authors: [{ id: "slots", label: authors[0]!.label, hue: 0, count: 2 }, { id: "ui", label: "picker-ui", hue: 1, count: 2 }] };
  const plainView = () => new ChangesView(host(() => MODEL));
  const authored = () => new ChangesView(host(() => withAuthors, { authors: () => authors, color: () => "\x1b[35m" }));
  for (const make of [plainView, authored]) for (const width of [34, 52, 89, 90, 140]) for (const height of [12, 20, 40]) {
    for (const row of frame(width, make(), height)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
  }
  for (const width of [10, 34, 52]) expect(visibleWidth(filterRow(withAuthors, width, () => "").line)).toBeLessThanOrEqual(width);
});

test("an author filter shows only that author's files and marks edits outside its paths", async () => {
  const root = mkdtempSync(join(tmpdir(), "changes-authors-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  const store = new ReviewStore(root, () => {});
  const authors: ChangeAuthor[] = [
    { id: "slots", label: "host-slots", hue: 0, paths: new Map([["a.txt", false], ["c.txt", true]]) },
    { id: "ui", label: "picker-ui", hue: 1, paths: new Map([["b.txt", false], ["c.txt", false]]) },
  ];
  try {
    git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    for (const name of ["a.txt", "b.txt", "c.txt"]) writeFileSync(join(root, name), "one\n");
    git("add", "."); git("commit", "-qm", "base");
    for (const name of ["a.txt", "b.txt", "c.txt"]) writeFileSync(join(root, name), "two\n");
    await store.refresh();
    const selections: string[] = [];
    const shown = { root, path: store.selected, comparison: comparisonLabel(store.comparison), text: "", head: store.head, base: undefined, file: undefined };
    const model = (filter: string, named = authors) => changesModel(store, shown, "fixture", 0, Date.now(), named, filter);
    // The reader had b.txt selected.
    const view: ChangesView = new ChangesView(host(() => ({ ...model(view.filter), selected: "b.txt" }),
      { authors: () => authors, select: path => { selections.push(path); } }));

    const all = model("all");
    expect(all.files.map(file => file.path)).toEqual(["a.txt", "b.txt", "c.txt"]);
    expect(all.authors.map(author => [author.label, author.count])).toEqual([["host-slots", 2], ["picker-ui", 2]]);
    const row = filterRow(all, 140, () => ""), line = stripTerminalSequences(row.line);
    const chips: Record<string, string> = { all: "all 3", slots: "host-slots 2", ui: "picker-ui 2" };
    expect(row.hits.map(hit => line.slice(hit.from, hit.to))).toEqual(row.hits.map(hit => chips[hit.id]));
    expect(row.hits.map(hit => hit.id)).toEqual(["all", "slots", "ui"]);

    view.cycleFilter();
    expect(view.filter).toBe("slots");
    // The filter hides the selected b.txt, so the selection moves to the first shown file.
    expect(selections).toEqual(["a.txt"]);
    const slots = model("slots");
    expect(slots.files.map(file => file.path)).toEqual(["a.txt", "c.txt"]);
    expect(slots.total).toBe(3);
    const rows = plain(frame(140, view));
    expect(rows[1]).toContain("host-slots 2");
    expect(rows.some(r => /^[▌ ] M .*b\.txt/.test(r.slice(0, LIST_WIDTH)))).toBeFalse();
    expect(rows.some(r => /^  M ● a\.txt/.test(r) && !r.slice(0, LIST_WIDTH).includes("!"))).toBeTrue();
    const c = rows.findIndex(r => /^  M ●● ! c\.txt/.test(r));
    expect(c).toBeGreaterThan(0);
    expect(rows[c + 1]).toContain("also edited by host-slots");

    view.cycleFilter(); view.cycleFilter();
    expect(view.filter).toBe("all");
    // Two authors inside their own paths: the file is flagged in `all`, and each author's filter still narrows to its files.
    const shared: ChangeAuthor[] = [
      { id: "slots", label: "host-slots", hue: 0, paths: new Map([["a.txt", false], ["c.txt", false]]) },
      { id: "ui", label: "picker-ui", hue: 1, paths: new Map([["b.txt", false], ["c.txt", false]]) },
    ];
    expect(model("ui", shared).files.map(file => file.path)).toEqual(["b.txt", "c.txt"]);
    const listed = plain(listLines(model("all", shared), 80, { stacked: true }).lines);
    const both = listed.findIndex(r => r.includes("c.txt"));
    expect(listed[both]).toContain("!");
    expect(listed[both + 1]).toContain("edited by host-slots and picker-ui");
    expect(listed[both + 1]).not.toContain("also");
    const alone = listed.findIndex(r => r.includes("a.txt"));
    expect(listed[alone]).not.toContain("!");
    expect(listed[alone + 1]).not.toContain("edited by");
    // Hosts that name no authors get no filter row and no author marks.
    expect(model("slots", []).files.map(file => [file.path, file.who, file.outside]))
      .toEqual([["a.txt", undefined, undefined], ["b.txt", undefined, undefined], ["c.txt", undefined, undefined]]);
    expect(plain(frame(140, new ChangesView(host(() => model("all", [])))))[1]?.startsWith(" 3 files")).toBeTrue();
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
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
    expect(model.others).toEqual([{ label: "working tree", count: 2 }, { label: "unstaged", count: 1 }, { label: "staged", count: 1 }]);
    expect(model.files.map(file => file.letter)).toEqual(["M", "M"]);
    expect(model.diff.kind).toBe("patch");
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});
