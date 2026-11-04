import { createHash } from "node:crypto";
import { basename, dirname } from "node:path";
import { chooseProject, projectScopes } from "./project-scopes.ts";
import type { HostIdentity } from "../roles/types.ts";
import { target } from "./chrome.ts";
import { getMarkdownTheme, type ExtensionUIContext, type InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { VStack, ScrollView, Text, getKeybindings, matchesKey, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { anchoredRow } from "./reading-anchor.ts";
import { NativeImageViewer } from "./image-viewer.ts";
import { ReviewStore, comparisonLabel, fileStatus, type PatchSnapshot } from "./review-store.ts";
import { ChangesView, changesModel, type ChangesModel } from "./changes-view.ts";
import { LifecycleView } from "./lifecycle-view.ts";
import { ArtifactView } from "./artifact-view.ts";
import type { ActivityMode } from "./activity-view.ts";
import { agentRows } from "./agent-view.ts";
import { activityKey, type ActivityTarget } from "./activity-sidebar.ts";
import { activityEntries, activityHints, activityScope, resolveSelection, LIST_WIDTH, type ActivityEntry } from "./activity-list.ts";
import { FittedScrollView, openOutputPager, pagerLines, renderActivityView, type JobFreeze } from "./activity-detail.ts";
import { InspectionStore, evidenceLines } from "./inspection.ts";
import { proofHint, renderProof, SPLIT_MIN } from "./proof-view.ts";
import { proofItems, type SectionId } from "./proof-parts.ts";
import { usageLines } from "./usage-view.ts";
import { TaskStore, taskHeadline, taskLines } from "./task-view.ts";
import { splitRow } from "./panels-chrome.ts";
import { DecisionInbox, inboxIdentity } from "./decision-inbox.ts";
import { InboxView } from "./inbox-view.ts";
import { HistoryNavigator, type HistoryRead } from "./history.ts";
import { pad, plainLabel, styledPanelLine, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, PANEL_BG, PANEL_WARNING, RESET, type PanelSnapshot, type PanelLine } from "./panels-view.ts";
export function plainClick(event: TuiMouseEvent): boolean {
  return event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && (event.clickCount ?? 1) === 1;
}
export type MainView = "task" | "conversation" | "changes" | "activity" | "agents" | "evidence" | "inbox" | "graph" | "timeline" | "usage" | "artifacts";
/** Views reached through Activity; its sub-tab row shows only while one of them is open. */
export const ACTIVITY_VIEWS: ReadonlySet<MainView> = new Set<MainView>(["activity", "agents", "graph", "timeline"]);
export interface WorkbenchOptions {
  readonly identity: HostIdentity;
  readonly inspection?: () => Promise<unknown>;
  readonly lifecycle?: () => Promise<unknown>;
  readonly artifacts?: () => Promise<unknown>;
  readonly artifactViewer?: (reference: string, viewer: "show" | "serve") => Promise<string>;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly readTask?: () => Promise<unknown>;
  /** True while the side panel is visible; it is the navigation then, so the tabs row hides. */
  readonly railVisible?: () => boolean;
  readonly cwd: string; readonly snapshot: () => PanelSnapshot | null;
  readonly changed: () => void; readonly canInteract: () => boolean;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
}
/** Main-area composition via the packaged SDK's public mainView hook; editor stays SDK-owned. */
export class NativeWorkbench {
  view: MainView = "conversation"; focused = false;
  readonly review: ReviewStore;
  readonly inspection: InspectionStore;
  readonly task: TaskStore;
  private taskScroll: ScrollView | undefined;
  readonly lifecycle: LifecycleView;
  readonly history: HistoryNavigator;
  private lifecycleScroll: ScrollView | undefined;
  private usageScroll: ScrollView | undefined; private usageChart = false;
  readonly artifacts: ArtifactView; private artifactScroll: ScrollView | undefined;
  private evidenceScroll: ScrollView | undefined; private criterionIndex = 0; private criterionHits = new Map<number, number>();
  readonly images: NativeImageViewer;
  readonly inbox: DecisionInbox; private readonly inboxView: InboxView;
  private inboxScroll: ScrollView | undefined;
  private ui: ExtensionUIContext | undefined;
  private changes: ChangesView | undefined;
  private changesScroll: ScrollView | undefined; private activityScroll: ScrollView | undefined;
  private agentsScroll: ScrollView | undefined; private agentId: string | undefined;
  private agentHits = new Map<number, string>(); private readingTranscript = false;
  private transcript: ScrollView | undefined; private transcriptWidth = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private debounce: ReturnType<typeof setTimeout> | undefined;
  private jobId: string | undefined;
  private activityMode: ActivityMode = "jobs"; private helperRequest: string | undefined;
  private readonly activityPositions = new Map<string, number>();
  private activityRestore: number | undefined;
  private chatPresentation: (() => InteractiveChatPresentation) | undefined;
  private activityHits: ReadonlyMap<number, ActivityTarget> = new Map(); private jobFreeze: JobFreeze | undefined;
  private toolsExpanded = false;
  private legend = false; private readonly proofOpen = new Set<SectionId>(); private proofWide = false;
  private hunk = 0;
  private lastPatch = ""; private displayedPath: string | undefined;
  private displayedRoot: string | undefined;
  private displayedBase: string | undefined;
  private displayedFile: PatchSnapshot["file"] | undefined;
  private displayedAt: number | undefined; private displayedHead: string | undefined; private displayedComparison = "";
  private projectName: string | undefined;
  private choosingScope = false;
  private readonly scopeAbort = new AbortController();
  get scopeLabel(): string { return this.review.scopeRequired ? "Choose project" : this.projectName ?? this.review.name; }
  constructor(private readonly options: WorkbenchOptions) {
    this.history = new HistoryNavigator(options.readHistory, options.canInteract);
    this.images = new NativeImageViewer(() => options.canInteract(), () => { this.focused = this.images.isOpen || this.view !== "conversation"; options.changed(); });
    this.lifecycle = new LifecycleView(options.identity, options.lifecycle, () => options.snapshot()?.agents, () => options.snapshot()?.bound ?? false, options.changed, options.canInteract, () => options.snapshot()?.inspector);
    this.inspection = new InspectionStore(options.identity, options.inspection, () => options.changed());
    this.artifacts = new ArtifactView({ identity: options.identity, load: options.artifacts, viewer: options.artifactViewer, exportSession: options.exportSession,
      openImage: source => this.images.activate(source), bound: () => options.snapshot()?.bound ?? false, changed: options.changed, canInteract: options.canInteract });
    this.task = new TaskStore(options.identity, options.readTask, () => options.changed());
    // Task-bound roles open on their task; assistant and PM open on Chat. Typing still reaches the editor.
    if (this.task.bound) this.view = "task";
    this.projectName = options.identity.project || undefined;
    this.review = new ReviewStore(options.projectDir ?? options.cwd, () => options.changed(), options.inspection && options.identity.task && options.identity.run ? async () => {
      await this.inspection.refresh();
      if (this.inspection.stale || !this.inspection.current) throw new Error(this.inspection.error ?? "Canonical candidate unavailable; r to reconcile");
      return this.inspection.current;
    } : undefined, !!options.readProjects && !options.identity.task && !options.projectDir, !!options.projectDir);
    this.inbox = new DecisionInbox(options.readInbox, options.changed);
    this.inboxView = new InboxView(this.inbox, options.identity.role, options.identity.project, options.canInteract, options.changed, () => { this.focused = true; });
  }
  async openHistory(): Promise<void> { if (this.ui) await this.history.open(this.ui); }
  bind(ui: ExtensionUIContext): void { this.ui = ui; this.images.bind(); this.inboxView.bind(ui); void this.task.refresh(); }
  invalidateFiles(): void {
    this.inspection.invalidate();
    if (this.debounce) clearTimeout(this.debounce);
    this.debounce = setTimeout(() => { this.debounce = undefined; void this.review.refresh(); }, 300);
    this.debounce.unref();
  }
  setView(view: MainView): void {
    if (this.view === "activity" && view !== "activity") this.rememberActivity();
    this.images.close();
    this.view = view; this.focused = view !== "conversation";
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    if (view !== "conversation") {
      if (view === "changes") void this.review.refresh();
      if (view === "task") void this.task.refresh(true);
      if (view === "evidence" || view === "usage") void this.inspection.refresh();
      if (view === "inbox") void this.inbox.refresh();
      if (view === "graph" || view === "timeline") { this.lifecycle.mode = view; void this.lifecycle.refresh(); }
      if (view === "artifacts") void this.artifacts.refresh();
      let ticks = 0;
      this.timer = setInterval(() => {
        ticks++;
        if (this.view === "changes" && ticks % 2 === 0) void this.review.refresh();
        if ((this.view === "evidence" || this.view === "usage") && ticks % 10 === 0) void this.inspection.refresh();
        if (this.view === "inbox" && ticks % 5 === 0) void this.inbox.refresh();
        if (this.view === "task" && ticks % 15 === 0) void this.task.refresh();
        this.options.changed();
      }, 1000); this.timer.unref();
    }
    this.options.changed();
  }
  get selectedActivity(): ActivityTarget | undefined {
    return this.activityMode === "helpers" ? this.helperRequest === undefined ? undefined : { kind: "helper", request: this.helperRequest }
      : this.jobId === undefined ? undefined : { kind: "job", job: this.jobId };
  }
  private rememberActivity(): void {
    const selected = this.selectedActivity;
    if (selected && this.activityScroll) this.activityPositions.set(activityKey(selected), this.activityScroll.scrollTop);
  }
  openHelpers(request?: string): void {
    if (this.view === "activity") this.rememberActivity();
    this.activityMode = "helpers";
    this.helperRequest = request ?? this.helperRequest ?? this.options.snapshot()?.helpers?.[0]?.request;
    this.openActivity();
  }
  openJob(job: string): void {
    if (this.view === "activity") this.rememberActivity();
    this.activityMode = "jobs"; this.jobId = job; this.openActivity();
  }
  private shownActivity(): ActivityEntry | undefined {
    return resolveSelection(activityEntries(this.options.snapshot(), Date.now()), this.selectedActivity).entry;
  }
  private selectActivity(target: ActivityTarget): void {
    if (target.kind === "job") this.openJob(target.job); else this.openHelpers(target.request);
  }
  private moveActivity(delta: number): void {
    const entries = activityEntries(this.options.snapshot(), Date.now()), shown = this.shownActivity();
    const index = shown ? entries.findIndex(entry => entry.key === shown.key) + delta : 0;
    const next = entries[Math.max(0, Math.min(entries.length - 1, index))];
    if (next) this.selectActivity(next.target);
  }
  private async openJobOutput(id: string, search: boolean): Promise<void> {
    const ui = this.ui, snapshot = this.options.snapshot(), job = snapshot?.jobs?.find(item => item.job === id);
    if (!ui || !this.options.canInteract()) return;
    if (!job?.detail || !snapshot?.bound) { ui.notify("Job output unavailable for this job source or host", "warning"); return; }
    const lines = pagerLines(job);
    let start = lines.length;
    if (search) {
      const query = await ui.input("Search retained job output");
      if (!query) return;
      start = lines.findIndex(line => line.includes(query));
      if (start < 0) { ui.notify(`No match for ${JSON.stringify(plainLabel(query))} in retained output`, "info"); return; }
    }
    await openOutputPager(ui, `Job ${id} · retained output · ${lines.length} lines`, lines, start);
  }
  private openActivity(): void {
    const selected = this.selectedActivity;
    this.activityRestore = selected ? this.activityPositions.get(activityKey(selected)) ?? 0 : 0;
    this.setView("activity");
  }
  async openFile(path: string): Promise<void> {
    this.setView("changes"); await this.review.select(path); this.acceptPatch();
  }
  private nav(): { readonly tabs: Component; readonly head: Component } {
    type Target = MainView | "history";
    let hits: { readonly to: number; readonly view: Target }[] = [];
    let subHits: { readonly to: number; readonly view: Target }[] = [];
    const strip = (labels: readonly (readonly [Target, string])[], active: (view: Target) => boolean) => {
      let x = 0;
      const placed = labels.map(([view, label]) => ({ view, to: x += visibleWidth(label) }));
      return { placed, text: labels.map(([view, label]) => (active(view) ? PANEL_BLUE + "\x1b[1m" : PANEL_MUTED) + label + "\x1b[22m").join("") };
    };
    let compact = false;
    const tabs: Component = {
      render: width => {
        compact = width < 110;
        const minimum = width < 64;
        const label = (short: string, mid: string, full: string) => minimum ? ` ${short} ` : compact ? ` ${mid} ` : ` ${full} `;
        const labels: (readonly [Target, string])[] = [
          ...(this.task.bound ? [["task", label("T", "Task", "Task F5")] as const] : []),
          ["conversation", label("C", "Chat", "Chat F9")], ["changes", label("D", "Changes", "Changes F7")],
          ["activity", label("A", "Activity", "Activity F8")], ["evidence", label("P", "Proof", "Proof F10")],
          ["inbox", label("I", "Inbox", "Inbox F11")], ["usage", label("U", "Usage", "Usage")]];
        // Artifacts opens with `a` from Proof, so Proof stays highlighted there.
        const main = strip(labels, view => view === this.view || view === "activity" && ACTIVITY_VIEWS.has(this.view)
          || view === "evidence" && this.view === "artifacts");
        hits = main.placed;
        return [PANEL_BG + pad(main.text + PANEL_MUTED + (this.focused ? compact ? " Esc Enter" : " · Esc chat · Enter compose" : ""), width) + RESET];
      },
      handleMouse: event => {
        if (!this.options.canInteract() || !plainClick(event) || event.y !== 0) return undefined;
        const hit = hits.find(hit => event.x < hit.to);
        if (!hit) return undefined;
        if (hit.view === "history") void this.openHistory(); else this.setView(hit.view);
        return { handled: true };
      }, invalidate() {},
    };
    const head: Component = {
      render: width => {
        // Renders before the transcript, so a paused reader keeps the same passage at a new width.
        this.anchorTranscript(width);
        const rows: string[] = [];
        if (ACTIVITY_VIEWS.has(this.view)) {
          const sub = strip([["activity", " Now "], ["graph", " Graph "], ["timeline", " Timeline "], ["history", " History "]],
            view => view === this.view || view === "activity" && this.view === "agents");
          subHits = sub.placed;
          const scope = this.view === "activity" ? activityScope(this.options.snapshot(), this.shownActivity()?.target.kind) : "";
          const fits = scope && visibleWidth(sub.text) + 2 + visibleWidth(scope) <= width;
          rows.push(PANEL_BG + (fits ? splitRow(sub.text, scope, width) : pad(sub.text, width)) + RESET);
        } else {
          subHits = [];
          rows.push(PANEL_BG + PANEL_TEXT + (this.headline(width) || pad(`Files: ${plainLabel(this.scopeLabel)} · ` + PANEL_MUTED
            + target(this.options.identity) + (this.ui ? "" : " · Initializing editor…"), width)) + RESET);
        }
        return rows;
      },
      handleMouse: event => {
        if (!this.options.canInteract() || !plainClick(event) || event.y !== 0) return undefined;
        const hit = subHits.find(hit => event.x < hit.to);
        if (!hit) return undefined;
        if (hit.view === "history") void this.openHistory(); else this.setView(hit.view);
        return { handled: true };
      }, invalidate() {},
    };
    return { tabs, head };
  }
  /** `T-796 · title … ● running P1 xs`; undefined when no task is bound. */
  private headline(width: number): string | undefined {
    const head = taskHeadline(this.options.identity, this.task);
    if (!head) return undefined;
    const chips = head.chips.map(chip => (chip === "stale" ? PANEL_WARNING : PANEL_MUTED) + chip).join(PANEL_MUTED + " · ");
    return splitRow("\x1b[1m" + PANEL_BLUE + plainLabel(head.id) + "\x1b[22m" + PANEL_TEXT + " · " + head.title, chips, width);
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
  mount(transcript: Component, presentation?: () => InteractiveChatPresentation): Component {
    this.chatPresentation = presentation;
    if (transcript instanceof ScrollView) this.transcript = transcript;
    this.changes = new ChangesView({ model: () => this.changesModel(), click: event => this.options.canInteract() && plainClick(event),
      focus: () => { this.focused = true; this.options.changed(); }, step: delta => void this.selectRelative(delta),
      select: path => void this.review.select(path).then(() => { this.acceptPatch(); this.changesScroll?.scrollToStart(); }),
      pickHunk: index => { this.hunk = index; this.options.changed(); } });
    this.changesScroll = this.changes.diff;
    this.activityScroll = new FittedScrollView({ render: width => this.renderActivity(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      this.focused = true;
      const target = event.x < LIST_WIDTH ? this.activityHits.get(event.y) : undefined;
      if (target) this.selectActivity(target);
      this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    const activityHintRow: Component = { render: width => [activityHints(this.shownActivity()?.target.kind, width)], invalidate() {} };
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
    this.lifecycleScroll = new ScrollView({ render: width => this.lifecycle.render(width), handleMouse: event => {
      const result = this.lifecycle.handleMouse(event);
      if (result?.handled) { this.focused = true; this.options.changed(); }
      return result;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.usageScroll = new ScrollView({ render: width => {
      const snapshot = this.options.snapshot();
      return usageLines(this.inspection, snapshot?.budget, snapshot?.bound ?? false, width, this.usageChart)
        .flatMap(row => new Text((row.heading ? PANEL_BLUE : PANEL_TEXT) + row.text, 0, 0).render(width));
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.artifactScroll = new ScrollView({ render: width => this.artifacts.render(width), handleMouse: event => {
      const result = this.artifacts.handleMouse(event);
      if (result?.handled) { this.focused = true; this.options.changed(); }
      return result;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.inboxScroll = new ScrollView(this.inboxView, { primary: true, follow: "none", scrollbar: "auto" });
    this.taskScroll = new ScrollView({ render: width => taskLines(this.options.identity, this.task)
      .flatMap(row => new Text(styledPanelLine(row), 0, 0).render(width)), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      this.focused = true; this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    const nav = this.nav(), rail = () => this.options.railVisible?.() ?? false;
    return new VStack([
      { component: nav.tabs, basis: 1, shrink: 0, visible: () => !rail() },
      { component: nav.head, basis: 1, shrink: 0 },
      { component: this.taskScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "task" && !this.images.isOpen },
      { component: transcript, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "conversation" && !this.images.isOpen },
      { component: this.images, basis: 0, grow: 1, minSize: 1, visible: viewport => { this.images.setViewportHeight(Math.max(0, viewport.height - (rail() ? 1 : 2))); return this.images.isOpen; } },
      { component: this.changes.root, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "changes" },
      { component: this.activityScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "activity" },
      { component: activityHintRow, basis: 1, shrink: 0, visible: () => this.view === "activity" },
      { component: this.agentsScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agents" },
      { component: this.evidenceScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "evidence" },
      { component: { render: width => [proofHint(width, this.legend)], invalidate() {} }, basis: 1, shrink: 0,
        visible: viewport => { this.proofWide = viewport.width >= SPLIT_MIN; return this.view === "evidence"; } },
      { component: this.usageScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "usage" },
      { component: this.lifecycleScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "graph" || this.view === "timeline" },
      { component: this.artifactScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "artifacts" && !this.images.isOpen },
      { component: this.inboxScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "inbox" },
    ]);
  }
  private renderEvidence(width: number): string[] {
    this.criterionIndex = Math.max(0, Math.min(this.criterionIndex, this.proofRows() - 1));
    const frame = renderProof(this.inspection, this.criterionIndex, width, this.legend, this.proofOpen, Date.now(), this.proofWide);
    this.criterionHits = frame.hits;
    return frame.lines;
  }
  /** Selectable Proof rows: criteria, then the MORE sections (list rows when wide, picker steps when narrow). */
  private proofRows(): number {
    return Math.max(1, proofItems(this.inspection.current).length);
  }
  private proofEnter(): void {
    const item = proofItems(this.inspection.current)[this.criterionIndex];
    if (item?.kind !== "section") {
      const path = item ? this.inspection.current?.criteria[item.index]?.observation?.artifacts[0] : undefined;
      this.setView("artifacts");
      if (path !== undefined) void this.artifacts.openPath(path);
      return;
    }
    if (!this.proofOpen.delete(item.id)) this.proofOpen.add(item.id);
    this.options.changed();
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
    const view = renderActivityView({ snapshot: this.options.snapshot(), identity: this.options.identity, selected: this.selectedActivity,
      width, height: this.activityScroll?.viewportHeight || 24, now: Date.now(), legend: this.legend, expanded: this.toolsExpanded,
      freeze: this.jobFreeze, markdownTheme: this.chatPresentation?.().markdownTheme ?? getMarkdownTheme() });
    this.activityHits = view.hits;
    const rows = view.lines;
    if (this.activityRestore !== undefined && this.activityScroll) {
      this.activityScroll.updateLayout(rows.length, this.activityScroll.viewportHeight, () => {});
      this.activityScroll.scrollTo(this.activityRestore, { disableFollow: true });
      this.activityRestore = undefined;
    }
    return rows;
  }
  private acceptPatch(): void {
    const snapshot = this.review.patchSnapshot;
    if (!snapshot || snapshot.root !== this.review.root || snapshot.path !== this.review.selected || snapshot.comparison !== this.review.comparison) return;
    this.lastPatch = snapshot.text; this.displayedPath = snapshot.path; this.displayedRoot = snapshot.root; this.displayedFile = snapshot.file;
    this.displayedAt = snapshot.at; this.displayedHead = snapshot.head; this.displayedBase = snapshot.base;
    this.displayedComparison = comparisonLabel(snapshot.comparison); this.hunk = 0;
    this.options.changed();
  }
  private changesModel(): ChangesModel {
    const r = this.review;
    if (r.current && r.patchPath === r.selected && r.patchComparison === r.comparison && (this.displayedPath !== r.selected || this.displayedRoot !== r.root
      || this.displayedComparison !== comparisonLabel(r.comparison) || !this.lastPatch)) this.acceptPatch();
    return changesModel(r, { root: this.displayedRoot, path: this.displayedPath, comparison: this.displayedComparison, text: this.lastPatch,
      head: this.displayedHead, base: this.displayedBase, file: this.displayedFile }, this.scopeLabel, this.hunk, Date.now());
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
    this.hunk = Math.max(0, Math.min((this.changes?.hunkRows.length ?? 0) - 1, this.hunk + delta));
    const row = this.changes?.hunkRows[this.hunk]; if (row !== undefined) this.changesScroll?.scrollTo(row, { disableFollow: true });
    this.options.changed();
  }
  askHunk(): void {
    const ui = this.ui, file = this.review.current, sources = this.changes?.hunkSources ?? [], start = sources[this.hunk];
    if (!ui || !file) return;
    if (this.displayedPath !== file.path || this.displayedRoot !== this.review.root || this.review.patchComparison !== this.review.comparison || this.displayedComparison !== comparisonLabel(this.review.comparison)) {
      ui.notify("Wait for the selected comparison to load before attaching its hunk.", "warning"); return;
    }
    const source = this.lastPatch.split("\n");
    const end = sources[this.hunk + 1] ?? source.length;
    const excerpt = start === undefined ? source.slice(0, 18) : source.slice(start, Math.min(end, start + 24));
    const reference = `Please review ${JSON.stringify(file.path)} in ${this.review.root}\nComparison: ${this.displayedComparison} · base ${this.displayedBase ?? "see comparison"} · HEAD ${this.displayedHead ?? "unborn/unavailable"}\nDiff SHA256: ${createHash("sha256").update(this.lastPatch).digest("hex")}\nSelected Git excerpt (observed ${this.displayedAt ? new Date(this.displayedAt).toISOString() : "time unavailable"}):\n` + excerpt.map(line => "> " + line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")).join("\n").slice(0, 3000);
    const draft = ui.getEditorText(); ui.setEditorText(draft + (draft ? "\n\n" : "") + reference + "\n");
    this.focused = false; this.options.changed(); // Never submit or overwrite the existing draft.
  }
  /** Activity › Now keys: j/k walk the shared list; f, Enter and / act on a job; Enter and t on a helper. */
  private activityInput(data: string): boolean {
    if (data === "j" || data === "k") { this.moveActivity(data === "j" ? 1 : -1); return true; }
    const target = this.shownActivity()?.target;
    if (target?.kind === "job") {
      if (data === "f") {
        const job = this.options.snapshot()?.jobs?.find(item => item.job === target.job);
        this.jobFreeze = this.jobFreeze?.job === target.job || !job?.detail ? undefined
          : { job: target.job, tail: job.detail.tail, lines: job.detail.lines, running: job.state === "running" };
        this.options.changed(); return true;
      }
      if (matchesKey(data, "enter") || data === "/") { void this.openJobOutput(target.job, data === "/"); return true; }
    } else if (target?.kind === "helper") {
      if (matchesKey(data, "enter")) { this.toolsExpanded = !this.toolsExpanded; this.options.changed(); return true; }
      if (data === "t") { this.agentId = target.request; void this.openAgentTranscript(); return true; }
    }
    return false;
  }
  handleInput(data: string): boolean {
    if (matchesKey(data, "f9")) { this.setView("conversation"); this.focused = false; return true; }
    if (matchesKey(data, "f5") && this.task.bound) { this.setView("task"); return true; }
    if (matchesKey(data, "f7")) { this.setView("changes"); return true; }
    if (matchesKey(data, "f8")) { this.setView("activity"); return true; }
    if (matchesKey(data, "f10")) { this.setView("evidence"); return true; }
    if (matchesKey(data, "f11")) { this.setView("inbox"); return true; }
    if (this.images.isOpen && matchesKey(data, "escape")) { this.images.close(); return true; }
    if (!this.focused || this.view === "conversation") return false;
    if (matchesKey(data, "escape")) { this.setView("conversation"); this.focused = false; return true; }
    if (this.view === "activity" && (data === "g" || data === "l")) { this.setView(data === "g" ? "graph" : "timeline"); return true; }
    if ((this.view === "graph" || this.view === "timeline") && this.lifecycle.handleInput(data)) { this.lifecycleScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (this.view === "inbox" && this.inboxView.handleInput(data)) return true;
    if (this.view === "activity" && this.activityMode === "helpers" && getKeybindings().matches(data, "app.tools.expand")) {
      this.toolsExpanded = !this.toolsExpanded; this.options.changed(); return true;
    }
    if (this.view === "activity" && this.activityInput(data)) return true;
    if (this.view === "evidence" && data === "a") { this.setView("artifacts"); return true; }
    if (this.view === "evidence" && matchesKey(data, "enter")) { this.proofEnter(); return true; }
    if (this.view === "artifacts" && this.artifacts.handleInput(data)) { this.artifactScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (matchesKey(data, "enter") || matchesKey(data, "tab")) { this.focused = false; this.options.changed(); return true; }
    if (this.view === "activity" && data === "a") { this.setView("agents"); return true; }
    if (this.view === "agents" && data === "b") { this.setView("activity"); return true; }
    if (this.view === "agents" && data === "t") { void this.openAgentTranscript(); return true; }
    if ((this.view === "evidence" || this.view === "activity") && data === "?") { this.legend = !this.legend; this.options.changed(); return true; }
    if (this.view === "usage" && data === "c") { this.usageChart = !this.usageChart; this.options.changed(); return true; }
    const scroll = this.view === "task" ? this.taskScroll : this.view === "usage" ? this.usageScroll : this.view === "artifacts" ? this.artifactScroll : this.view === "graph" || this.view === "timeline" ? this.lifecycleScroll : this.view === "agents" ? this.agentsScroll : this.view === "changes" ? this.changesScroll : this.view === "evidence" ? this.evidenceScroll : this.view === "inbox" ? this.inboxScroll : this.activityScroll;
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
      else if (this.view === "evidence") this.criterionIndex = Math.max(0, Math.min(this.proofRows() - 1, this.criterionIndex + (data === "j" ? 1 : -1)));
      else if (this.view === "agents") {
        const agents = this.options.snapshot()?.agents ?? [], index = agents.findIndex(agent => agent.id === this.agentId);
        this.agentId = agents[Math.max(0, Math.min(agents.length - 1, index + (data === "j" ? 1 : -1)))]?.id;
      }
      this.options.changed(); return true;
    }
    if ((this.view === "evidence" || this.view === "usage") && data === "r") { void this.inspection.refresh(true); return true; }
    if (this.view === "task" && data === "r") { void this.task.refresh(true); return true; }
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
    this.chatPresentation = undefined;
    this.history.dispose(); this.scopeAbort.abort(); this.images.dispose(); this.review.dispose(); this.inspection.dispose(); this.task.dispose(); this.inboxView.dispose(); this.lifecycle.dispose(); this.artifacts.dispose(); this.ui = undefined;
  }
}
