import { test, expect } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { ArtifactView, COMPARE_ROW_MIN, type ArtifactCatalog } from "./artifact-view.ts";
import { VIEW_INSET } from "./frame.ts";
import { clock } from "./panel-parts.ts";
import { SPLIT_MIN } from "./proof-view.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-1" };
const png = readFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url));
const sha = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");
const REPORT = "# Report\nbody line\u001b[31m";
/** What `corral artifact show` prints, decoded as UTF-8 the way the host decodes it. */
const SHOWN: Readonly<Record<string, string>> = { "T-1:1": "owner notes", "T-1:2": png.toString("utf8"), "T-1:3": REPORT, "T-1:4": "edited after registration" };
const row = (id: number, title: string, extra: Partial<ArtifactCatalog[number]> = {}): ArtifactCatalog[number] => ({
  task: "T-1", id, sha256: sha(SHOWN[`T-1:${id}`] ?? title), bytes: 10, kind: "report", title, at: `2026-01-0${id}T00:00:00Z`, by: "agent", ...extra,
});
/** The engine's `artifact list` order: oldest first. */
const catalog: ArtifactCatalog = [
  row(1, "notes.md", { kind: null, by: "owner" }),
  row(2, "shot.png", { kind: "screenshot", sha256: sha(png), bytes: png.length }),
  row(3, "report.md"),
  row(4, "changed.md", { sha256: "0".repeat(64) }),
];
function harness(data: unknown, extra: { exportSession?: () => Promise<string> } = {}) {
  const calls: string[] = [];
  let reads = 0;
  const view = new ArtifactView({ identity, load: async () => { reads++; return data; },
    viewer: async reference => { calls.push(reference); return SHOWN[reference] ?? ""; },
    bound: () => true, changed: () => {}, canInteract: () => true, ...extra });
  return { view, calls, reads: () => reads };
}
/** The view's text at a stacked width (list above detail), so detail rows read in order. */
const text = (view: ArtifactView, width = 100) => view.render(width).map(row => stripTerminalSequences(row).trimEnd()).join(" ").replace(/\s+/g, " ");
/** One macrotask turn: the viewer and export promises Enter and x start have settled. */
const settle = () => { const { promise, resolve } = Promise.withResolvers<void>(); setImmediate(resolve); return promise; };

test("catalog renders newest first with its provenance and reads no content", async () => {
  const { view, calls, reads } = harness(catalog);
  await view.refresh();
  for (let i = 0; i < 50; i++) { view.render(140); view.render(52); }
  expect(reads()).toBe(1); expect(calls).toEqual([]);
  const shown = text(view);
  expect(shown).toContain("4 artifacts, newest first");
  expect(shown.indexOf("changed.md") < shown.indexOf("notes.md")).toBe(true);
  expect(shown).toContain("Reference T-1:4");
  expect(shown).toContain("By agent");
  expect(shown).toContain("Added 2026-01-04T00:00:00Z");
  expect(shown).toContain("bytes not re-hashed");
  view.handleInput("j"); view.handleInput("j"); view.handleInput("j");
  expect(text(view)).toContain("Kind — By owner");
  expect(view.render(52).every(row => visibleWidth(row) <= 52)).toBe(true);
  expect(calls).toEqual([]);
});

test("Enter shows text whose bytes match; binary and changed bytes are not shown", async () => {
  const { view, calls } = harness(catalog);
  await view.refresh();
  view.handleInput("\r"); await settle();
  expect(text(view)).toContain("Not displayed: bytes differ from the registered artifact");
  expect(text(view)).toContain(`MISMATCH: bytes at open hash ${sha(SHOWN["T-1:4"] ?? "")}`);
  expect(text(view)).toContain("BYTES MISMATCHED at open");
  expect(text(view)).not.toContain("edited after registration");
  view.handleInput("j"); view.handleInput("\r"); await settle();
  expect(text(view)).toContain("body line");
  expect(text(view)).toContain("bytes matched at open");
  expect(view.render(100).join("")).not.toContain("\u001b[31m");
  view.handleInput("j"); view.handleInput("\r"); await settle();
  expect(text(view)).toContain("Binary content · not shown in the terminal");
  expect(text(view)).toContain(`corral artifact show T-1:2 writes its ${png.length.toLocaleString("en-US")} bytes to stdout`);
  expect(text(view)).toContain("binary content · not re-hashed as text");
  expect(calls).toEqual(["T-1:4", "T-1:3", "T-1:2"]);
});

test("comparison uses only explicit selection order and never infers before/after", async () => {
  const { view } = harness(catalog);
  await view.refresh();
  view.handleInput("c");
  expect(view.compare).toBe(false);
  expect(text(view)).toContain("Comparison needs two artifacts selected explicitly with m");
  view.handleInput("j"); view.handleInput("j"); view.handleInput("m"); view.handleInput("k"); view.handleInput("m");
  expect(view.marks).toEqual(["T-1:2", "T-1:3"]);
  view.handleInput("k"); view.handleInput("m");
  expect(text(view)).toContain("Two artifacts already selected");
  view.handleInput("c");
  const wide = text(view);
  expect(wide).toContain("A and B in your selection order · no before/after, candidate order or freshness implied");
  expect(wide).toContain("Registered bytes differ (different SHA256)");
  expect(wide).toContain("Title shot.png │ report.md");
  expect(text(view, 60)).toContain("Title B report.md");
  // One row per field from COMPARE_ROW_MIN main-view columns; the workbench renders the view VIEW_INSET narrower.
  expect(text(view, COMPARE_ROW_MIN - VIEW_INSET)).toContain("Title shot.png │ report.md");
  expect(text(view, COMPARE_ROW_MIN - VIEW_INSET - 1)).toContain("Title B report.md");
  expect(wide).not.toMatch(/\b(before|after):/i);
  view.handleInput("j"); view.handleInput("m");
  expect(view.compare).toBe(false);
  expect(view.marks).toEqual(["T-1:2"]);
});

test("export reaches the existing flow and shows its refusal", async () => {
  const refused = harness(catalog, { exportSession: async () => { throw new Error("session_export is not enabled for this role"); } });
  refused.view.handleInput("x"); await settle();
  expect(text(refused.view)).toContain("Session export refused: session_export is not enabled for this role");
  const allowed = harness(catalog, { exportSession: async () => "corral: session export committed: /tmp/session.html" });
  allowed.view.handleInput("x"); await settle();
  expect(text(allowed.view)).toContain("corral: session export committed");
  const absent = harness(catalog);
  absent.view.handleInput("x");
  expect(text(absent.view)).toContain("Session export unavailable");
});

test("a failed reread keeps the last catalog; another task's rows are refused", async () => {
  let fail = false, bound = true;
  const flaky = new ArtifactView({ identity, load: async () => { if (fail) throw new Error("db locked"); return catalog; }, bound: () => bound, changed: () => {}, canInteract: () => true });
  await flaky.refresh();
  const readAt = flaky.state.readAt;
  fail = true; bound = false; await flaky.refresh();
  expect(flaky.state).toMatchObject({ phase: "retained", readAt, error: "db locked" });
  for (const width of [44, 68, 101]) {
    const shown = text(flaky, width);
    expect(shown).toContain(clock(readAt ?? NaN));
    expect(shown).toContain("db locked");
    expect(shown).toContain("host unbound");
  }
  expect(text(flaky)).toContain("changed.md");
  const foreign = harness([...catalog, row(5, "elsewhere.md", { task: "T-9" })]);
  await foreign.view.refresh();
  expect(foreign.view.current).toBeUndefined();
  expect(foreign.view.state.phase).toBe("failed");
  expect(text(foreign.view)).toContain("the read returned T-9, not this session's task");
  expect(text(foreign.view)).not.toContain("notes.md");
});

test("from SPLIT_MIN the list sits beside the detail; by words line up under their header; back link and legend", async () => {
  let legend = false;
  const view = new ArtifactView({ identity, load: async () => catalog, bound: () => true, changed: () => {}, canInteract: () => true, legend: () => legend });
  await view.refresh();
  for (const width of [138, SPLIT_MIN - VIEW_INSET, SPLIT_MIN - VIEW_INSET - 1, 59, 44]) {
    const rows = view.render(width).map(row => stripTerminalSequences(row));
    for (const row of view.render(width)) expect(visibleWidth(row)).toBeLessThanOrEqual(width);
    expect(rows.some(row => row.includes("┬"))).toBe(width + VIEW_INSET >= SPLIT_MIN);
    expect(rows[0]?.startsWith("‹ Proof · Artifacts")).toBe(true);
    // The owner's row says owner under the by header; the selected (newest) row carries the bar.
    const header = rows.find(row => /^ +(kind +)?title/.test(row)) ?? "";
    // Titles clip to their column, so the owner's row is found by the start of its title.
    const owned = rows.find(row => row.includes("notes")) ?? "";
    expect(owned.slice(header.indexOf(" by") + 1).startsWith("owner")).toBe(true);
    expect(rows.some(row => row.startsWith("▌") && row.includes("changed"))).toBe(true);
  }
  expect(view.backAt(0, 0)).toBe(true); expect(view.backAt("‹ Proof".length - 1, 0)).toBe(true);
  expect(view.backAt("‹ Proof".length, 0)).toBe(false); expect(view.backAt(0, 1)).toBe(false);
  // The full digest appears once, on its own detail row; the caveat is one line until `?` shows the legend.
  expect(text(view, 138).split("0".repeat(64)).length).toBe(2);
  expect(text(view)).toContain("not live"); expect(text(view)).not.toContain("never on focus or selection");
  legend = true;
  expect(text(view)).toContain("Content is read only on Enter, never on focus or selection");
  expect(text(view)).toContain("binary content is not shown in the terminal");
});
