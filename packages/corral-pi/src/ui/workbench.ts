import { createHash } from "node:crypto";
import { basename, dirname } from "node:path";
import { chooseProject, projectScopes } from "./project-scopes.ts";
import type { HostIdentity } from "../roles/types.ts";
import { target } from "./chrome.ts";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { VStack, ScrollView, Text, matchesKey, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { anchoredRow } from "./reading-anchor.ts";
import { NativeImageViewer } from "./image-viewer.ts";
import { ReviewStore, comparisonLabel, fileStatus, type PatchSnapshot } from "./review-store.ts";
import { activityRows } from "./activity-view.ts";
import { agentRows } from "./agent-view.ts";
import { InspectionStore, evidenceLines } from "./inspection.ts";
import { DecisionInbox, inboxIdentity } from "./decision-inbox.ts";
import { InboxView } from "./inbox-view.ts";
import { pad, plainLabel, diffLine, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_BG, RESET, type PanelSnapshot, type PanelLine } from "./panels-view.ts";
export function plainClick(event: TuiMouseEvent): boolean {
  return event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && (event.clickCount ?? 1) === 1;
}
export type MainView = "conversation" | "changes" | "activity" | "agents" | "evidence" | "inbox";
export interface WorkbenchOptions {
  readonly identity: HostIdentity;
  readonly inspection?: () => Promise<unknown>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly cwd: string; readonly snapshot: () => PanelSnapshot | null;
  readonly changed: () => void; readonly canInteract: () => boolean;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
}
/** Main-area composition via the packaged SDK's public mainView hook; editor stays SDK-owned. */
export class NativeWorkbench {
  view: MainView = "conversation"; focused = false;
  readonly review: ReviewStore;
  readonly inspection: InspectionStore;
  private evidenceScroll: ScrollView | undefined; private criterionIndex = 0; private criterionHits = new Map<number, number>();
  readonly images: NativeImageViewer;
  readonly inbox: DecisionInbox; private readonly inboxView: InboxView;
  private inboxScroll: ScrollView | undefined;
  private ui: ExtensionUIContext | undefined;
  private changesScroll: ScrollView | undefined; private activityScroll: ScrollView | undefined;
  private agentsScroll: ScrollView | undefined; private agentId: string | undefined;
  private agentHits = new Map<number, string>(); private readingTranscript = false;
  private transcript: ScrollView | undefined; private transcriptWidth = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private debounce: ReturnType<typeof setTimeout> | undefined;
  private jobId: string | undefined; private jobHits = new Map<number, string>(); private hunk = 0; private hunkRows: number[] = [];
  private hunkSources: number[] = []; private lastPatch = ""; private displayedPath: string | undefined;
  private displayedRoot: string | undefined;
  private displayedBase: string | undefined;
  private displayedMetadata: Pick<PatchSnapshot, "file" | "stat"> | undefined;
  private displayedAt: number | undefined; private displayedHead: string | undefined; private displayedComparison = "";
  private projectName: string | undefined;
  private choosingScope = false;
  private readonly scopeAbort = new AbortController();
  get scopeLabel(): string { return this.review.scopeRequired ? "Choose project" : this.projectName ?? this.review.name; }
  constructor(private readonly options: WorkbenchOptions) {
    this.images = new NativeImageViewer(() => options.canInteract(), () => { this.focused = this.images.isOpen; options.changed(); });
    this.inspection = new InspectionStore(options.identity, options.inspection, () => options.changed());
    this.projectName = options.identity.project || undefined;
    this.review = new ReviewStore(options.projectDir ?? options.cwd, () => options.changed(), options.inspection && options.identity.task && options.identity.run ? async () => {
      await this.inspection.refresh();
      if (this.inspection.stale || !this.inspection.current) throw new Error(this.inspection.error ?? "Canonical candidate unavailable; r to reconcile");
      return this.inspection.current;
    } : undefined, !!options.readProjects && !options.identity.task && !options.projectDir, !!options.projectDir);
    this.inbox = new DecisionInbox(options.readInbox, options.changed);
    this.inboxView = new InboxView(this.inbox, options.identity.role, options.identity.project, options.canInteract, options.changed, () => { this.focused = true; });
  }
  bind(ui: ExtensionUIContext): void { this.ui = ui; this.images.bind(); this.inboxView.bind(ui); }
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
      if (view === "inbox") void this.inbox.refresh();
      let ticks = 0;
      this.timer = setInterval(() => {
        ticks++;
        if (this.view === "changes" && ticks % 2 === 0) void this.review.refresh();
        if (this.view === "evidence" && ticks % 10 === 0) void this.inspection.refresh();
        if (this.view === "inbox" && ticks % 5 === 0) void this.inbox.refresh();
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
        const labels: readonly [MainView, string][] = [["conversation", compact ? " Chat " : " Conversation F9 "],
          ["changes", compact ? " Diff " : " Changes F7 "], ["activity", compact ? " Jobs " : " Activity F8 "],
          ["evidence", compact ? " Proof " : " Evidence F10 "], ["inbox", " Inbox F11 "]];
        let x = 0;
        hits = labels.map(([view, label]) => ({ view, to: x += label.length }));
        return [PANEL_BG + pad(labels.map(([view, label]) => (this.view === view || view === "activity" && this.view === "agents" ? PANEL_BLUE : PANEL_MUTED) + label).join("") + PANEL_MUTED +
          (this.focused ? compact ? "· Esc · Enter" : "· Esc chat · Enter compose" : ""), width) + RESET,
        PANEL_BG + PANEL_TEXT + pad(`Files: ${plainLabel(this.scopeLabel)} · ` + PANEL_MUTED + `${this.options.identity.role.toUpperCase()} ${target(this.options.identity)} · g${this.options.identity.generation} · ${!this.ui ? "Initializing editor…" : this.options.snapshot()?.bound ? "Bound to Corral" : "Unbound"}`, width) + RESET];
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
      if (this.options.canInteract() && plainClick(event)) { this.focused = true; const job = this.jobHits.get(event.y); if (job !== undefined) this.jobId = job; this.options.changed(); return { handled: true }; } return undefined;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.agentsScroll = new ScrollView({ render: width => this.renderAgents(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      const id = this.agentHits.get(event.y); if (id) this.agentId = id;
      this.focused = true; this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.evidenceScroll = new ScrollView({ render: width => this.renderEvidence(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      const index = this.criterionHits.get(event.y); if (index !== undefined) this.criterionIndex = index;
      this.focused = true; this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.inboxScroll = new ScrollView(this.inboxView, { primary: true, follow: "none", scrollbar: "auto" });
    return new VStack([
      { component: this.nav(), basis: 2, shrink: 0 },
      { component: transcript, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "conversation" && !this.images.isOpen },
      { component: this.images, basis: 0, grow: 1, minSize: 1, visible: viewport => { this.images.setViewportHeight(Math.max(0, viewport.height - 2)); return this.images.isOpen; } },
      { component: this.changesScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "changes" },
      { component: this.activityScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "activity" },
      { component: this.agentsScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agents" },
      { component: this.evidenceScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "evidence" },
      { component: this.inboxScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "inbox" },
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
  private renderAgents(width: number): string[] {
    this.agentHits.clear(); const result: string[] = [], snapshot = this.options.snapshot();
    this.agentId ??= snapshot?.agents?.[0]?.id;
    for (const row of agentRows(this.options.identity, snapshot?.agents, snapshot?.bound ?? false, this.agentId)) {
      const wrapped = new Text(row.text, 0, 0).render(width);
      if (row.agent) for (let i = 0; i < wrapped.length; i++) this.agentHits.set(result.length + i, row.agent);
      result.push(...wrapped);
    }
    return result;
  }
  async openAgentTranscript(): Promise<void> {
    const ui = this.ui, id = this.agentId, snapshot = this.options.snapshot();
    if (!ui || !this.options.canInteract() || this.readingTranscript) return;
    if (!id || !snapshot?.bound || !snapshot.agents?.find(agent => agent.id === id)?.transcript || !this.options.readTranscript) {
      ui.notify("Selected agent missing, stale or transcript ineligible", "warning"); return;
    }
    this.readingTranscript = true;
    try {
      const text = await this.options.readTranscript(id);
      if (this.scopeAbort.signal.aborted) return;
      const current = this.options.snapshot();
      if (this.agentId !== id || !current?.bound || !current.agents?.find(agent => agent.id === id)?.transcript) throw new Error("Agent selection or binding changed during read");
      // Read-only bounded text, never an SDK switchSession/open/resume call.
      await ui.custom<void>((tui, _theme, _keys, done) => {
        const content = new Text(text.slice(0, 32 * 1024).replace(/[\u0000-\u001f\u007f-\u009f]/g, char => char === "\n" ? char : " "), 0, 0);
        let offset = 0, max = 0;
        return { render: width => {
          const lines = content.render(width), height = Math.max(1, Math.floor(tui.terminal.rows * 0.8) - 2);
          max = Math.max(0, lines.length - height); offset = Math.max(0, Math.min(max, offset));
          return [pad("Transcript · Esc close · ↑/↓ scroll", width), ...lines.slice(offset, offset + height)];
        }, handleInput: data => {
            if (matchesKey(data, "escape") || data === "q") { done(); return; }
            if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) offset = Math.max(0, Math.min(max, offset + (matchesKey(data, "up") ? -1 : matchesKey(data, "down") ? 1 : matchesKey(data, "pageUp") ? -10 : 10)));
            tui.requestRender();
          }, invalidate() { content.invalidate(); } };
      }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
    } catch (error) {
      if (!this.scopeAbort.signal.aborted) ui.notify(`Transcript unavailable: ${plainLabel(error instanceof Error ? error.message : String(error))}`, "warning");
    } finally { this.readingTranscript = false; this.options.changed(); }
  }
  private renderActivity(width: number): string[] {
    this.jobHits.clear(); const rows: string[] = [], snapshot = this.options.snapshot();
    this.jobId ??= snapshot?.jobs?.[0]?.job;
    const selected = snapshot?.jobs?.findIndex(job => job.job === this.jobId) ?? -1;
    for (const row of activityRows(snapshot, selected)) {
      const line = row.text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
      const wrapped = new Text(this.ui ? this.ui.theme.fg(row.color ?? "muted", line) : line, 0, 0).render(width);
      const id = row.job === undefined ? undefined : snapshot?.jobs?.[row.job]?.job;
      if (id) { for (let i = 0; i < wrapped.length; i++) this.jobHits.set(rows.length + i, id); }
      rows.push(...wrapped);
    }
    return rows;
  }
  private acceptPatch(): void {
    const snapshot = this.review.patchSnapshot;
    if (!snapshot || snapshot.root !== this.review.root || snapshot.path !== this.review.selected || snapshot.comparison !== this.review.comparison) return;
    this.lastPatch = snapshot.text; this.displayedPath = snapshot.path; this.displayedRoot = snapshot.root;
    this.displayedMetadata = snapshot;
    this.displayedAt = snapshot.at; this.displayedHead = snapshot.head; this.displayedBase = snapshot.base;
    this.displayedComparison = comparisonLabel(snapshot.comparison); this.hunk = 0;
    this.options.changed();
  }
  private renderChanges(width: number): string[] {
    const r = this.review;
    const label = comparisonLabel(r.comparison);
    const sameScope = this.displayedRoot === r.root && this.displayedPath === r.selected && this.displayedComparison === label;
    const base = sameScope ? this.displayedHead : r.head;
    const state = r.error ? "STALE / unavailable: " + plainLabel(r.error) : r.refreshing ? "Checking Git…" : r.checkedAt ? "Updated " + new Date(r.checkedAt).toLocaleTimeString() : "Not loaded";
    if (r.scopeRequired) return [PANEL_TEXT + "Choose a project", "", PANEL_MUTED + "Workspace-wide changes are not shown.",
      PANEL_MUTED + "Press o to choose a project or your scratch worktree.", PANEL_MUTED + "This is a read-only view; your session stays bound to Corral."]
      .flatMap(line => new Text(line, 0, 0).render(width));
    const head = [PANEL_BLUE + `${plainLabel(this.scopeLabel)} (${plainLabel(r.branch)})` + PANEL_TEXT,
      ...(width >= 90 ? [PANEL_MUTED + plainLabel(r.root)] : []), PANEL_MUTED + label + (width >= 90 && r.comparison !== "candidate" ? ` · base ${base?.slice(0, 10) ?? "unborn/unavailable"}` : ""),
      ...(r.comparison === "candidate" ? [PANEL_MUTED + `Comparison base ${sameScope ? this.displayedBase ?? "unavailable" : r.candidateBase ?? "unavailable"}`,
        PANEL_MUTED + `Candidate HEAD ${base ?? r.head ?? "unavailable"} · committed only; uncommitted edits excluded`] : []),
      PANEL_MUTED + state, PANEL_MUTED + (width >= 90 ? "/ search · c compare · o repository · j/k files · n/p hunks · a ask hunk · r refresh" : "j/k files · n/p hunks · a ask · / filter"), ""];
    if (r.limitNotice) head.push(PANEL_MUTED + r.limitNotice);
    const file = r.current;
    const selected = sameScope ? this.displayedMetadata?.file : file;
    const stat = sameScope ? this.displayedMetadata?.stat : file ? r.stats.get(file.path) : undefined;
    head.push(PANEL_BLUE + (selected ? `${plainLabel(selected.path)} · ${fileStatus(selected)}` : "No selected file"),
      ...(selected ? [PANEL_MUTED + (stat ? stat.added === null ? "Diffstat: binary" : `Diffstat: +${stat.added} −${stat.removed}` : "Diffstat: unavailable")] : []), "");
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
    if (r.scopeRequired) return [{ text: "Choose project or scratch", heading: true, action: { kind: "scope" } },
      { text: "No project selected", tone: "muted" }, { text: "" },
      { text: "Workspace changes are hidden." }, { text: "o choose project", tone: "accent", action: { kind: "scope" } }];
    const rows: PanelLine[] = [{ text: this.scopeLabel, heading: true, action: { kind: "scope" } },
      { text: r.branch, tone: "muted" },
      { text: "o project / worktree", tone: "accent", action: { kind: "scope" } },
      ...(r.comparison === "candidate" ? [{ text: `Base ${r.candidateBase ?? "unavailable"}` }, { text: `HEAD ${r.head ?? "unavailable"}` }] : []),
      { text: "" },
      { text: `${r.checkedAt ? r.visible.length + " matching" : "Loading / unavailable"} · ${r.comparison}${r.query ? " · /" + r.query : ""}`, tone: "muted" }];
    if (r.error) rows.push({ text: "STALE: " + plainLabel(r.error) });
    for (const file of r.visible) {
      const stat = r.stats.get(file.path);
      const delta = stat ? stat.added === null ? " · binary" : ` · +${stat.added} −${stat.removed}` : "";
      const selected = file.path === r.selected;
      const action = { kind: "diff" as const, path: file.path };
      rows.push({ text: "" },
        { text: `${selected ? "›" : "·"} ${basename(file.path)}`, continuationIndent: 2, tone: selected ? "accent" : "text", action },
        { text: `${fileStatus(file)}${delta}`, indent: 2, tone: fileStatus(file) === "Conflict" ? "warning" : "muted", action });
      if (dirname(file.path) !== ".") rows.push({ text: dirname(file.path) + "/", indent: 2, tone: "muted", action });
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
  async repository(): Promise<void> {
    const ui = this.ui;
    if (!ui || this.choosingScope || this.scopeAbort.signal.aborted) return;
    this.choosingScope = true;
    try {
      let path: string | undefined;
      let name: string | undefined;
      if (this.options.readProjects) {
        const listed = projectScopes(await this.options.readProjects(this.scopeAbort.signal));
        const scopes = this.options.identity.task ? [{ label: `${this.options.identity.project} / bound worktree`, path: this.options.cwd }, ...listed] : listed;
        if (this.scopeAbort.signal.aborted) return;
        if (!scopes.length) { ui.notify("No configured projects. Add a project in Corral settings.", "warning"); return; }
        const scope = await chooseProject(scopes, (title, choices) => ui.select(title, choices));
        path = scope?.path; name = scope?.label;
      } else path = await ui.input("Read-only repository scope (does not rebind this session)", this.review.root);
      if (path && !this.scopeAbort.signal.aborted) {
        this.lastPatch = ""; this.displayedPath = undefined; this.projectName = name;
        await this.review.setRepository(path); this.changesScroll?.scrollToStart(); this.options.changed();
      }
    } catch (error) {
      if (!this.scopeAbort.signal.aborted) ui.notify(`Projects unavailable: ${plainLabel(error instanceof Error ? error.message : String(error))}`, "error");
    } finally { this.choosingScope = false; }
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
    if (matchesKey(data, "f11")) { this.setView("inbox"); return true; }
    if (this.images.isOpen && matchesKey(data, "escape")) { this.images.close(); return true; }
    if (!this.focused || this.view === "conversation") return false;
    if (matchesKey(data, "escape")) { this.setView("conversation"); this.focused = false; return true; }
    if (this.view === "inbox" && this.inboxView.handleInput(data)) return true;
    if (matchesKey(data, "enter") || matchesKey(data, "tab")) { this.focused = false; this.options.changed(); return true; }
    if (this.view === "activity" && data === "a") { this.setView("agents"); return true; }
    if (this.view === "agents" && data === "b") { this.setView("activity"); return true; }
    if (this.view === "agents" && data === "t") { void this.openAgentTranscript(); return true; }
    const scroll = this.view === "agents" ? this.agentsScroll : this.view === "changes" ? this.changesScroll : this.view === "evidence" ? this.evidenceScroll : this.view === "inbox" ? this.inboxScroll : this.activityScroll;
    if (this.view === "inbox" && (data === "j" || data === "k" || matchesKey(data, "up") || matchesKey(data, "down"))) {
      this.inbox.move(data === "j" || matchesKey(data, "down") ? 1 : -1);
      const row = this.inbox.current && this.inboxView.rows.get(inboxIdentity(this.inbox.current));
      if (row !== undefined) scroll?.scrollTo(row, { disableFollow: true });
      this.options.changed(); return true;
    }
    if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      scroll?.scrollBy(matchesKey(data, "up") ? -3 : matchesKey(data, "down") ? 3 : matchesKey(data, "pageUp") ? -12 : 12); this.options.changed(); return true;
    }
    if (data === "j" || data === "k") {
      if (this.view === "changes") void this.selectRelative(data === "j" ? 1 : -1);
      else if (this.view === "evidence") this.criterionIndex = Math.max(0, Math.min((this.inspection.current?.criteria.length ?? 1) - 1, this.criterionIndex + (data === "j" ? 1 : -1)));
      else if (this.view === "agents") {
        const agents = this.options.snapshot()?.agents ?? [], index = agents.findIndex(agent => agent.id === this.agentId);
        this.agentId = agents[Math.max(0, Math.min(agents.length - 1, index + (data === "j" ? 1 : -1)))]?.id;
      }
      else {
        const jobs = this.options.snapshot()?.jobs ?? [], index = jobs.findIndex(job => job.job === this.jobId);
        this.jobId = jobs[Math.max(0, Math.min(jobs.length - 1, index + (data === "j" ? 1 : -1)))]?.job;
      }
      this.options.changed(); return true;
    }
    if (this.view === "evidence" && data === "r") { void this.inspection.refresh(true); return true; }
    if (this.view === "changes") {
      if (data === "/") { void this.search(); return true; }
      if (data === "o") { void this.repository(); return true; }
      if (data === "c") { const choices = this.review.comparisons; const next = choices[(choices.indexOf(this.review.comparison) + 1) % choices.length]; if (next) { this.lastPatch = ""; void this.review.setComparison(next); } return true; }
      if (data === "r") { this.inspection.invalidate(); void this.review.refresh().then(() => this.acceptPatch()); return true; }
      if (data === "n" || data === "p") { this.moveHunk(data === "n" ? 1 : -1); return true; }
      if (data === "a") { this.askHunk(); return true; }
    }
    if (data.startsWith("\x1b")) return false; // Terminal replies/focus events are not typing.
    this.focused = false; return false;
  }
  dispose(): void {
    if (this.timer) clearInterval(this.timer); if (this.debounce) clearTimeout(this.debounce);
    this.scopeAbort.abort(); this.images.dispose(); this.review.dispose(); this.inspection.dispose(); this.inboxView.dispose(); this.ui = undefined;
  }
}
