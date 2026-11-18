import { test, expect } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getCapabilities, setCapabilities, stripTerminalSequences, visibleWidth, type ImageSource } from "@earendil-works/pi-tui";
import { ArtifactView, COMPARE_ROW_MIN, type ArtifactCatalog } from "./artifact-view.ts";
import { VIEW_INSET } from "./frame.ts";
import { clock } from "./panel-parts.ts";
import { SPLIT_MIN } from "./proof-view.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-1" };
const png = readFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url));
const sha = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");
const REPORT = "# Report\nbody line\u001b[31m";
/** The bytes `corral artifact show` writes for each reference. */
const SHOWN: Readonly<Record<string, Buffer | string>> = {
  "T-1:1": "owner notes", "T-1:2": png, "T-1:3": REPORT, "T-1:4": "edited after registration", "T-1:5": Buffer.from([0x00, 0xff, 0xfe, 0x01]),
};
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
  const calls: string[] = [], images: ImageSource[] = [], dir = mkdtempSync(join(tmpdir(), "artifact-view-"));
  let reads = 0;
  const view = new ArtifactView({ identity, load: async () => { reads++; return data; },
    file: async reference => { calls.push(reference); const path = join(dir, reference); writeFileSync(path, SHOWN[reference] ?? ""); return path; },
    openImage: source => { images.push(source); return true; },
    bound: () => true, changed: () => {}, canInteract: () => true, ...extra });
  return { view, calls, images, reads: () => reads };
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

test("Enter shows text whose bytes match; changed bytes and other binary content are not shown", async () => {
  const { view, calls, images } = harness([...catalog, row(5, "core.bin", { kind: "dump" })]);
  const capabilities = getCapabilities();
  try {
    setCapabilities({ ...capabilities, images: null });
    await view.refresh();
    view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Binary content · not shown in the terminal");
    view.handleInput("j"); view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Not displayed: bytes differ from the registered artifact");
    expect(text(view)).toContain(`MISMATCH: bytes at open hash ${sha(SHOWN["T-1:4"] ?? "")}`);
    expect(text(view)).toContain("BYTES MISMATCHED at open");
    expect(text(view)).not.toContain("edited after registration");
    view.handleInput("j"); view.handleInput("\r"); await settle();
    expect(text(view)).toContain("body line");
    expect(text(view)).toContain("bytes matched at open");
    expect(view.render(100).join("")).not.toContain("\u001b[31m");
    // Without graphics an image is described, never drawn as text.
    view.handleInput("j"); view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Image · this terminal cannot draw graphics");
    expect(images).toHaveLength(0);
    // With graphics it opens in the reading area, its exact bytes and title.
    setCapabilities({ ...capabilities, images: "kitty" });
    view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Image · shown in the reading area");
    expect(images.map(image => [image.mimeType, image.filename, image.data])).toEqual([["image/png", "shot.png", png.toString("base64")]]);
    expect(calls).toEqual(["T-1:5", "T-1:4", "T-1:3", "T-1:2", "T-1:2"]);
  } finally { setCapabilities(capabilities); }
});

test("a click selects a row and a click on the selected row opens it", async () => {
  const { view, calls } = harness(catalog);
  await view.refresh();
  const rows = view.render(100).map(line => stripTerminalSequences(line));
  const at = rows.findIndex(line => line.includes("report.md"));
  const click = { type: "click", button: "left", x: 4, y: at, shift: false, alt: false, ctrl: false } as Parameters<ArtifactView["handleMouse"]>[0];
  view.handleMouse(click); await settle();
  expect(calls).toEqual([]);
  view.handleMouse(click); await settle();
  expect(calls).toEqual(["T-1:3"]);
  expect(text(view)).toContain("body line");
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
    expect(shown).toContain("host closed");
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
  expect(text(view)).toContain("other binary content is not shown");
});
