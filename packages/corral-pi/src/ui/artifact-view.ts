import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { extname, isAbsolute, resolve, sep } from "node:path";
import { z } from "zod";
import { getCapabilities, getImageDimensions, matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth, type Component, type ImageSource, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { pad, plainLabel, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { onSurface, VIEW_INSET } from "./frame.ts";
import { backLine, bold, cells, chip, chipHeader, fitColumns, headerRow, listRow, sectionRule, toneColor, wrap, type Tone } from "./panel-parts.ts";
import { SPLIT_MIN } from "./proof-view.ts";

/**
 * Main-view columns from which a comparison puts A and B on one row; below, one row each. The workbench draws this
 * view VIEW_INSET cells narrower than the main view, so the comparison adds them back.
 */
export const COMPARE_ROW_MIN = 100;

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
/** List rows shown around the selection: beside the detail from SPLIT_MIN, above it below. */
const WINDOW_SPLIT = 12, WINDOW_STACKED = 8;
/** The one muted caveat line, longest first; `?` shows the full reading rules instead. */
const CAVEAT = [
  "Catalog at last reconcile, not live · viewers start only on Enter or o · HEAD is a SHA, not a freshness verdict",
  "Catalog at last reconcile, not live · HEAD is a SHA, not a freshness verdict",
  "last reconcile, not live · HEAD ≠ freshness",
  "not live · HEAD ≠ freshness",
];
/** Glyph, word and tone of an artifact's content at read; the word always carries the state. */
function stateOf(entry: Entry): readonly [text: string, tone: Tone] {
  return entry.purged ? ["○ purged", "none"] : entry.available ? ["✓ present", "ok"] : ["! ABSENT", "warn"];
}
/** A muted label column beside wrapped text in `tone`. */
function toned(label: string, value: string, width: number, labels: number, tone: Tone = "none"): string[] {
  const color = tone === "none" ? PANEL_TEXT : toneColor(tone);
  return wrap(plainLabel(value) || "—", width - labels - 1).map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labels)} ${color}${row}${PANEL_TEXT}`);
}

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
  /** The workbench's `?` legend toggle: the full reading rules replace the one-line caveat. */
  readonly legend?: () => boolean;
  /** True while the view has the keys: the selection bar is accent, otherwise muted. */
  readonly focused?: () => boolean;
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
  /** Columns of the list, where a click selects; the end of the `‹ Proof` hit range on row 0. */
  private hitWidth = 0; private backTo = 0;
  private readAt: number | undefined;
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
      this.current = data; this.stale = false; this.error = ""; this.readAt = Date.now();
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
    if (reference === undefined || event.x >= this.hitWidth) return undefined;
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
  /** Select the artifact registered at `path` and open it (Proof's Enter on a criterion artifact). */
  async openPath(path: string): Promise<void> {
    if (!this.current) await this.refresh();
    const entry = this.current?.artifacts.find(a => a.path === path);
    if (!entry) { this.say(`Artifact ${path} is not in the catalog at read · not opened`); return; }
    if (entry.reference !== this.selected) this.opened = undefined;
    this.selected = entry.reference;
    await this.open();
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
  /** True when a click at (`x`, `y`) in this view's own cells lands on the `‹ Proof` back link of row 0. */
  backAt(x: number, y: number): boolean { return y === 0 && x >= 0 && x < this.backTo; }
  /** The selected artifact: title with its state, label/value rows, evidence links and what Enter opened. */
  private detail(entry: Entry, data: ArtifactCatalog, width: number): string[] {
    const [state, tone] = stateOf(entry), slot = this.marks.indexOf(entry.reference);
    const tags = [chip(state, tone), ...(slot < 0 ? [] : [chip(slot ? "B" : "A")])].join(" ");
    const out = [`${PANEL_TEXT}${bold(truncateToWidth(plainLabel(entry.title), Math.max(1, width - visibleWidth(tags) - 2), "…"))}  ${tags}`];
    const verified = this.verified.get(entry.reference), labels = 10;
    out.push(...toned("Reference", entry.reference, width, labels), ...toned("Kind", entry.kind, width, labels),
      ...toned("Run", entry.run || "none (session artifact)", width, labels), ...toned("Added", entry.added, width, labels),
      ...toned("Size", `${entry.bytes.toLocaleString("en-US")} bytes${entry.compressed ? " · stored compressed" : ""}`, width, labels),
      ...toned("Content", entry.purged ? "PURGED (metadata only)" : entry.available ? `present at read${entry.raw ? " (raw snapshot file)" : " (content store)"}`
        : "ABSENT: no snapshot file or live blob at read", width, labels, entry.purged || entry.available ? "none" : "warn"),
      ...toned("Digest", `${entry.sha256.slice(0, 12)} · ${verified === undefined ? "bytes not re-hashed" : verified ? "bytes matched at open" : "BYTES MISMATCHED at open"}`,
        width, labels, verified === false ? "warn" : "none"),
      ...toned("SHA256", entry.sha256, width, labels),
      ...toned("Candidate", "none recorded · only evidence links carry a registration HEAD", width, labels));
    out.push(sectionRule("Evidence", width));
    if (entry.run !== data.run) out.push(...wrap(`${PANEL_MUTED}unknown · run ${plainLabel(entry.run || "none")} evidence not read (bound run ${plainLabel(data.run)} only)${PANEL_TEXT}`, width));
    else if (!entry.evidence.length) out.push(...wrap(`${PANEL_MUTED}none of ${plainLabel(data.run)} names this snapshot · association unknown, not rejected${PANEL_TEXT}`, width));
    for (const link of entry.evidence) {
      const [glyph, mark]: readonly [string, Tone] = link.status === "pass" ? ["✓", "ok"] : link.status === "fail" ? ["✗", "bad"] : ["○", "none"];
      const relation = !data.head ? "bound HEAD unavailable" : link.head === data.head ? "same SHA as bound HEAD at read" : "differs from bound HEAD at read · historical";
      const hash = link.hash_match === null ? "absent" : link.hash_match ? "matches" : `${PANEL_WARNING}MISMATCH${PANEL_MUTED}`;
      out.push(...wrap(`${toneColor(mark)}${glyph}${PANEL_TEXT} ${plainLabel(link.criterion)} · ${plainLabel(link.key)}${PANEL_MUTED} · worker assertion ${link.status.replace("_", " ")} · captured ${plainLabel(link.captured)}${PANEL_TEXT}`, width),
        ...wrap(`${PANEL_MUTED}registered against HEAD ${plainLabel(link.head).slice(0, 12)} (${relation}; freshness not evaluated) · recorded hash ${hash}${PANEL_TEXT}`, width - 2).map(row => `  ${PANEL_MUTED}${row}`));
    }
    if (entry.evidence_omitted) out.push(`${PANEL_MUTED}… ${entry.evidence_omitted} more evidence links omitted${PANEL_TEXT}`);
    if (this.opened?.reference === entry.reference) {
      out.push(sectionRule("Opened", width), ...wrap(`${PANEL_MUTED}${plainLabel(this.opened.title)} · read at action time${PANEL_TEXT}`, width));
      for (const line of this.opened.lines) out.push(...(line.trim() ? wrap(PANEL_TEXT + line.replaceAll("\t", "  "), width) : [""]));
    }
    return out;
  }
  /** A and B in the owner's selection order; one row per field when `side`, else one row each. */
  private comparison(data: ArtifactCatalog, width: number, side: boolean): string[] {
    const out = [sectionRule("Compare", width), ...wrap(`${PANEL_MUTED}A and B in your selection order · no before/after, candidate order or freshness implied${PANEL_TEXT}`, width)];
    const [a, b] = this.marks.map(m => this.entry(m));
    if (!a || !b) return [...out, ...wrap(`${PANEL_WARNING}! A selected artifact is no longer in this window · comparison unavailable${PANEL_TEXT}`, width)];
    out.push(...wrap(PANEL_TEXT + (a.sha256 === b.sha256 ? "Registered bytes identical (same SHA256)" : "Registered bytes differ (different SHA256)"), width));
    const fields = [["Title", (e: Entry) => e.title], ["Kind", (e: Entry) => e.kind], ["Run", (e: Entry) => e.run || "none"], ["Added", (e: Entry) => e.added],
      ["Bytes", (e: Entry) => e.bytes.toLocaleString("en-US")], ["SHA256", (e: Entry) => e.sha256.slice(0, 12)],
      ["Evidence", (e: Entry) => e.run !== data.run ? "unknown (not read)" : e.evidence.map(l => `${l.criterion}@${l.head.slice(0, 12)}`).join(", ") || "none"]] as const;
    if (!side) {
      for (const [label, pick] of fields) out.push(...toned(`${label} A`, pick(a), width, 11), ...toned(`${label} B`, pick(b), width, 11));
      return [...out, ""];
    }
    const column = Math.max(4, Math.floor((width - 9 - 3) / 2));
    out.push(`${PANEL_MUTED}${pad("", 8)} ${pad(bold("A"), column)}${PANEL_RULE} │ ${PANEL_MUTED}${bold("B")}${PANEL_TEXT}`);
    for (const [label, pick] of fields)
      out.push(`${PANEL_MUTED}${pad(label, 8)}${PANEL_TEXT} ${pad(plainLabel(pick(a)), column)}${PANEL_RULE} │ ${PANEL_TEXT}${truncateToWidth(plainLabel(pick(b)), column, "…")}`);
    return [...out, ""];
  }
  /** The list: a muted header row, then a window of `window` rows around the selection with `↑`/`↓ n more`. */
  private list(items: readonly Entry[], index: number, width: number, window: number): { readonly text: string; readonly reference?: string }[] {
    const run = { label: "run", width: Math.min(8, Math.max(3, ...items.map(a => visibleWidth(plainLabel(a.run || "session"))))) };
    const kind = { label: "kind", width: Math.min(12, Math.max(4, ...items.map(a => visibleWidth(plainLabel(a.kind))))) };
    const title = { label: "title", width: 10 }, state = { label: "state", width: 9 };
    const columns = fitColumns([{ columns: [kind, title, run, state], flex: 1 }, { columns: [title, run, state], flex: 0 }, { columns: [title, state], flex: 0 }], width - 3);
    const focused = this.options.focused?.() ?? true;
    const start = Math.max(0, Math.min(index - Math.floor(window / 3), items.length - window));
    const rows: { text: string; reference?: string }[] = [{ text: headerRow(columns, width) }];
    if (start > 0) rows.push({ text: listRow(`${PANEL_MUTED}↑ ${start} more${PANEL_TEXT}`, "", false, width) });
    for (const a of items.slice(start, start + window)) {
      const [word, tone] = stateOf(a), slot = this.marks.indexOf(a.reference), on = a === items[index];
      const values: Readonly<Record<string, string>> = {
        kind: PANEL_MUTED + plainLabel(a.kind), run: PANEL_MUTED + plainLabel(a.run || "session"), state: toneColor(tone) + word,
        title: (slot < 0 ? "" : chip(slot ? "B" : "A") + " ") + PANEL_TEXT + plainLabel(a.title),
      };
      const text = cells(columns, columns.map(c => values[c.label] ?? "")) + PANEL_TEXT;
      rows.push({ text: listRow(on ? onSurface(text, PANEL_SELECTED) : text, "", on, width, focused), reference: a.reference });
    }
    const after = items.length - start - window;
    if (after > 0) rows.push({ text: listRow(`${PANEL_MUTED}↓ ${after} more${PANEL_TEXT}`, "", false, width) });
    return rows;
  }
  render(width: number): string[] {
    const lines: string[] = []; this.hits.clear();
    const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
    const { identity } = this.options, data = this.current, items = data?.artifacts ?? [];
    const back = backLine("Proof", "Artifacts", `${identity.task ?? "unbound"} / ${identity.run ?? "unbound"}`, width);
    this.backTo = back.to; lines.push(back.line);
    const bound = this.options.bound(), stale = this.stale || !bound;
    const absent = items.filter(a => !a.purged && !a.available).length, purged = items.filter(a => a.purged).length;
    lines.push(...chipHeader([
      stale ? chip("~ stale", "warn") : chip("canonical catalog"),
      ...(bound ? [] : [chip("! unbound", "warn")]),
      ...(this.refreshing ? [chip("reading… rows retained")] : []),
      ...(data ? [chip(`${items.length} ${items.length === 1 ? "artifact" : "artifacts"}, newest first`)] : []),
      ...(absent ? [chip(`! ${absent} absent`, "warn")] : []),
      ...(purged ? [chip(`○ ${purged} purged`)] : []),
      ...(data?.more ? [chip(`! GAP: newest ${items.length} rows only`, "warn")] : []),
      ...(data ? [chip(data.head ? `HEAD ${plainLabel(data.head).slice(0, 12)}` : "HEAD unavailable")] : []),
      ...this.marks.map((m, i) => chip(`${i ? "B" : "A"} ${stripTerminalSequences(truncateToWidth(plainLabel(this.entry(m)?.title ?? m), 24, "…"))}`)),
    ], width, this.readAt, Date.now()));
    if (stale) full(`${PANEL_WARNING}~ STALE / disconnected · ${plainLabel(this.error || "retained catalog; reconcile")}${PANEL_TEXT}`);
    if (this.notice) full(PANEL_TEXT + plainLabel(this.notice));
    if (this.options.legend?.()) {
      full(`${PANEL_MUTED}Read-only canonical catalog at the last reconcile, not a live subscription. Viewers start only on Enter or o, never on focus or selection.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}Bound worktree HEAD ${plainLabel(data?.head ?? "unavailable")} at read · a SHA, not a freshness verdict. Artifact rows record no candidate; only evidence links carry a registration HEAD.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}✓ present · ! ABSENT: no snapshot file or live blob at read · ○ purged: metadata only. A and B mark the two artifacts chosen with m, in your selection order.${PANEL_TEXT}`);
    } else {
      const caveat = CAVEAT.find(c => visibleWidth(c) <= width) ?? CAVEAT.at(-1) ?? "";
      lines.push(PANEL_MUTED + truncateToWidth(caveat, width, "…") + PANEL_TEXT);
    }
    if (!data) { full(`${PANEL_MUTED}Unavailable · no empty catalog inferred${PANEL_TEXT}`); return lines; }
    if (!items.length) { full(`${PANEL_MUTED}${data.more ? "No rows in this window" : "No artifacts registered for this task"}${PANEL_TEXT}`); return lines; }
    if (this.selected !== undefined && !this.entry(this.selected)) full(`${PANEL_WARNING}! Selected artifact no longer in this window · j/k selects an existing row${PANEL_TEXT}`);
    const index = Math.max(0, items.findIndex(a => a.reference === this.selected)), entry = items[index];
    const wide = width + VIEW_INSET >= SPLIT_MIN;
    const listWidth = wide ? Math.max(46, Math.min(60, Math.floor(width * 0.45))) : width;
    const paneWidth = wide ? Math.max(20, width - listWidth - 2) : width;
    const list = this.list(items, index, listWidth, wide ? WINDOW_SPLIT : WINDOW_STACKED);
    const pane = [...(this.compare ? this.comparison(data, paneWidth, width + VIEW_INSET >= COMPARE_ROW_MIN) : []), ...(entry ? this.detail(entry, data, paneWidth) : [])];
    this.hitWidth = listWidth;
    if (wide) {
      lines.push(`${PANEL_RULE}${"─".repeat(listWidth)}┬${"─".repeat(Math.max(0, width - listWidth - 1))}${PANEL_TEXT}`);
      const start = lines.length;
      for (let i = 0; i < Math.max(list.length, pane.length); i++) {
        const row = list[i];
        if (row?.reference !== undefined) this.hits.set(start + i, row.reference);
        lines.push(`${row?.text ?? " ".repeat(listWidth)}${PANEL_RULE}│${PANEL_TEXT} ${pane[i] ?? ""}`);
      }
    } else {
      for (const row of list) { if (row.reference !== undefined) this.hits.set(lines.length, row.reference); lines.push(row.text); }
      lines.push(`${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`, ...pane);
    }
    return lines.map(line => truncateToWidth(line, width, "…"));
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; }
}
