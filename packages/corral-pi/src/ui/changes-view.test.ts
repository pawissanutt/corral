import { test, expect } from "bun:test";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { ChangesView, LIST_WIDTH, authorChips, changesModel, diffLines, emptyChanges, listLines, type ChangeAuthor, type ChangesHost, type ChangesModel } from "./changes-view.ts";
import { ReviewStore, comparisonLabel } from "./review-store.ts";
import { PANEL_DIALOG } from "./panels-view.ts";

const PATCH = [
  "diff --git a/src/a/one.rs b/src/a/one.rs", "--- a/src/a/one.rs", "+++ b/src/a/one.rs",
  "@@ -10,3 +10,4 @@ fn first() {", " keep", "-old", "+new", "+more", " tail",
  "@@ -40,2 +41,2 @@ fn second()", "-gone", "+came", " end", "",
].join("\n");
const MODEL: ChangesModel = {
  comparison: "since session", custom: false, range: "aaaaaaaa → worktree", state: { text: "commits + worktree", tone: "muted" }, scope: "repo @ main",
  updated: "updated 1s ago", query: "", notes: [], hunk: 1, selected: "src/a/one.rs",
  files: [{ path: "src/a/one.rs", letter: "M", added: 3, removed: 2 }, { path: "src/a/two.rs", letter: "A", added: 1, removed: 0 },
    { path: "README.md", letter: "M", added: 1, removed: 1 }],
  total: 3, authors: [], filter: "all",
  diff: { kind: "patch", path: "src/a/one.rs", status: "M", text: PATCH },
};
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const host = (model: () => ChangesModel, extra: Partial<ChangesHost> = {}): ChangesHost =>
  ({ model, click: () => false, focus() {}, select() {}, step() {}, pickHunk() {}, ...extra });
function frame(width: number, view = new ChangesView(host(() => MODEL)), height = 20): string[] {
  renderLayoutFrame(view.root, width, height, () => {});
  return renderLayoutFrame(view.root, width, height, () => {}).lines;
}
/** A diff row's cells after the margin: the four-cell line number, the sign, and the text. */
const cells = (rows: readonly string[], text: string) => {
  const row = rows.find(r => r.includes(text)) ?? "";
  return { number: row.slice(1, 5).trim(), sign: row.slice(6, 7) };
};

test("the diff draws the bold header, hunk rows on the hunk surface and numbered rows on their own surfaces", () => {
  const result = diffLines(MODEL, 90), rows = plain(result.lines);
  for (const row of result.lines) expect(visibleWidth(row)).toBe(90);
  expect(rows[0]?.trim()).toBe("src/a/one.rs");
  expect(cells(rows, "keep")).toEqual({ number: "10", sign: " " });
  expect(cells(rows, "old")).toEqual({ number: "", sign: "−" });
  expect(cells(rows, "new")).toEqual({ number: "11", sign: "+" });
  expect(cells(rows, "more")).toEqual({ number: "12", sign: "+" });
  expect(cells(rows, "tail")).toEqual({ number: "13", sign: " " });
  expect(cells(rows, "gone")).toEqual({ number: "", sign: "−" });
  expect(cells(rows, "came")).toEqual({ number: "41", sign: "+" });
  expect(cells(rows, "end")).toEqual({ number: "42", sign: " " });
  expect(rows.some(row => row.includes("│") || row.includes("---") || row.includes("diff --git"))).toBeFalse();
  expect(result.hunkRows.map(row => rows[row]?.trim().split(" @@")[0])).toEqual(["@@ -10,3 +10,4", "@@ -40,2 +41,2"]);
  expect(result.hunkRows.every(row => result.lines[row]?.includes(PANEL_DIALOG))).toBeTrue();
  expect(result.hunkSources.map(index => PATCH.split("\n")[index]?.startsWith("@@"))).toEqual([true, true]);
  // Only the current hunk (index 1) carries its place among the hunks.
  expect(rows[result.hunkRows[1] ?? -1]?.trimEnd().endsWith("2/2")).toBeTrue();
  expect(rows[result.hunkRows[0] ?? -1]).not.toContain("/2");
  // Added and removed rows sit on their own surfaces after the canvas margin; context rows stay on the canvas.
  const surface = (text: string) => [...(result.lines[rows.findIndex(row => row.includes(text))] ?? "").matchAll(/\x1b\[48;2;[\d;]+m/g)].map(match => match[0])[1];
  expect(new Set([surface("keep"), surface("more"), surface("old"), PANEL_DIALOG]).size).toBe(4);
  expect(surface("came")).toBe(surface("more"));
  expect(surface("gone")).toBe(surface("old"));
});

test("a new file's hunk reads `@@ new file` and its rows number from 1", () => {
  const model: ChangesModel = { ...MODEL, hunk: 0, diff: { kind: "patch", path: "src/b.rs", status: "?", text: "diff --git a/src/b.rs b/src/b.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/b.rs\n@@ -0,0 +1,2 @@\n+alpha\n+beta\n" } };
  const result = diffLines(model, 60), rows = plain(result.lines);
  expect(rows[result.hunkRows[0] ?? -1]?.trim()).toBe("@@ new file");
  expect(rows.some(row => row.includes("new file mode"))).toBeFalse();
  expect(cells(rows, "alpha")).toEqual({ number: "1", sign: "+" });
  expect(cells(rows, "beta")).toEqual({ number: "2", sign: "+" });
});

test("at 90+ columns the file column sits beside the diff; below 90 the files stack above a rule and the diff", () => {
  for (const width of [140, 90]) {
    const rows = plain(frame(width));
    for (const row of rows) expect(row[LIST_WIDTH - 1]).toBe("│");
    // Margin, bar, path; counts end two cells before the rule.
    expect(rows[0]?.startsWith(" ▌ src/a/one.rs")).toBeTrue();
    expect(rows[0]?.slice(0, LIST_WIDTH - 2).trimEnd().endsWith("+3 −2")).toBeTrue();
    expect(rows[0]?.slice(LIST_WIDTH + 1).startsWith("src/a/one.rs")).toBeTrue();
    expect(rows.some(row => /files|other comparisons|since session/.test(row))).toBeFalse();
  }
  for (const width of [89, 52]) {
    const rows = plain(frame(width)), path = width < 60 ? "a/one.rs" : "src/a/one.rs";
    expect(rows[0]?.startsWith(" ▌ " + path)).toBeTrue();
    expect(rows[3]?.trim()).toMatch(/^─+$/);
    expect(rows[4]?.trim()).toBe(path);
    expect(rows.some(row => row.includes("│"))).toBeFalse();
  }
  // Twenty files, three rows on a very short terminal: the selection is shown wherever it sits.
  const files = Array.from({ length: 20 }, (_, i) => ({ path: `src/deep/f${String(i).padStart(2, "0")}.rs`, letter: "M", added: i, removed: 1 }));
  let selected = "src/deep/f15.rs";
  const view = new ChangesView(host(() => ({ ...MODEL, files, total: 20, selected }), { rows: () => 14 }));
  for (const [path, range] of [["src/deep/f15.rs", "14–16/20"], ["src/deep/f00.rs", "1–3/20"], ["src/deep/f19.rs", "18–20/20"]] as const) {
    selected = path;
    const rows = plain(frame(52, view));
    expect(rows.slice(0, 3).some(row => row.startsWith(" ▌ deep/" + path.slice(9)))).toBeTrue();
    expect(rows[3]?.trim().startsWith("─")).toBeTrue();
    expect(rows[3]).toContain(range);
  }
});

test("the comparison row shows for a candidate or a non-default comparison, not for the session's default worktree", () => {
  expect(plain(frame(140)).some(row => row.includes("since session"))).toBeFalse();
  const candidate: ChangesModel = { ...MODEL, comparison: "candidate", range: "aaaaaaaa → bbbbbbbb", state: { text: "committed · worktree dirty", tone: "warn" }, scope: "repo @ task/T-1" };
  for (const [model, lead] of [[candidate, " candidate ▾  aaaaaaaa → bbbbbbbb"], [{ ...MODEL, comparison: "staged", custom: true }, " staged ▾  aaaaaaaa → worktree"]] as const) {
    for (const width of [140, 80, 52, 46]) {
      const rows = plain(frame(width, new ChangesView(host(() => model))));
      // The label and both ids stay at every width; the files follow on the next row.
      expect(rows[0]?.startsWith(lead)).toBeTrue();
      expect(rows[1]?.startsWith(" ▌ ")).toBeTrue();
    }
  }
});

test("a comparison without files is one empty-state paragraph, without columns", () => {
  const model: ChangesModel = { ...MODEL, files: [], total: 0, selected: undefined,
    diff: { kind: "message", lines: ["No changes yet.", "Files appear here as agents edit them, in the colour of who touched them."] } };
  expect(emptyChanges(model)).toBeTrue();
  expect(emptyChanges({ ...model, query: "slots" })).toBeFalse();
  for (const width of [140, 46]) {
    const rows = plain(frame(width, new ChangesView(host(() => model))));
    expect(rows[0]?.startsWith(" No changes yet. Files appear")).toBeTrue();
    expect(rows.some(row => row.includes("│") || row.includes("─"))).toBeFalse();
    for (const row of rows) expect(visibleWidth(row.trimEnd())).toBeLessThanOrEqual(Math.min(width, 71));
  }
});

test("every Changes row stays within the pane, with and without authors", () => {
  const authors: ChangeAuthor[] = [
    { id: "slots", label: "host-slots-with-a-long-name", hue: 0, paths: new Map([["src/a/one.rs", false], ["README.md", true]]) },
    { id: "ui", label: "picker-ui", hue: 1, paths: new Map([["src/a/two.rs", false], ["README.md", false]]) },
  ];
  const withAuthors: ChangesModel = { ...MODEL, custom: true, query: "a",
    files: [{ ...MODEL.files[0]!, who: ["slots"] }, { ...MODEL.files[1]!, who: ["ui"] }, { ...MODEL.files[2]!, who: ["slots", "ui"], outside: ["slots"] }],
    authors: [{ id: "slots", label: authors[0]!.label, hue: 0, count: 2 }, { id: "ui", label: "picker-ui", hue: 1, count: 2 }] };
  const plainView = () => new ChangesView(host(() => MODEL));
  const authored = () => new ChangesView(host(() => withAuthors, { authors: () => authors, color: () => "\x1b[35m" }));
  for (const make of [plainView, authored]) for (const width of [34, 46, 52, 61, 80, 89, 90, 92, 105, 150]) for (const height of [12, 20, 40]) {
    for (const row of frame(width, make(), height)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
  }
  for (const width of [10, 34, 44]) for (const row of authorChips(withAuthors, width, () => "").lines) expect(visibleWidth(row)).toBe(width);
});

test("author chips filter the files; dots, `!` and the flag row mark who edited what", async () => {
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
    expect(all.custom).toBeFalse();
    expect(all.files.map(file => file.path)).toEqual(["a.txt", "b.txt", "c.txt"]);
    expect(all.authors.map(author => [author.label, author.count])).toEqual([["host-slots", 2], ["picker-ui", 2]]);
    const chips = authorChips(all, 44, () => ""), first = stripTerminalSequences(chips.lines[0] ?? "");
    const text: Record<string, string> = { all: "all 3", slots: "host-slots 2", ui: "picker-ui 2" };
    expect(chips.hits.map(hit => first.slice(hit.from, hit.to))).toEqual(chips.hits.map(hit => text[hit.id]));
    expect(chips.hits.map(hit => hit.id)).toEqual(["all", "slots", "ui"]);
    // Too narrow for one row: the chips wrap, and none is lost.
    const wrapped = authorChips(all, 20, () => "");
    expect(wrapped.lines.length).toBeGreaterThan(1);
    expect(wrapped.hits.map(hit => hit.id)).toEqual(["all", "slots", "ui"]);

    view.cycleFilter();
    expect(view.filter).toBe("slots");
    // The filter hides the selected b.txt, so the selection moves to the first shown file.
    expect(selections).toEqual(["a.txt"]);
    const slots = model("slots");
    expect(slots.files.map(file => file.path)).toEqual(["a.txt", "c.txt"]);
    expect(slots.total).toBe(3);
    const rows = plain(frame(140, view));
    expect(rows[0]).toContain("host-slots 2");
    expect(rows.some(r => r.slice(0, LIST_WIDTH).includes("b.txt"))).toBeFalse();
    // Margin, bar, the author's dot, path.
    expect(rows.some(r => r.startsWith("   ● a.txt") && !r.slice(0, LIST_WIDTH).includes("!"))).toBeTrue();
    const c = rows.findIndex(r => r.startsWith("   ●● ! c.txt"));
    expect(c).toBeGreaterThan(0);
    expect(rows[c + 1]?.startsWith("     also edited by host-slots")).toBeTrue();

    view.cycleFilter(); view.cycleFilter();
    expect(view.filter).toBe("all");
    // Two authors inside their own paths: the file is flagged in `all`, and each author's filter still narrows to its files.
    const shared: ChangeAuthor[] = [
      { id: "slots", label: "host-slots", hue: 0, paths: new Map([["a.txt", false], ["c.txt", false]]) },
      { id: "ui", label: "picker-ui", hue: 1, paths: new Map([["b.txt", false], ["c.txt", false]]) },
    ];
    expect(model("ui", shared).files.map(file => file.path)).toEqual(["b.txt", "c.txt"]);
    const listed = plain(listLines(model("all", shared), 80).lines);
    const both = listed.findIndex(r => r.includes("c.txt"));
    expect(listed[both]).toContain("!");
    expect(listed[both + 1]).toContain("edited by host-slots and picker-ui");
    expect(listed[both + 1]).not.toContain("also");
    const alone = listed.findIndex(r => r.includes("a.txt"));
    expect(listed[alone]).not.toContain("!");
    expect(listed[alone + 1]).not.toContain("edited by");
    // Hosts that name no authors get no chips and no dots.
    expect(model("slots", []).files.map(file => [file.path, file.who, file.outside]))
      .toEqual([["a.txt", undefined, undefined], ["b.txt", undefined, undefined], ["c.txt", undefined, undefined]]);
    const bare = plain(frame(140, new ChangesView(host(() => model("all", []))))).map(r => r.slice(0, LIST_WIDTH));
    expect(bare[0]).toMatch(/^ [▌ ] a\.txt/);
    expect(bare.some(r => r.includes("●") || r.includes("all 3"))).toBeFalse();
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("the default comparison is not custom, another is", async () => {
  const root = mkdtempSync(join(tmpdir(), "changes-view-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  const store = new ReviewStore(root, () => {});
  try {
    git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "a.txt"), "one\n"); writeFileSync(join(root, "b.txt"), "one\n"); git("add", "."); git("commit", "-qm", "base");
    writeFileSync(join(root, "a.txt"), "two\n"); writeFileSync(join(root, "b.txt"), "two\n"); git("add", "b.txt");
    await store.refresh();
    const read = () => {
      const snap = store.patchSnapshot;
      return changesModel(store, { root: snap?.root, path: snap?.path, comparison: comparisonLabel(store.comparison), text: snap?.text ?? "",
        head: snap?.head, base: snap?.base, file: snap?.file }, "fixture", 0, Date.now());
    };
    const session = read();
    expect(session.custom).toBeFalse();
    expect(session.files.map(file => file.letter)).toEqual(["M", "M"]);
    expect(session.diff.kind).toBe("patch");
    await store.setComparison("staged");
    const staged = read();
    expect(staged.custom).toBeTrue();
    expect(staged.comparison).toBe("staged");
    expect(staged.files.map(file => file.path)).toEqual(["b.txt"]);
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("untracked files carry their counts whatever is selected; binary and oversized ones read `bin` and `new`", async () => {
  const root = mkdtempSync(join(tmpdir(), "changes-untracked-"));
  const git = (...args: string[]) => execFileSync("git", ["-C", root, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  const store = new ReviewStore(root, () => {});
  try {
    git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.invalid");
    writeFileSync(join(root, "a.txt"), "one\n"); git("add", "."); git("commit", "-qm", "base");
    writeFileSync(join(root, "a.txt"), "two\n");
    writeFileSync(join(root, "n1.txt"), "x\ny\n"); writeFileSync(join(root, "n2.txt"), "z");
    writeFileSync(join(root, "bin.dat"), Buffer.from([0x50, 0x00, 0x01, 0x0a]));
    writeFileSync(join(root, "big.txt"), "x\n".repeat(140_000));
    await store.refresh(); await store.select("a.txt");
    const snap = store.patchSnapshot;
    const model = changesModel(store, { root: snap?.root, path: snap?.path, comparison: comparisonLabel(store.comparison), text: snap?.text ?? "",
      head: snap?.head, base: snap?.base, file: snap?.file }, "fixture", 0, Date.now());
    expect(model.selected).toBe("a.txt");
    const rows = plain(listLines(model, 44).lines);
    const tail = (path: string) => rows.find(row => row.includes(path))?.trimEnd().split(/\s{2,}/).at(-1);
    expect(tail("a.txt")).toBe("+1 −1");
    expect(tail("n1.txt")).toBe("+2 −0");
    expect(tail("n2.txt")).toBe("+1 −0");
    expect(tail("bin.dat")).toBe("bin");
    expect(tail("big.txt")).toBe("new");
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});
