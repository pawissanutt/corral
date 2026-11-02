import { test, expect } from "bun:test";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getCapabilities, setCapabilities, visibleWidth, type ImageSource } from "@earendil-works/pi-tui";
import { ArtifactView, type ArtifactCatalog } from "./artifact-view.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
const HEAD = "b".repeat(40), OLD = "a".repeat(40);
const png = readFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url));
const sha = (bytes: Buffer | string) => createHash("sha256").update(bytes).digest("hex");

function fixture() {
  const store = mkdtempSync(join(tmpdir(), "artifact-view-"));
  mkdirSync(join(store, "artifacts/T-001/R1"), { recursive: true });
  copyFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url), join(store, "artifacts/T-001/R1/1111-shot.png"));
  const row = (name: string, extra: Partial<ArtifactCatalog["artifacts"][number]> = {}) => ({
    reference: `T-001:R1:${name}`, run: "R1", kind: "report", title: name, path: `artifacts/T-001/R1/${name}`, sha256: sha(name),
    bytes: 10, added: "2026-01-01T00:00:00Z", compressed: false, purged: false, available: true, raw: join(store, "artifacts/T-001/R1", name), evidence: [], evidence_omitted: 0, ...extra,
  });
  const catalog: ArtifactCatalog = { version: 1, task: "T-001", run: "R1", store, head: HEAD, more: true, artifacts: [
    row("1111-shot.png", { kind: "screenshot", sha256: sha(png), bytes: png.length,
      evidence: [{ criterion: "C1", key: "c1-obs", status: "pass", captured: "2026-01-02T00:00:00Z", head: OLD, hash_match: true }] }),
    row("2222-report.md", { evidence: [{ criterion: "C2", key: "c2-obs", status: "pass", captured: "2026-01-02T00:00:00Z", head: HEAD, hash_match: false }] }),
    row("3333-gone.log", { kind: "log", available: false, raw: null }),
    row("4444-page.html"),
    { ...row("5555-other.md"), reference: "T-001:R2:5555-other.md", run: "R2", path: "artifacts/T-001/R2/5555-other.md" },
  ] };
  return { store, catalog };
}
function harness(catalog: unknown, extra: { exportSession?: () => Promise<string> } = {}, decoded = "") {
  const calls: string[] = [], images: ImageSource[] = [];
  let reads = 0;
  const view = new ArtifactView({ identity, load: async () => { reads++; return catalog; },
    viewer: async (reference, viewer) => { calls.push(`${viewer} ${reference}`); return viewer === "serve" ? "http://127.0.0.1:1/x" : reference.endsWith(".png") ? `png image\n${decoded}` : "# Report\nbody line\u001b[31m"; },
    openImage: source => { images.push(source); return true; }, bound: () => true, changed: () => {}, canInteract: () => true, ...extra });
  return { view, calls, images, reads: () => reads };
}
const text = (view: ArtifactView, width = 140) => view.render(width).map(row => row.trimEnd()).join(" ").replace(/\s+/g, " ");
const settle = () => new Promise(resolve => setImmediate(resolve));

test("catalog renders provenance without reading files or starting viewers", async () => {
  const { store, catalog } = fixture();
  try {
    const { view, calls, images, reads } = harness(catalog);
    await view.refresh();
    for (let i = 0; i < 50; i++) { view.render(140); view.render(52); }
    expect(reads()).toBe(1); expect(calls).toEqual([]); expect(images).toEqual([]);
    const wide = text(view);
    expect(wide).toContain("GAP: newest 5 artifact rows only");
    expect(wide).toContain(`registered against ${OLD} (differs from bound HEAD at read · historical; freshness not evaluated)`);
    expect(wide).toContain("Candidate: artifact rows record none");
    expect(wide).toContain("bytes not re-hashed");
    view.handleInput("j");
    expect(text(view)).toContain("same SHA as bound HEAD at read; freshness not evaluated) · recorded hash MISMATCH");
    view.handleInput("j");
    expect(text(view)).toContain("ABSENT: no snapshot file or live blob at read");
    expect(text(view)).toContain("Evidence: none of R1 names this snapshot · association unknown");
    view.handleInput("j"); view.handleInput("j");
    expect(text(view)).toContain("Evidence: unknown · run R2 evidence not read");
    expect(view.render(52).every(row => visibleWidth(row) <= 52)).toBe(true);
    expect(calls).toEqual([]);
  } finally { rmSync(store, { recursive: true, force: true }); }
});

test("Enter opens images with verified bytes, text fallback without graphics, and reports through corral show", async () => {
  const { store, catalog } = fixture();
  const caps = getCapabilities();
  try {
    const { view, calls, images } = harness(catalog);
    await view.refresh();
    setCapabilities({ ...caps, images: null });
    view.handleInput("\r"); await settle();
    expect(images).toEqual([]);
    expect(text(view)).toContain("Graphics unavailable in this terminal · text/path fallback");
    expect(text(view)).toContain(join(store, "artifacts/T-001/R1/1111-shot.png"));
    expect(text(view)).toContain("Bytes at open match the registered SHA256");
    expect(text(view)).toContain("bytes matched at open");
    setCapabilities({ ...caps, images: "kitty" });
    view.handleInput("\r"); await settle();
    expect(images.length).toBe(1);
    expect(images[0]?.mimeType).toBe("image/png");
    view.handleInput("j"); view.handleInput("\r"); await settle(); await settle();
    expect(calls).toEqual(["show T-001:R1:2222-report.md"]);
    expect(text(view)).toContain("body line");
    expect(text(view)).not.toContain("\u001b[31m");
    view.handleInput("j"); view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Unavailable · not opened");
    view.handleInput("j"); view.handleInput("\r"); await settle();
    expect(text(view)).toContain("Browser document · not rendered in the terminal");
    expect(calls).toEqual(["show T-001:R1:2222-report.md"]);
    view.handleInput("o"); await settle(); await settle();
    expect(calls).toEqual(["show T-001:R1:2222-report.md", "serve T-001:R1:4444-page.html"]);
    expect(text(view)).toContain("Browser requested");
  } finally { setCapabilities(caps); rmSync(store, { recursive: true, force: true }); }
});

test("image reads stay confined to the artifact store and report hash mismatches", async () => {
  const { store, catalog } = fixture();
  const caps = getCapabilities();
  try {
    setCapabilities({ ...caps, images: null });
    const escaped = { ...catalog, artifacts: [{ ...catalog.artifacts[0]!, raw: join(store, "../outside.png") }] };
    const outside = harness(escaped); await outside.view.refresh();
    outside.view.handleInput("\r"); await settle();
    expect(text(outside.view)).toContain("Viewer unavailable: snapshot path outside the artifact store");
    symlinkSync(join(store, "artifacts/T-001/R1/1111-shot.png"), join(store, "artifacts/T-001/R1/link.png"));
    const linked = harness({ ...catalog, artifacts: [{ ...catalog.artifacts[0]!, raw: join(store, "artifacts/T-001/R1/link.png") }] }); await linked.view.refresh();
    linked.view.handleInput("\r"); await settle();
    expect(text(linked.view)).toContain("snapshot is not a regular file");
    const decoded = join(store, "decoded-copy.png"); copyFileSync(join(store, "artifacts/T-001/R1/1111-shot.png"), decoded);
    const blob = harness({ ...catalog, artifacts: [{ ...catalog.artifacts[0]!, raw: null }] }, {}, decoded); await blob.view.refresh();
    blob.view.handleInput("\r"); await settle(); await settle();
    expect(blob.calls).toEqual(["show T-001:R1:1111-shot.png"]);
    expect(text(blob.view)).toContain(`Path: ${decoded}`);
    expect(text(blob.view)).toContain("Bytes at open match the registered SHA256");
    const changed = harness({ ...catalog, artifacts: [{ ...catalog.artifacts[0]!, sha256: "0".repeat(64) }] }); await changed.view.refresh();
    changed.view.handleInput("\r"); await settle();
    expect(text(changed.view)).toContain("MISMATCH: bytes at open hash");
    expect(text(changed.view)).toContain("Not displayed: bytes differ from the registered artifact");
    expect(text(changed.view)).toContain("BYTES MISMATCHED at open");
  } finally { setCapabilities(caps); rmSync(store, { recursive: true, force: true }); }
});

test("comparison uses only explicit selection order and never infers before/after", async () => {
  const { store, catalog } = fixture();
  try {
    const { view } = harness(catalog);
    await view.refresh();
    view.handleInput("c");
    expect(view.compare).toBe(false);
    expect(text(view)).toContain("Comparison needs two artifacts selected explicitly with m");
    view.handleInput("j"); view.handleInput("m"); view.handleInput("k"); view.handleInput("m");
    expect(view.marks).toEqual(["T-001:R1:2222-report.md", "T-001:R1:1111-shot.png"]);
    view.handleInput("j"); view.handleInput("j"); view.handleInput("m");
    expect(text(view)).toContain("Two artifacts already selected");
    view.handleInput("c");
    const wide = text(view);
    expect(wide).toContain("Comparison · A and B in your selection order · no before/after, candidate order or freshness implied");
    expect(wide).toContain("Registered bytes differ (different SHA256)");
    expect(wide).toContain("Title: A 2222-report.md │ B 1111-shot.png");
    expect(text(view, 60)).toContain("Title B: 1111-shot.png");
    expect(wide).not.toMatch(/\b(before|after):/i);
    view.handleInput("k"); view.handleInput("k"); view.handleInput("m");
    expect(view.compare).toBe(false);
    expect(view.marks).toEqual(["T-001:R1:2222-report.md"]);
  } finally { rmSync(store, { recursive: true, force: true }); }
});

test("export reaches the existing flow; refusals and stale reads stay visible", async () => {
  const { store, catalog } = fixture();
  try {
    const refused = harness(catalog, { exportSession: async () => { throw new Error("session_export is not enabled for this role"); } });
    await refused.view.refresh();
    refused.view.handleInput("x"); await settle(); await settle();
    expect(text(refused.view)).toContain("Session export refused: session_export is not enabled for this role");
    const allowed = harness(catalog, { exportSession: async () => "corral: session export committed: /tmp/session.html" });
    await allowed.view.refresh();
    allowed.view.handleInput("x"); await settle(); await settle();
    expect(text(allowed.view)).toContain("corral: session export committed");
    const absent = harness(catalog); await absent.view.refresh();
    absent.view.handleInput("x"); await settle();
    expect(text(absent.view)).toContain("Session export unavailable");
    let fail = false;
    const flaky = new ArtifactView({ identity, load: async () => { if (fail) throw new Error("disconnected"); return catalog; }, openImage: () => true, bound: () => true, changed: () => {}, canInteract: () => true });
    await flaky.refresh(); fail = true; await flaky.refresh();
    expect(text(flaky)).toContain("STALE / disconnected · disconnected");
    expect(text(flaky)).toContain("1111-shot.png");
    const foreign = new ArtifactView({ identity, load: async () => ({ ...catalog, run: "R9" }), openImage: () => true, bound: () => true, changed: () => {}, canInteract: () => false });
    await foreign.refresh();
    expect(foreign.current).toBeUndefined();
    expect(text(foreign)).toContain("Unavailable · no empty catalog inferred");
    expect(foreign.handleInput("\r")).toBe(false);
  } finally { rmSync(store, { recursive: true, force: true }); }
});
