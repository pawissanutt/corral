import { createHash } from "node:crypto";
import type { HostIdentity } from "../roles/types.ts";
import { target } from "./chrome.ts";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { VStack, ScrollView, Text, matchesKey, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { anchoredRow } from "./reading-anchor.ts";
import { NativeImageViewer } from "./image-viewer.ts";
import { ReviewStore, COMPARISONS, comparisonLabel, fileStatus } from "./review-store.ts";
import { activityRows } from "./activity-view.ts";
import { InspectionStore, evidenceLines } from "./inspection.ts";
import { pad, plainLabel, diffLine, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_BG, RESET, type PanelSnapshot, type PanelLine } from "./panels-view.ts";
export function plainClick(event: TuiMouseEvent): boolean {
  return event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && (event.clickCount ?? 1) === 1;
}
export type MainView = "conversation" | "changes" | "activity" | "evidence";
export interface WorkbenchOptions {
  readonly identity: HostIdentity;
  readonly inspection?: () => Promise<unknown>;
  readonly cwd: string; readonly snapshot: () => PanelSnapshot | null;
  readonly changed: () => void; readonly canInteract: () => boolean;
}
/** Main-area composition via the packaged SDK's public mainView hook; editor stays SDK-owned. */
export class NativeWorkbench {
  view: MainView = "conversation"; focused = false;
  readonly review: ReviewStore;
  readonly inspection: InspectionStore;
  private evidenceScroll: ScrollView | undefined; private criterionIndex = 0; private criterionHits = new Map<number, number>();
  readonly images: NativeImageViewer;
  private ui: ExtensionUIContext | undefined;
  private changesScroll: ScrollView | undefined; private activityScroll: ScrollView | undefined;
  private transcript: ScrollView | undefined; private transcriptWidth = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private debounce: ReturnType<typeof setTimeout> | undefined;
  private jobIndex = 0; private jobHits = new Map<number, number>(); private hunk = 0; private hunkRows: number[] = [];
  private hunkSources: number[] = []; private lastPatch = ""; private displayedPath: string | undefined;
  private displayedRoot: string | undefined;
  private displayedBase: string | undefined;
  private displayedAt: number | undefined; private displayedHead: string | undefined; private displayedComparison = "";
  constructor(private readonly options: WorkbenchOptions) {
    this.images = new NativeImageViewer(() => options.canInteract(), () => { this.focused = this.images.isOpen; options.changed(); });
    this.inspection = new InspectionStore(options.identity, options.inspection, () => options.changed());
    this.review = new ReviewStore(options.cwd, () => options.changed(), options.inspection && options.identity.task && options.identity.run ? async () => {
      await this.inspection.refresh();
      if (this.inspection.stale || !this.inspection.current) throw new Error(this.inspection.error ?? "Canonical candidate unavailable; r to reconcile");
      return this.inspection.current;
    } : undefined);
  }
  bind(ui: ExtensionUIContext): void { this.ui = ui; this.images.bind(); }
  invalidateFiles(): void {
    this.inspection.invalidate();
    if (this.debounce) clearTimeout(this.debounce);
    this.debounce = setTimeout(() => { this.debounce = undefined; void this.review.refresh(); }, 300);
    this.debounce.unref();
  }
  setView(view: MainView): void {
    this.images.close();
    this.view = view; this.focused = view !== "conversation";
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    if (view !== "conversation") {
      if (view === "changes") void this.review.refresh();
      if (view === "evidence") void this.inspection.refresh();
      let ticks = 0;
      this.timer = setInterval(() => {
        ticks++;
        if (this.view === "changes" && ticks % 2 === 0) void this.review.refresh();
        if (this.view === "evidence" && ticks % 10 === 0) void this.inspection.refresh();
        this.options.changed();
      }, 1000); this.timer.unref();
    }
    this.options.changed();
  }
  async openFile(path: string): Promise<void> {
    this.setView("changes"); await this.review.select(path); this.acceptPatch();
  }
  private nav(): Component {
    let hits: { readonly to: number; readonly view: MainView }[] = [];
    return {
      render: width => {
        this.anchorTranscript(width);
        const compact = width < 90;
        const labels: readonly [MainView, string][] = [["conversation", compact ? " Chat F9 " : " Conversation F9 "],
          ["changes", compact ? " Diff F7 " : " Changes F7 "], ["activity", compact ? " Act F8 " : " Activity F8 "], ["evidence", compact ? " Proof F10 " : " Evidence F10 "]];
        let x = 0;
        hits = labels.map(([view, label]) => ({ view, to: x += label.length }));
        return [PANEL_BG + pad(labels.map(([view, label]) => (this.view === view ? PANEL_BLUE : PANEL_MUTED) + label).join("") + PANEL_MUTED +
          (this.focused ? compact ? "· Esc · Enter" : "· Esc chat · Enter compose" : ""), width) + RESET,
        PANEL_BG + PANEL_MUTED + pad(`${this.options.identity.role.toUpperCase()} ${target(this.options.identity)} · g${this.options.identity.generation} · ${!this.ui ? "Initializing editor…" : this.options.snapshot()?.bound ? "Bound to Corral" : "Unbound"}`, width) + RESET];
      },
      handleMouse: event => {
        if (!this.options.canInteract() || !plainClick(event) || event.y !== 0) return undefined;
        const hit = hits.find(hit => event.x < hit.to);
        if (!hit) return undefined;
        this.setView(hit.view); return { handled: true };
      }, invalidate() {},
    };
  }
  /**
   * Runs in the main-view bar's render, before Pi lays out the transcript at a new width. A reader
   * who paused follow keeps the same passage; following readers stay live through Pi itself.
   */
  private anchorTranscript(width: number): void {
    const transcript = this.transcript;
    if (!transcript || this.view !== "conversation" || this.images.isOpen) return;
    const previous = this.transcriptWidth;
    this.transcriptWidth = width;
    if (!previous || previous === width || transcript.isFollowingEnd || transcript.scrollTop === 0) return;
    const before = transcript.render(previous), after = transcript.render(width);
    const top = anchoredRow(before, transcript.scrollTop, after);
    transcript.updateLayout(after.length, transcript.viewportHeight, () => {});
    transcript.scrollTo(top, { disableFollow: true });
  }
  mount(transcript: Component): Component {
    if (transcript instanceof ScrollView) this.transcript = transcript;
    this.changesScroll = new ScrollView({ render: width => this.renderChanges(width), handleMouse: event => this.diffMouse(event), invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.activityScroll = new ScrollView({ render: width => this.renderActivity(width), handleMouse: event => {
      if (this.options.canInteract() && plainClick(event)) { this.focused = true; const job = this.jobHits.get(event.y); if (job !== undefined) this.jobIndex = job; this.options.changed(); return { handled: true }; } return undefined;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.evidenceScroll = new ScrollView({ render: width => this.renderEvidence(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      const index = this.criterionHits.get(event.y); if (index !== undefined) this.criterionIndex = index;
      this.focused = true; this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    return new VStack([
      { component: this.nav(), basis: 2, shrink: 0 },
      { component: transcript, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "conversation" && !this.images.isOpen },
      { component: this.images, basis: 0, grow: 1, minSize: 1, visible: viewport => { this.images.setViewportHeight(Math.max(0, viewport.height - 2)); return this.images.isOpen; } },
      { component: this.changesScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "changes" },
      { component: this.activityScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "activity" },
      { component: this.evidenceScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "evidence" },
    ]);
  }
  private renderEvidence(width: number): string[] {
    this.criterionHits.clear(); const rows: string[] = [];
    this.criterionIndex = Math.max(0, Math.min(this.criterionIndex, (this.inspection.current?.criteria.length ?? 1) - 1));
    for (const row of evidenceLines(this.inspection, this.criterionIndex, width)) {
      const wrapped = new Text((row.heading ? PANEL_BLUE : PANEL_TEXT) + row.text, 0, 0).render(width);
      if (row.criterion !== undefined) for (let i = 0; i < wrapped.length; i++) this.criterionHits.set(rows.length + i, row.criterion);
      rows.push(...wrapped);
    }
    return rows;
  }
  private renderActivity(width: number): string[] {
    this.jobHits.clear(); const rows: string[] = [];
    for (const row of activityRows(this.options.snapshot(), this.jobIndex)) {
      const line = row.text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
      const wrapped = new Text(this.ui ? this.ui.theme.fg(row.color ?? "muted", line) : line, 0, 0).render(width);
      if (row.job !== undefined) { for (let i = 0; i < wrapped.length; i++) this.jobHits.set(rows.length + i, row.job); }
      rows.push(...wrapped);
    }
    return rows;
  }
  private acceptPatch(): void {
    const snapshot = this.review.patchSnapshot;
    if (!snapshot || snapshot.root !== this.review.root || snapshot.path !== this.review.selected || snapshot.comparison !== this.review.comparison) return;
    this.lastPatch = snapshot.text; this.displayedPath = snapshot.path; this.displayedRoot = snapshot.root;
    this.displayedAt = snapshot.at; this.displayedHead = snapshot.head; this.displayedBase = snapshot.base;
    this.displayedComparison = comparisonLabel(snapshot.comparison); this.hunk = 0;
    this.options.changed();
  }
  private renderChanges(width: number): string[] {
    const r = this.review;
    const label = comparisonLabel(r.comparison);
    const sameScope = this.displayedRoot === r.root && this.displayedPath === r.selected && this.displayedComparison === label;
    const base = sameScope ? this.displayedHead : r.patchSnapshot?.comparison === r.comparison ? r.patchSnapshot.head : undefined;
    const state = r.error ? "STALE / unavailable: " + plainLabel(r.error) : r.refreshing ? "Checking Git…" : r.checkedAt ? "Updated " + new Date(r.checkedAt).toLocaleTimeString() : "Not loaded";
    const head = [PANEL_BLUE + `${plainLabel(r.name)} (${plainLabel(r.branch)})` + PANEL_TEXT,
      ...(width >= 90 ? [PANEL_MUTED + plainLabel(r.root)] : []), PANEL_MUTED + label + (width >= 90 && r.comparison !== "candidate" ? ` · base ${base?.slice(0, 10) ?? "unborn/unavailable"}` : ""),
      ...(r.comparison === "candidate" ? [PANEL_MUTED + `Comparison base ${sameScope ? this.displayedBase ?? "unavailable" : r.candidateBase ?? "unavailable"}`,
        PANEL_MUTED + `Candidate HEAD ${base ?? r.head ?? "unavailable"} · committed only; uncommitted edits excluded`] : []),
      PANEL_MUTED + state, PANEL_MUTED + (width >= 90 ? "/ search · c compare · o repository · j/k files · n/p hunks · a ask hunk · r refresh" : "j/k files · n/p hunks · a ask · / filter"), ""];
    if (r.limitNotice) head.push(PANEL_MUTED + r.limitNotice);
    const file = r.current;
    const stat = file ? r.stats.get(file.path) : undefined;
    head.push(PANEL_BLUE + (file ? `${plainLabel(file.path)} · ${fileStatus(file)}` : "No selected file"),
      ...(file ? [PANEL_MUTED + (stat ? stat.added === null ? "Diffstat: binary" : `Diffstat: +${stat.added} −${stat.removed}` : "Diffstat: unavailable")] : []), "");
    if (!file) return [...head.flatMap(line => new Text(line, 0, 0).render(width)), plainLabel(r.refreshing ? "Loading comparison…" : r.error ?? (r.selected ? "Selected file is no longer in this comparison." : "No changes in this comparison."))];
    if (r.patchPath !== r.selected || r.patchComparison !== r.comparison) return [...head.map(line => pad(line, width)), "Loading selected diff…"];
    if (this.displayedPath !== r.selected || this.displayedRoot !== r.root || this.displayedComparison !== comparisonLabel(r.comparison) || !this.lastPatch) this.acceptPatch();
    if (this.displayedRoot !== r.root || this.displayedPath !== r.selected || this.displayedComparison !== label) {
      return [...head.flatMap(line => new Text(line, 0, 0).render(width)), r.patch.startsWith("Diff unavailable:") ? plainLabel(r.patch) : "Waiting for a captured diff matching this scope…"];
    }
    if (!r.patchSnapshot && r.patch.startsWith("Diff unavailable:")) return [...head.flatMap(line => new Text(line, 0, 0).render(width)), plainLabel(r.patch)];
    if (r.patch !== this.lastPatch || r.patchSnapshot?.head !== this.displayedHead || r.patchSnapshot?.base !== this.displayedBase) head.push(PANEL_BLUE + "New diff available (content or identity) · r to apply; current hunk/scroll preserved");
    const result = head.flatMap(line => new Text(line, 0, 0).render(width));
    this.hunkRows = []; this.hunkSources = [];
    const source = this.lastPatch.split("\n");
    let oldLine: number | undefined, newLine: number | undefined;
    source.forEach((line, index) => {
      if (line.startsWith("@@")) { this.hunkRows.push(result.length); this.hunkSources.push(index); }
      if (line.startsWith("diff ")) { oldLine = undefined; newLine = undefined; }
      const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
      if (hunk) { oldLine = Number(hunk[1]); newLine = Number(hunk[2]); }
      let gutter = "";
      const inHunk = oldLine !== undefined && newLine !== undefined && !hunk && /^[ +\-]/.test(line);
      if (inHunk && oldLine !== undefined && newLine !== undefined) {
        const before = line.startsWith("+") ? "" : String(oldLine++);
        const after = line.startsWith("-") ? "" : String(newLine++);
        gutter = PANEL_MUTED + before.padStart(4) + " " + after.padStart(4) + " │ " + PANEL_TEXT;
      }
      const colored = this.ui ? diffLine(line, this.ui.theme, inHunk) : plainLabel(line);
      result.push(...new Text(gutter + colored, 0, 0).render(width));
    });
    return result;
  }
  private diffMouse(event: TuiMouseEvent) {
    if (!this.options.canInteract() || !plainClick(event)) return undefined;
    this.focused = true;
    for (let i = 0; i < this.hunkRows.length; i++) if ((this.hunkRows[i] ?? Infinity) <= event.y) this.hunk = i;
    this.options.changed(); return { handled: true };
  }
  files(): PanelLine[] {
    const r = this.review;
    const rows: PanelLine[] = [{ text: `${r.name} · ${r.branch}`, heading: true },
      ...(r.comparison === "candidate" ? [{ text: `Base ${r.candidateBase ?? "unavailable"}` }, { text: `HEAD ${r.head ?? "unavailable"}` }] : []),
      { text: `${r.checkedAt ? r.visible.length + " matching" : "Loading / unavailable"} · ${r.comparison}${r.query ? " · /" + r.query : ""}` }];
    if (r.error) rows.push({ text: "STALE: " + plainLabel(r.error) });
    for (const file of r.visible) {
      const stat = r.stats.get(file.path);
      const delta = stat ? stat.added === null ? " · binary" : ` +${stat.added} −${stat.removed}` : "";
      rows.push({ text: `${file.path === r.selected ? ">" : " "}${delta} ${file.path} · ${fileStatus(file)}`, action: { kind: "diff", path: file.path } });
    }
    if (!r.visible.length) rows.push({ text: r.error ? "Git unavailable · r to retry" : r.refreshing ? "Reading Git…" : r.files.length ? "No matching files · / to search" : r.comparison === "candidate" ? "No committed candidate changes" : "Working tree clean" });
    if (r.limitNotice) rows.push({ text: r.limitNotice });
    // PanelLine text is data, not terminal markup. Keep action paths literal for Git.
    return rows.map(row => ({ ...row, text: plainLabel(row.text) }));
  }
  async selectRelative(delta: number): Promise<void> {
    const rows = this.review.visible, from = rows.findIndex(file => file.path === this.review.selected);
    const file = rows[Math.max(0, Math.min(rows.length - 1, from + delta))];
    if (file) { await this.review.select(file.path); this.acceptPatch(); this.changesScroll?.scrollToStart(); }
  }
  private async search(): Promise<void> {
    const value = await this.ui?.input("Filter changed paths", this.review.query);
    if (value === undefined) return;
    this.review.query = value;
    if (!this.review.visible.some(file => file.path === this.review.selected)) {
      const file = this.review.visible[0]; if (file) await this.review.select(file.path);
    }
    this.options.changed();
  }
  private async repository(): Promise<void> {
    const value = await this.ui?.input("Read-only repository scope (does not rebind this session)", this.review.root);
    if (value) { this.lastPatch = ""; this.displayedPath = undefined; await this.review.setRepository(value); this.changesScroll?.scrollToStart(); }
  }
  private moveHunk(delta: number): void {
    this.hunk = Math.max(0, Math.min(this.hunkRows.length - 1, this.hunk + delta));
    const row = this.hunkRows[this.hunk]; if (row !== undefined) this.changesScroll?.scrollTo(row, { disableFollow: true });
    this.options.changed();
  }
  askHunk(): void {
    const ui = this.ui, file = this.review.current, start = this.hunkSources[this.hunk];
    if (!ui || !file) return;
    if (this.displayedPath !== file.path || this.displayedRoot !== this.review.root || this.review.patchComparison !== this.review.comparison || this.displayedComparison !== comparisonLabel(this.review.comparison)) {
      ui.notify("Wait for the selected comparison to load before attaching its hunk.", "warning"); return;
    }
    const source = this.lastPatch.split("\n");
    const end = this.hunkSources[this.hunk + 1] ?? source.length;
    const excerpt = start === undefined ? source.slice(0, 18) : source.slice(start, Math.min(end, start + 24));
    const reference = `Please review ${JSON.stringify(file.path)} in ${this.review.root}\nComparison: ${this.displayedComparison} · base ${this.displayedBase ?? "see comparison"} · HEAD ${this.displayedHead ?? "unborn/unavailable"}\nDiff SHA256: ${createHash("sha256").update(this.lastPatch).digest("hex")}\nSelected Git excerpt (observed ${this.displayedAt ? new Date(this.displayedAt).toISOString() : "time unavailable"}):\n` + excerpt.map(line => "> " + line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")).join("\n").slice(0, 3000);
    const draft = ui.getEditorText(); ui.setEditorText(draft + (draft ? "\n\n" : "") + reference + "\n");
    this.focused = false; this.options.changed(); // Never submit or overwrite the existing draft.
  }
  handleInput(data: string): boolean {
    if (matchesKey(data, "f9")) { this.setView("conversation"); this.focused = false; return true; }
    if (matchesKey(data, "f7")) { this.setView("changes"); return true; }
    if (matchesKey(data, "f8")) { this.setView("activity"); return true; }
    if (matchesKey(data, "f10")) { this.setView("evidence"); return true; }
    if (this.images.isOpen && matchesKey(data, "escape")) { this.images.close(); return true; }
    if (!this.focused || this.view === "conversation") return false;
    if (matchesKey(data, "escape")) { this.setView("conversation"); this.focused = false; return true; }
    if (matchesKey(data, "enter") || matchesKey(data, "tab")) { this.focused = false; this.options.changed(); return true; }
    const scroll = this.view === "changes" ? this.changesScroll : this.view === "evidence" ? this.evidenceScroll : this.activityScroll;
    if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      scroll?.scrollBy(matchesKey(data, "up") ? -3 : matchesKey(data, "down") ? 3 : matchesKey(data, "pageUp") ? -12 : 12); this.options.changed(); return true;
    }
    if (data === "j" || data === "k") {
      if (this.view === "changes") void this.selectRelative(data === "j" ? 1 : -1);
      else if (this.view === "evidence") this.criterionIndex = Math.max(0, Math.min((this.inspection.current?.criteria.length ?? 1) - 1, this.criterionIndex + (data === "j" ? 1 : -1)));
      else this.jobIndex = Math.max(0, Math.min((this.options.snapshot()?.jobs?.length ?? 1) - 1, this.jobIndex + (data === "j" ? 1 : -1)));
      this.options.changed(); return true;
    }
    if (this.view === "evidence" && data === "r") { void this.inspection.refresh(true); return true; }
    if (this.view === "changes") {
      if (data === "/") { void this.search(); return true; }
      if (data === "o") { void this.repository(); return true; }
      if (data === "c") { const next = COMPARISONS[(COMPARISONS.indexOf(this.review.comparison) + 1) % COMPARISONS.length]; if (next) { this.lastPatch = ""; void this.review.setComparison(next); } return true; }
      if (data === "r") { this.inspection.invalidate(); void this.review.refresh().then(() => this.acceptPatch()); return true; }
      if (data === "n" || data === "p") { this.moveHunk(data === "n" ? 1 : -1); return true; }
      if (data === "a") { this.askHunk(); return true; }
    }
    if (data.startsWith("\x1b")) return false; // Terminal replies/focus events are not typing.
    this.focused = false; return false;
  }
  dispose(): void {
    if (this.timer) clearInterval(this.timer); if (this.debounce) clearTimeout(this.debounce);
    this.images.dispose(); this.review.dispose(); this.inspection.dispose(); this.ui = undefined;
  }
}
