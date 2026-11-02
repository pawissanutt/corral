import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { extname, isAbsolute, resolve, sep } from "node:path";
import { z } from "zod";
import { Text, getCapabilities, getImageDimensions, matchesKey, type Component, type ImageSource, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { plainLabel } from "./panels-view.ts";

const Label = z.string().max(4096);
const Schema = z.object({
  version: z.literal(1), task: Label, run: Label, store: Label, head: Label.nullable(), more: z.boolean(),
  artifacts: z.array(z.object({
    reference: Label, run: Label, kind: Label, title: Label, path: Label, sha256: Label,
    bytes: z.number().int().nonnegative(), added: Label, compressed: z.boolean(), purged: z.boolean(), available: z.boolean(), raw: Label.nullable(),
    evidence: z.array(z.object({
      criterion: Label, key: Label, status: z.enum(["pass", "fail", "not_run"]), captured: Label, head: Label, hash_match: z.boolean().nullable(),
    })).max(8),
    evidence_omitted: z.number().int().nonnegative(),
  })).max(100),
});
export type ArtifactCatalog = z.infer<typeof Schema>;
type Entry = ArtifactCatalog["artifacts"][number];

const IMAGE_MIME: Readonly<Record<string, string>> = { ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp" };
const BROWSER_ONLY = new Set([".html", ".htm", ".svg"]);
const MAX_IMAGE_BYTES = 16 * 1024 * 1024;
const MAX_TEXT_LINES = 400;

export interface ArtifactViewOptions {
  readonly identity: HostIdentity;
  readonly load?: () => Promise<unknown>;
  readonly viewer?: (reference: string, viewer: "show" | "serve") => Promise<string>;
  /** The existing `/corral-export` flow; its refusal message is shown verbatim. */
  readonly exportSession?: () => Promise<string>;
  readonly openImage: (source: ImageSource) => boolean;
  readonly bound: () => boolean;
  readonly changed: () => void;
  readonly canInteract: () => boolean;
  readonly readFile?: (path: string) => Buffer;
}
interface Opened { readonly reference: string; readonly title: string; readonly lines: readonly string[] }

/**
 * Cached bounded catalog. Rendering and focus never read files or start viewers: Enter, o and x
 * are the only actions that do, and each acts on the selected identity at that moment.
 */
export class ArtifactView implements Component {
  current: ArtifactCatalog | undefined;
  error = "Catalog not read"; stale = true; refreshing = false;
  marks: string[] = [];
  compare = false;
  notice = "";
  private selected: string | undefined;
  private opened: Opened | undefined;
  private verified = new Map<string, boolean>();
  private active: Promise<void> | undefined;
  private disposed = false;
  private hits = new Map<number, string>();
  constructor(private readonly options: ArtifactViewOptions) {}
  refresh(): Promise<void> {
    if (this.active || this.disposed) return this.active ?? Promise.resolve();
    this.active = this.read().finally(() => { this.active = undefined; });
    return this.active;
  }
  private async read(): Promise<void> {
    const { identity } = this.options;
    this.refreshing = true; this.options.changed();
    try {
      if (!this.options.load || !identity.task || !identity.run) throw new Error("No canonical task/run bound to this host");
      const data = Schema.parse(await this.options.load());
      if (data.task !== identity.task || data.run !== identity.run) throw new Error("Artifact catalog identity mismatch");
      if (this.disposed) return;
      this.current = data; this.stale = false; this.error = "";
      this.selected ??= data.artifacts[0]?.reference;
    } catch (error) { if (!this.disposed) { this.stale = true; this.error = error instanceof Error ? error.message : String(error); } }
    finally { this.refreshing = false; if (!this.disposed) this.options.changed(); }
  }
  private entry(reference: string | undefined): Entry | undefined {
    return reference === undefined ? undefined : this.current?.artifacts.find(a => a.reference === reference);
  }
  handleInput(data: string): boolean {
    if (!this.options.canInteract()) return false;
    const list = this.current?.artifacts ?? [];
    if (data === "j" || data === "k" || matchesKey(data, "down") || matchesKey(data, "up")) {
      const index = list.findIndex(a => a.reference === this.selected);
      const next = list[Math.max(0, Math.min(list.length - 1, index + (data === "j" || matchesKey(data, "down") ? 1 : -1)))];
      if (next) this.selected = next.reference;
      this.opened = undefined; this.options.changed(); return true;
    }
    if (data === "r") { void this.refresh(); return true; }
    if (data === "m") { this.mark(); return true; }
    if (data === "c") {
      this.compare = !this.compare && this.marks.length === 2;
      this.notice = this.compare || this.marks.length === 2 ? "" : "Comparison needs two artifacts selected explicitly with m";
      this.options.changed(); return true;
    }
    if (matchesKey(data, "enter")) { void this.open(); return true; }
    if (data === "o") { void this.browse(); return true; }
    if (data === "x") { void this.exportSession(); return true; }
    return false;
  }
  handleMouse(event: TuiMouseEvent) {
    if (!this.options.canInteract() || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const reference = this.hits.get(event.y);
    if (reference === undefined) return undefined;
    if (reference !== this.selected) this.opened = undefined;
    this.selected = reference; this.options.changed(); return { handled: true };
  }
  private mark(): void {
    const reference = this.selected;
    if (!reference || !this.entry(reference)) { this.notice = "No artifact selected"; this.options.changed(); return; }
    if (this.marks.includes(reference)) this.marks = this.marks.filter(m => m !== reference);
    else if (this.marks.length < 2) this.marks = [...this.marks, reference];
    else this.notice = "Two artifacts already selected · m on a selected one clears it";
    if (this.marks.length < 2) this.compare = false;
    this.options.changed();
  }
  private say(text: string): void { if (!this.disposed) { this.notice = plainLabel(text).slice(0, 2000); this.options.changed(); } }
  private async open(): Promise<void> {
    const entry = this.entry(this.selected);
    if (!entry) { this.say("No artifact selected"); return; }
    const ext = extname(entry.path).toLowerCase();
    const stored = entry.raw ?? resolve(this.current?.store ?? "", entry.path);
    const show = (lines: string[]) => { if (!this.disposed && this.selected === entry.reference) { this.opened = { reference: entry.reference, title: entry.title, lines }; this.notice = ""; this.options.changed(); } };
    if (entry.purged) { show(["Content purged by retention · metadata only; no bytes to view"]); return; }
    if (!entry.available) { show([`Snapshot absent at read: no snapshot file or live blob for ${stored}`, "Unavailable · not opened; reconcile with r."]); return; }
    if (BROWSER_ONLY.has(ext)) { show([`Snapshot: ${stored}`, "Browser document · not rendered in the terminal. o opens it through corral's loopback artifact server (explicit action only)."]); return; }
    if (!this.options.viewer && !entry.raw) { show([`Snapshot: ${stored}`, "Viewer unavailable in this host"]); return; }
    const mime = IMAGE_MIME[ext];
    try {
      if (mime) {
        // Raw legacy bytes are read in place; blob-backed bytes come from corral's decoded copy.
        const path = entry.raw ?? (await this.options.viewer?.(entry.reference, "show") ?? "").trim().split("\n").at(-1) ?? "";
        const bytes = this.readImage(path, entry.raw !== null);
        const hash = createHash("sha256").update(bytes).digest("hex");
        this.verified.set(entry.reference, hash === entry.sha256);
        const data = bytes.toString("base64");
        const dimensions = getImageDimensions(data, mime);
        const facts = [`Path: ${path}`, `${dimensions ? `${dimensions.widthPx}×${dimensions.heightPx}px` : "dimensions unavailable"} · ${bytes.length} bytes`,
          hash === entry.sha256 ? "Bytes at open match the registered SHA256" : `MISMATCH: bytes at open hash ${hash}; registered ${entry.sha256}`];
        if (hash !== entry.sha256) { show(["Not displayed: bytes differ from the registered artifact", ...facts]); return; }
        if (!getCapabilities().images || !dimensions) { show(["Graphics unavailable in this terminal · text/path fallback", ...facts]); return; }
        if (this.disposed || this.selected !== entry.reference) return;
        show(facts);
        this.options.openImage({ data, mimeType: mime, dimensions, filename: entry.title });
        return;
      }
      const text = await this.options.viewer?.(entry.reference, "show") ?? "";
      const lines = text.split("\n").map(line => line.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, " "));
      show([...lines.slice(0, MAX_TEXT_LINES), ...(lines.length > MAX_TEXT_LINES ? [`… ${lines.length - MAX_TEXT_LINES} more lines · corral artifact show ${entry.reference}`] : [])]);
    } catch (error) { show([`Viewer unavailable: ${error instanceof Error ? error.message : String(error)}`, `Snapshot: ${stored}`]); }
  }
  /**
   * A raw snapshot must lie beneath the catalog's store; corral's decoded copy must be an absolute
   * path. Either way a symlink, non-regular or oversized file is refused, and display needs a hash match.
   */
  private readImage(path: string, raw: boolean): Buffer {
    if (!isAbsolute(path) || resolve(path) !== path) throw new Error("viewer returned no absolute image path");
    if (raw && !path.startsWith(resolve(this.current?.store ?? "") + sep)) throw new Error("snapshot path outside the artifact store");
    const stat = lstatSync(path);
    if (!stat.isFile()) throw new Error("snapshot is not a regular file");
    if (stat.size > MAX_IMAGE_BYTES) throw new Error(`snapshot exceeds ${MAX_IMAGE_BYTES} bytes`);
    return (this.options.readFile ?? readFileSync)(path);
  }
  private async browse(): Promise<void> {
    const entry = this.entry(this.selected);
    if (!entry) { this.say("No artifact selected"); return; }
    if (!this.options.viewer) { this.say("Browser viewer unavailable in this host"); return; }
    this.say(`Opening ${entry.reference} through corral artifact serve…`);
    try { this.say(`Browser requested · ${(await this.options.viewer(entry.reference, "serve")).trim()}`); }
    catch (error) { this.say(`Browser open refused/failed: ${error instanceof Error ? error.message : String(error)}`); }
  }
  private async exportSession(): Promise<void> {
    if (!this.options.exportSession) { this.say("Session export unavailable: session_export is not enabled for this role"); return; }
    this.say("Exporting through /corral-export…");
    try { this.say(await this.options.exportSession()); }
    catch (error) { this.say(`Session export refused: ${error instanceof Error ? error.message : String(error)}`); }
  }
  private provenance(entry: Entry, data: ArtifactCatalog): string[] {
    const verified = this.verified.get(entry.reference);
    const rows = [
      `${entry.reference} · ${entry.kind} · run ${entry.run || "none (session artifact)"} · added ${entry.added}`,
      `${entry.bytes} bytes · ${entry.purged ? "PURGED (metadata only)" : entry.available ? `content present at read${entry.raw ? " (raw snapshot file)" : " (content store)"}` : "ABSENT: no snapshot file or live blob at read"}${entry.compressed ? " · stored compressed" : ""}`,
      `Registered SHA256 ${entry.sha256} · ${verified === undefined ? "bytes not re-hashed" : verified ? "bytes matched at open" : "BYTES MISMATCHED at open"}`,
      "Candidate: artifact rows record none; only evidence links below carry a registration HEAD",
    ];
    if (entry.run !== data.run) rows.push(`Evidence: unknown · run ${entry.run || "none"} evidence not read (bound run ${data.run} only)`);
    else if (!entry.evidence.length) rows.push(`Evidence: none of ${data.run} names this snapshot · association unknown, not rejected`);
    for (const link of entry.evidence) {
      const relation = !data.head ? "bound HEAD unavailable" : link.head === data.head ? "same SHA as bound HEAD at read" : "differs from bound HEAD at read · historical";
      rows.push(`Evidence ${link.criterion} · ${link.key} · worker_assertion ${link.status} · captured ${link.captured}`,
        `  registered against ${link.head} (${relation}; freshness not evaluated) · recorded hash ${link.hash_match === null ? "absent" : link.hash_match ? "matches" : "MISMATCH"}`);
    }
    if (entry.evidence_omitted) rows.push(`${entry.evidence_omitted} more evidence links omitted`);
    return rows;
  }
  render(width: number): string[] {
    const rows: string[] = []; this.hits.clear();
    const add = (text: string, reference?: string) => {
      const wrapped = new Text(plainLabel(text), 0, 0).render(Math.max(1, width));
      if (reference !== undefined) for (let i = 0; i < wrapped.length; i++) this.hits.set(rows.length + i, reference);
      rows.push(...wrapped);
    };
    const { identity } = this.options;
    add("Artifacts · read-only catalog · j/k or click select · Enter open · o browser · m select for comparison · c compare · x export · r reconcile");
    add(`${identity.task ?? "unbound"} / ${identity.run ?? "unbound"} · viewers start only on Enter/o, never on focus`);
    add(this.stale || !this.options.bound() ? `STALE / disconnected · ${this.error || "retained catalog; reconcile"}` : "Canonical catalog at last reconcile · not a live subscription");
    if (this.refreshing) add("Reading… retained rows are historical");
    if (this.notice) add(this.notice);
    const data = this.current;
    if (!data) { add("Unavailable · no empty catalog inferred"); return rows; }
    add(`Bound worktree HEAD ${data.head ?? "unavailable"} at read · a SHA, not a freshness verdict`);
    if (data.more) add(`GAP: newest ${data.artifacts.length} artifact rows only; older rows not read`);
    if (!data.artifacts.length) { add(data.more ? "No rows in this window" : "No artifacts registered for this task"); return rows; }
    const index = Math.max(0, data.artifacts.findIndex(a => a.reference === this.selected));
    const entry = data.artifacts[index];
    if (this.compare) {
      const [a, b] = this.marks.map(m => this.entry(m));
      add("Comparison · A and B in your selection order · no before/after, candidate order or freshness implied");
      if (!a || !b) add("A selected artifact is no longer in this window · comparison unavailable");
      else {
        add(a.sha256 === b.sha256 ? "Registered bytes identical (same SHA256)" : "Registered bytes differ (different SHA256)");
        for (const [label, pick] of [["Title", (e: Entry) => e.title], ["Kind", (e: Entry) => e.kind], ["Run", (e: Entry) => e.run || "none"], ["Added", (e: Entry) => e.added],
          ["Bytes", (e: Entry) => String(e.bytes)], ["SHA256", (e: Entry) => e.sha256], ["Evidence", (e: Entry) => e.run !== data.run ? "unknown (not read)" : e.evidence.map(l => `${l.criterion}@${l.head.slice(0, 12)}`).join(", ") || "none"]] as const) {
          if (width >= 100) add(`${label}: A ${pick(a)} │ B ${pick(b)}`);
          else { add(`${label} A: ${pick(a)}`); add(`${label} B: ${pick(b)}`); }
        }
      }
    } else if (this.marks.length) add(`Selected for comparison: ${this.marks.map((m, i) => `${i ? "B" : "A"} ${m}`).join(" · ")} · c compares`);
    if (this.opened && this.opened.reference === entry?.reference) {
      add(`Opened ${this.opened.title} · read at action time`);
      for (const line of this.opened.lines) add(line);
    }
    if (this.selected !== undefined && !this.entry(this.selected)) add("Selected artifact no longer in this window · j/k selects an existing row");
    add(`Artifacts ${index + 1}/${data.artifacts.length} · newest first`);
    const start = Math.max(0, Math.min(index - 3, data.artifacts.length - 10));
    data.artifacts.slice(start, start + 10).forEach(a => {
      const slot = this.marks.indexOf(a.reference);
      add(`${a.reference === entry?.reference ? ">" : " "} ${slot < 0 ? "   " : slot === 0 ? "[A]" : "[B]"} ${a.kind} · ${a.title} · ${a.run || "session"} · ${a.purged ? "purged" : a.available ? "present" : "ABSENT"}`, a.reference);
    });
    if (entry) { add(""); add(`Inspect ${entry.title}`); for (const row of this.provenance(entry, data)) add(row); }
    return rows;
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; }
}
