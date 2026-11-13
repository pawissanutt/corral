import { createHash } from "node:crypto";
import { basename } from "node:path";
import { chooseProject, projectScopes } from "./project-scopes.ts";
import type { HostIdentity } from "../roles/types.ts";
import type { JobControl } from "../host/job-controls.ts";
import { target } from "./chrome.ts";
import type { ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { VStack, ScrollView, Text, getKeybindings, matchesKey, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { anchoredRow } from "./reading-anchor.ts";
import { NativeImageViewer } from "./image-viewer.ts";
import { ReviewStore, comparisonLabel, fileStatus, type PatchSnapshot, type ReviewBases } from "./review-store.ts";
import { ChangesView, changesModel, type ChangesModel } from "./changes-view.ts";
import { LifecycleView } from "./lifecycle-view.ts";
import { ArtifactView } from "./artifact-view.ts";
import { agentsModel, isLive, liveCount, statusLabel, type AgentRow, type AgentsModel } from "./agents-model.ts";
import { AgentsView, type AgentsAction } from "./agents-view.ts";
import { AgentConversation } from "./agent-conversation.ts";
import { HelperChat } from "./helper-chat.ts";
import { openOutputPager, pagerLines, type JobFreeze } from "./job-detail.ts";
import { agentColor } from "./agent-hues.ts";
import { InspectionStore } from "./inspection.ts";
import { renderProof, SPLIT_MIN } from "./proof-view.ts";
import { proofItems, type SectionId } from "./proof-parts.ts";
import { usageLines } from "./usage-view.ts";
import { TaskStore, taskHeadline, taskLines } from "./task-view.ts";
import { DecisionInbox, inboxIdentity } from "./decision-inbox.ts";
import { InboxView } from "./inbox-view.ts";
import { HistoryNavigator, type HistoryRead } from "./history.ts";
import {
  changesBadge, headerRows, mainTabs, proofBadge, tabRow, tabSpec, COMPACT_COLUMNS, NOTICE_MS, SHORT_ROWS,
  type HeaderInput, type Notice, type TabHit, type TabId, type TabSpec,
} from "./header.ts";
import type { HintState, KeyContext } from "./keymap.ts";
import type { SideSource } from "./sidebar.ts";
import { pad, plainLabel, styledPanelLine, PANEL_BLUE, PANEL_TEXT, type PanelSnapshot } from "./panels-view.ts";

export function plainClick(event: TuiMouseEvent): boolean {
  return event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && (event.clickCount ?? 1) === 1;
}
export type MainView = "task" | "conversation" | "agents" | "agent" | "changes" | "evidence" | "inbox" | "graph" | "timeline" | "usage" | "artifacts";
/** The tab a view highlights: one agent's conversation highlights Agents, Graph and Timeline highlight Task, Artifacts highlights Proof. */
const TAB_OF: Readonly<Record<MainView, TabId | undefined>> = {
  task: "task", graph: "task", timeline: "task", conversation: "conversation", agents: "agents", agent: "agents",
  changes: "changes", evidence: "evidence", artifacts: "evidence", inbox: "inbox", usage: undefined,
};
/** The footer and F1 context of each view while it has the focus. */
const CONTEXT_OF: Readonly<Record<MainView, KeyContext>> = {
  task: "task", graph: "lifecycle", timeline: "lifecycle", conversation: "composer", agents: "agents", agent: "agent",
  changes: "changes", evidence: "proof", artifacts: "artifacts", inbox: "inbox", usage: "usage",
};
const LETTERS: Readonly<Record<string, string>> = { Conflict: "U", New: "?", Renamed: "R", Deleted: "D", Added: "A", Modified: "M" };
/**
 * The letters each view defines. While the view has the focus, a defined letter that the current selection
 * does not offer does nothing; only letters a view does not define go on to the composer.
 */
const VIEW_LETTERS: Readonly<Record<MainView, string>> = {
  task: "rgl", conversation: "", agents: "jkstxmpbf/?", agent: "tsxm", changes: "jknpaf/cor", evidence: "jkar?",
  artifacts: "jkmcoxr", inbox: "jkhar/?", usage: "cr", graph: "jkr", timeline: "jkrhc",
};

export interface WorkbenchOptions {
  readonly jobControl?: JobControl;
  readonly identity: HostIdentity;
  readonly inspection?: () => Promise<unknown>;
  readonly lifecycle?: () => Promise<unknown>;
  readonly artifacts?: () => Promise<unknown>;
  readonly artifactViewer?: (reference: string, viewer: "show" | "serve") => Promise<string>;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly readTask?: () => Promise<unknown>;
  /** Physical terminal size for the chrome tiers; without a terminal the render width stands in and rows count as tall. */
  readonly terminal?: () => { readonly columns: number; readonly rows: number } | undefined;
  /** Called when the main view takes the logical focus, so the sidebar and the queue give it up. */
  readonly onFocusMain?: () => void;
  readonly cwd: string; readonly snapshot: () => PanelSnapshot | null;
  readonly changed: () => void; readonly canInteract: () => boolean;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
  readonly reviewBases?: ReviewBases;
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
  /** F8: helper calls and jobs, with keyed selection. */
  readonly agentsView: AgentsView;
  private conversation: AgentConversation | undefined;
  /** The agent whose conversation the `agent` view shows. */
  private agentKey: string | undefined;
  private ui: ExtensionUIContext | undefined;
  private changes: ChangesView | undefined;
  private changesScroll: ScrollView | undefined;
  private shownChanges: ChangesModel | undefined;
  private readingTranscript = false;
  /** Read-only pagers (job output, peek, transcript) this workbench has open. */
  private pagers = 0;
  private transcript: ScrollView | undefined; private transcriptWidth = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private debounce: ReturnType<typeof setTimeout> | undefined;
  private chatPresentation: (() => InteractiveChatPresentation) | undefined;
  private jobFreeze: JobFreeze | undefined;
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
  /** The repository the session launched in; header row 2 names another Changes scope only when it differs. */
  private readonly launchRoot: string;
  private shownNotice: Notice | undefined;
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;
  get scopeLabel(): string { return this.review.scopeRequired ? "Choose project" : this.projectName ?? this.review.name; }
  /** The panel-originated notice header row 1 shows in place of the state word, if any. */
  get currentNotice(): Notice | undefined { return this.shownNotice; }
  /** True while a pager or detail sheet the workbench opened is on screen; the footer lists its keys. */
  get pagerOpen(): boolean { return this.pagers > 0 || this.inboxView.sheetOpen; }
  /** Runs `show` (a read-only overlay) with the pager marked open. */
  private async inPager(show: () => Promise<void>): Promise<void> {
    this.pagers++; this.options.changed();
    try { await show(); } finally { this.pagers--; this.options.changed(); }
  }
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
    } : undefined, !!options.readProjects && !options.identity.task && !options.projectDir, !!options.projectDir, options.reviewBases);
    this.launchRoot = this.review.root;
    this.inbox = new DecisionInbox(options.readInbox, options.changed);
    this.inboxView = new InboxView(this.inbox, options.identity.role, options.identity.project, options.canInteract, options.changed,
      () => this.focusMain(), () => this.legend, text => this.notice(text, "warning"));
    this.agentsView = new AgentsView({
      model: () => this.agentsModel(), now: () => Date.now(), short: () => this.size(0).rows < SHORT_ROWS,
      legend: () => this.legend, freeze: () => this.jobFreeze, color: agentColor,
      click: event => options.canInteract() && plainClick(event), focus: () => { this.focusMain(); options.changed(); },
      act: (action, key) => this.agentAction(action, key),
    });
  }
  async openHistory(): Promise<void> { if (this.ui) await this.history.open(this.ui); }
  bind(ui: ExtensionUIContext): void { this.ui = ui; this.images.bind(); this.inboxView.bind(ui); void this.task.refresh(); }
  invalidateFiles(): void {
    this.inspection.invalidate();
    clearTimeout(this.debounce);
    this.debounce = setTimeout(() => { this.debounce = undefined; void this.review.refresh(); }, 300);
    this.debounce.unref();
  }
  private focusMain(): void { this.focused = true; this.options.onFocusMain?.(); }
  /** Physical terminal size, or the render width and an unbounded height outside a terminal. */
  private size(width: number): { readonly columns: number; readonly rows: number } {
    return this.options.terminal?.() ?? { columns: width, rows: Number.POSITIVE_INFINITY };
  }
  private compact(width: number): { readonly short: boolean; readonly tiny: boolean } {
    const size = this.size(width);
    return { short: size.rows < SHORT_ROWS, tiny: size.columns < COMPACT_COLUMNS };
  }
  /** Rows above an expanded image: the header only, since the tab row hides while the image is open. */
  private chromeRows(width: number): number {
    const { short, tiny } = this.compact(width);
    return short || tiny ? 1 : 2;
  }
  setView(view: MainView): void {
    if (this.view === "agent" && view !== "agent") this.conversation?.remember();
    this.images.close();
    this.view = view; this.focused = view !== "conversation";
    if (this.focused) this.options.onFocusMain?.();
    clearInterval(this.timer);
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
  /** F8: the agents view, with `key` selected when given. */
  openAgents(key?: string): void {
    if (key !== undefined) this.agentsView.select(key);
    this.setView("agents");
  }
  /** One agent's conversation in the reading area, where it was last read. */
  openAgent(key: string): void {
    if (this.view === "agent") this.conversation?.remember();
    this.agentsView.select(key); this.agentKey = key;
    this.conversation?.restore(key);
    this.setView("agent");
  }
  /** A tab click: the same as the tab's key. */
  openTab(id: TabId): void {
    if (id === "agents") this.openAgents(); else this.setView(id);
  }
  /** Panel feedback in header row 1 for NOTICE_MS; Pi's own notifications keep their native place. */
  notice(text: string, tone: Notice["tone"] = "text"): void {
    clearTimeout(this.noticeTimer);
    this.shownNotice = { text: plainLabel(text), tone };
    this.noticeTimer = setTimeout(() => { this.noticeTimer = undefined; this.shownNotice = undefined; this.options.changed(); }, NOTICE_MS);
    this.noticeTimer.unref();
    this.options.changed();
  }
  agentsModel(): AgentsModel {
    return agentsModel(this.options.snapshot(), Date.now(), { jobControl: !!this.options.jobControl });
  }
  private agentRow(key: string | undefined): AgentRow | undefined {
    return key === undefined ? undefined : this.agentsModel().agents.find(row => row.key === key);
  }
  private agentAction(action: AgentsAction, key: string): void {
    if (action === "open") { this.openAgent(key); return; }
    if (action === "transcript") { void this.openAgentTranscript(key); return; }
    const job = this.agentsModel().jobs.find(row => row.key === key)?.job;
    if (job === undefined) return;
    if (action === "output" || action === "search") void this.openJobOutput(job, action === "search");
    else if (action === "follow") this.toggleFollow(job);
    else if (action === "peek" || action === "background" || action === "stop") void this.controlJob(job, action);
  }
  private toggleFollow(id: string): void {
    const job = this.options.snapshot()?.jobs?.find(item => item.job === id);
    this.jobFreeze = this.jobFreeze?.job === id || !job?.detail ? undefined
      : { job: id, tail: job.detail.tail, lines: job.detail.lines, running: job.state === "running" };
    this.options.changed();
  }
  private async openJobOutput(id: string, search: boolean): Promise<void> {
    const ui = this.ui, snapshot = this.options.snapshot(), job = snapshot?.jobs?.find(item => item.job === id);
    if (!ui || !this.options.canInteract()) return;
    if (!job?.detail || !snapshot?.bound) { this.notice("Job output unavailable for this job source or host", "warning"); return; }
    const lines = pagerLines(job);
    let start = lines.length;
    if (search) {
      const query = await ui.input("Search retained job output");
      if (!query) return;
      start = lines.findIndex(line => line.includes(query));
      if (start < 0) { this.notice(`No match for ${JSON.stringify(plainLabel(query))} in retained output`); return; }
    }
    await this.inPager(() => openOutputPager(ui, `Job ${id} · retained output · ${lines.length} lines`, lines, start));
  }
  private async controlJob(job: string, action: "peek" | "background" | "stop"): Promise<void> {
    const ui = this.ui, snapshot = this.options.snapshot(), session = snapshot?.inspector?.session;
    if (!ui || !this.options.canInteract()) return;
    if (!snapshot?.bound || !session || !this.options.jobControl) { this.notice("Command control unavailable: stale or unsupported host", "warning"); return; }
    const target = { job, session, generation: this.options.identity.generation };
    if (action === "stop" && !await ui.confirm("Stop command?", "Stop the owned process group for " + plainLabel(job) + "? Output is retained.")) return;
    try {
      const result = await this.options.jobControl(target, action);
      if (action === "peek" && typeof result.text === "string") {
        const lines = result.text.split("\n").map(plainLabel);
        await this.inPager(() => openOutputPager(ui, "Peek " + plainLabel(job) + " · " + plainLabel(String(result.state)) + " · current logs", lines, lines.length));
      } else this.notice(plainLabel(JSON.stringify(result)), result.state === "denied" || result.state === "outcome_unknown" ? "warning" : "text");
    } catch (error) { this.notice("Command control outcome unknown; inspect before retrying: " + plainLabel(String(error)), "warning"); }
    this.options.changed();
  }
  async openFile(path: string): Promise<void> {
    this.setView("changes"); await this.review.select(path); this.acceptPatch();
  }
  private pendingInbox(): number {
    return this.inbox.snapshot?.items.filter(item => item.pending).length ?? 0;
  }
  private headerInput(): HeaderInput {
    const snapshot = this.options.snapshot(), identity = this.options.identity, r = this.review, pending = this.pendingInbox();
    const state: HeaderInput["state"] = !snapshot ? "Unavailable" : snapshot.streaming ? "Working" : "Idle";
    const head = taskHeadline(identity, this.task);
    const elsewhere = r.scopeRequired || r.root !== this.launchRoot;
    const context = head ? { left: `${head.id} · ${head.title}`, right: head.chips.join(" · ") } : {
      left: `${identity.project || "workspace"} · ${basename(this.options.cwd)}${!elsewhere && r.branch !== "unknown" ? ` (${r.branch})` : ""}`,
      ...(elsewhere ? { right: `Changes: ${this.scopeLabel}` } : {}),
    };
    return {
      title: target(identity), state, context,
      ...(pending ? { attention: { text: `⚑ ${pending}`, tone: "warning" as const } } : {}),
      ...(this.shownNotice ? { notice: this.shownNotice } : {}),
    };
  }
  private tabSpecs(): TabSpec[] {
    const snapshot = this.options.snapshot(), r = this.review, data = this.inspection.current;
    const live = liveCount(this.agentsModel()), pending = this.pendingInbox();
    const badge = (id: TabId): TabSpec["badge"] => {
      switch (id) {
        case "task": return undefined;
        case "conversation": return snapshot?.streaming ? { text: "●", tone: "accent" } : undefined;
        case "agents": return live ? { text: String(live), tone: "accent" } : undefined;
        case "changes": return changesBadge({ checked: r.checkedAt !== undefined, files: r.files.length, scopeRequired: r.scopeRequired, error: r.error !== undefined });
        case "evidence": return proofBadge({ gate: data?.gate ?? undefined, review: data?.review ?? undefined, stale: this.inspection.stale, error: this.inspection.error !== undefined });
        case "inbox": return pending ? { text: String(pending), tone: "warning" } : undefined;
      }
    };
    return mainTabs(this.options.identity).map(id => tabSpec(id, badge(id)));
  }
  private chrome(): { readonly header: Component; readonly tabs: Component } {
    let hits: readonly TabHit[] = [];
    const header: Component = {
      render: width => {
        // Renders before the transcript, so a paused reader keeps the same passage at a new width.
        this.anchorTranscript(width);
        return headerRows(this.headerInput(), width, this.compact(width));
      }, invalidate() {},
    };
    const tabs: Component = {
      render: width => {
        const row = tabRow(this.tabSpecs(), TAB_OF[this.view], width);
        hits = row.hits;
        return [row.line];
      },
      handleMouse: event => {
        if (!this.options.canInteract() || !plainClick(event) || event.y !== 0) return undefined;
        const hit = hits.find(item => event.x >= item.from && event.x < item.to);
        if (!hit) return undefined;
        this.openTab(hit.id);
        return { handled: true };
      }, invalidate() {},
    };
    return { header, tabs };
  }
  /**
   * Runs in the header's render, before Pi lays out the transcript at a new width. A reader
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
    const terminal = this.options.terminal;
    this.changes = new ChangesView({ model: () => this.changesModel(), click: event => this.options.canInteract() && plainClick(event),
      focus: () => { this.focusMain(); this.options.changed(); }, step: delta => void this.selectRelative(delta),
      select: path => void this.review.select(path).then(() => { this.acceptPatch(); this.changesScroll?.scrollToStart(); }),
      pickHunk: index => { this.hunk = index; this.options.changed(); }, color: agentColor,
      ...(terminal ? { rows: () => this.size(0).rows } : {}) });
    this.changesScroll = this.changes.diff;
    const chat = new HelperChat(() => this.chatPresentation?.().tui, this.options.cwd, presentation);
    chat.expandTools(this.toolsExpanded);
    this.conversation = new AgentConversation({ row: () => this.agentRow(this.agentKey), chat, color: agentColor, now: () => Date.now(),
      click: event => this.options.canInteract() && plainClick(event), focus: () => { this.focusMain(); this.options.changed(); },
      back: () => this.openAgents() });
    if (this.agentKey !== undefined) this.conversation.restore(this.agentKey);
    this.evidenceScroll = new ScrollView({ render: width => this.renderEvidence(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      const index = this.criterionHits.get(event.y); if (index !== undefined) this.criterionIndex = index;
      this.focusMain(); this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.lifecycleScroll = new ScrollView({ render: width => this.lifecycle.render(width), handleMouse: event => {
      const result = this.lifecycle.handleMouse(event);
      if (result?.handled) { this.focusMain(); this.options.changed(); }
      return result;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.usageScroll = new ScrollView({ render: width => {
      const snapshot = this.options.snapshot();
      return usageLines(this.inspection, snapshot?.budget, snapshot?.bound ?? false, width, this.usageChart)
        .flatMap(row => new Text((row.heading ? PANEL_BLUE : PANEL_TEXT) + row.text, 0, 0).render(width));
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.artifactScroll = new ScrollView({ render: width => this.artifacts.render(width), handleMouse: event => {
      const result = this.artifacts.handleMouse(event);
      if (result?.handled) { this.focusMain(); this.options.changed(); }
      return result;
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.inboxScroll = new ScrollView(this.inboxView, { primary: true, follow: "none", scrollbar: "auto" });
    this.taskScroll = new ScrollView({ render: width => taskLines(this.options.identity, this.task)
      .flatMap(row => new Text(styledPanelLine(row), 0, 0).render(width)), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      this.focusMain(); this.options.changed(); return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    const chrome = this.chrome();
    return new VStack([
      { component: chrome.header, basis: "auto", shrink: 0 },
      // An expanded image takes the tab row's place; Esc or F9 brings both back.
      { component: chrome.tabs, basis: 1, shrink: 0, visible: () => !this.images.isOpen },
      { component: this.taskScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "task" && !this.images.isOpen },
      { component: transcript, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "conversation" && !this.images.isOpen },
      { component: this.images, basis: 0, grow: 1, minSize: 1, visible: viewport => {
        this.images.setViewportHeight(Math.max(0, viewport.height - this.chromeRows(viewport.width))); return this.images.isOpen;
      } },
      { component: this.changes.root, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "changes" },
      { component: this.agentsView.scroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agents" },
      { component: this.conversation.scroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agent" },
      { component: this.evidenceScroll, basis: 0, grow: 1, minSize: 1,
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
  /** Bounded read-only transcript of the agent at `key`, only when its observation is eligible on a bound host. */
  async openAgentTranscript(key: string | undefined): Promise<void> {
    const ui = this.ui, read = this.options.readTranscript, row = this.agentRow(key);
    if (!ui || !this.options.canInteract() || this.readingTranscript) return;
    if (key === undefined || !row?.capabilities.transcript || !read) {
      this.notice("Transcript unavailable: the agent is missing, stale or has no eligible transcript", "warning"); return;
    }
    this.readingTranscript = true;
    try {
      const text = await read(row.id);
      if (this.scopeAbort.signal.aborted) return;
      const shown = this.view === "agent" ? this.agentKey : this.agentsView.entry()?.row.key;
      if (shown !== key || !this.agentRow(key)?.capabilities.transcript) throw new Error("Agent selection or binding changed during read");
      // Read-only bounded text, never an SDK switchSession/open/resume call.
      await this.inPager(() => ui.custom<void>((tui, _theme, _keys, done) => {
        const content = new Text(text.slice(0, 32 * 1024).replace(/[\u0000-\u001f\u007f-\u009f]/g, char => char === "\n" ? char : " "), 0, 0);
        let offset = 0, max = 0;
        return { render: width => {
          const lines = content.render(width), height = Math.max(1, Math.floor(tui.terminal.rows * 0.8) - 2);
          max = Math.max(0, lines.length - height); offset = Math.max(0, Math.min(max, offset));
          return [pad(`Transcript · ${plainLabel(row.name)} · read-only`, width), ...lines.slice(offset, offset + height)];
        }, handleInput: data => {
            if (matchesKey(data, "escape") || data === "q") { done(); return; }
            if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) offset = Math.max(0, Math.min(max, offset + (matchesKey(data, "up") ? -1 : matchesKey(data, "down") ? 1 : matchesKey(data, "pageUp") ? -10 : 10)));
            tui.requestRender();
          }, invalidate() { content.invalidate(); } };
      }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } }));
    } catch (error) {
      if (!this.scopeAbort.signal.aborted) this.notice(`Transcript unavailable: ${plainLabel(error instanceof Error ? error.message : String(error))}`, "warning");
    } finally { this.readingTranscript = false; this.options.changed(); }
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
    this.shownChanges = changesModel(r, { root: this.displayedRoot, path: this.displayedPath, comparison: this.displayedComparison, text: this.lastPatch,
      head: this.displayedHead, base: this.displayedBase, file: this.displayedFile }, this.scopeLabel, this.hunk, Date.now(), [], this.changes?.filter ?? "all");
    return this.shownChanges;
  }
  /** j/k in Changes: the next or previous file the list shows, so files the author filter hides are skipped. */
  async selectRelative(delta: number): Promise<void> {
    const rows = this.changesModel().files, from = rows.findIndex(file => file.path === this.review.selected);
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
        if (!scopes.length) { this.notice("No configured projects. Add a project in Corral settings.", "warning"); return; }
        const scope = await chooseProject(scopes, (title, choices) => ui.select(title, choices));
        path = scope?.path; name = scope?.label;
      } else path = await ui.input("Read-only repository scope (does not rebind this session)", this.review.root);
      if (path && !this.scopeAbort.signal.aborted) {
        this.lastPatch = ""; this.displayedPath = undefined; this.projectName = name;
        await this.review.setRepository(path); this.changesScroll?.scrollToStart(); this.options.changed();
      }
    } catch (error) {
      if (!this.scopeAbort.signal.aborted) this.notice(`Projects unavailable: ${plainLabel(error instanceof Error ? error.message : String(error))}`, "error");
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
      this.notice("Wait for the selected comparison to load before attaching its hunk.", "warning"); return;
    }
    const source = this.lastPatch.split("\n");
    const end = sources[this.hunk + 1] ?? source.length;
    const excerpt = start === undefined ? source.slice(0, 18) : source.slice(start, Math.min(end, start + 24));
    const reference = `Please review ${JSON.stringify(file.path)} in ${this.review.root}\nComparison: ${this.displayedComparison} · base ${this.displayedBase ?? "see comparison"} · HEAD ${this.displayedHead ?? "unborn/unavailable"}\nDiff SHA256: ${createHash("sha256").update(this.lastPatch).digest("hex")}\nSelected Git excerpt (observed ${this.displayedAt ? new Date(this.displayedAt).toISOString() : "time unavailable"}):\n` + excerpt.map(line => "> " + line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")).join("\n").slice(0, 3000);
    const draft = ui.getEditorText(); ui.setEditorText(draft + (draft ? "\n\n" : "") + reference + "\n");
    this.focused = false; this.options.changed(); // Never submit or overwrite the existing draft.
  }
  /** The view one step back: an agent's conversation returns to Agents, Graph and Timeline to Task, Artifacts to Proof, the rest to Chat. */
  private back(): void {
    if (this.view === "agent") this.openAgents();
    else if (this.view === "graph" || this.view === "timeline") this.setView("task");
    else if (this.view === "artifacts") this.setView("evidence");
    else this.setView("conversation");
  }
  /** The footer and F1 context of the open view while it has the focus. */
  mainContext(): KeyContext { return this.images.isOpen ? "image" : CONTEXT_OF[this.view]; }
  /** What the footer hints for `context`; the panels own the focus and the queue. */
  hintState(context: KeyContext, queue: HintState["queue"]): HintState {
    const identity = this.options.identity;
    const selection = context === "agents" || context === "agent" ? this.agentsView.actions() : undefined;
    return {
      context, streaming: this.options.snapshot()?.streaming ?? false, queue,
      mode: { taskBound: !!identity.task, answer: identity.role === "pm" || identity.role === "assistant", lifecycle: !!identity.task },
      ...(selection ? { selection } : {}),
      authors: (this.shownChanges?.authors.length ?? 0) > 0, legend: this.legend, history: this.inbox.history,
      ...(this.view === "timeline" ? { timelineSource: this.lifecycle.source } : {}),
    };
  }
  /** The sidebar's inputs: the agents view's rows, the Changes file list and the plan. */
  sideSource(): SideSource {
    const snapshot = this.options.snapshot(), model = this.agentsModel(), r = this.review;
    return {
      agents: model.sources.helpers ? model.agents.map(row => ({
        key: row.key, name: row.name, hue: row.hue, profile: row.profile, glyph: row.status.glyph, tone: row.status.tone, activity: row.activity,
        settled: isLive(row.state) ? undefined : row.state === "done" ? "ok" as const : "failed" as const,
      })) : null,
      jobs: model.sources.jobs ? model.jobs.map(row => ({
        key: row.key, command: row.command, glyph: row.status.glyph, tone: row.status.tone, time: statusLabel(row.status),
        settled: row.live ? undefined : row.status.tone === "success" ? "ok" as const : "failed" as const,
      })) : null,
      files: {
        scope: this.scopeLabel,
        state: r.scopeRequired ? "choose" : r.error !== undefined ? "error" : r.checkedAt === undefined ? "loading" : "ok",
        ...(r.error !== undefined ? { error: r.error } : {}),
        list: r.visible.map(file => {
          const stat = r.stats.get(file.path);
          return { path: file.path, letter: LETTERS[fileStatus(file)] ?? "M", added: stat?.added, removed: stat?.removed };
        }),
      },
      plan: { progress: snapshot?.ledger ?? null, stale: snapshot?.ledgerStale ?? false },
      bound: snapshot?.bound ?? false,
      color: agentColor,
    };
  }
  handleInput(data: string): boolean {
    const tabs = mainTabs(this.options.identity);
    if (matchesKey(data, "f9")) { this.setView("conversation"); return true; }
    if (matchesKey(data, "f5") && tabs.includes("task")) { this.setView("task"); return true; }
    if (matchesKey(data, "f7")) { this.setView("changes"); return true; }
    if (matchesKey(data, "f8")) { this.openAgents(); return true; }
    if (matchesKey(data, "f10") && tabs.includes("evidence")) { this.setView("evidence"); return true; }
    if (matchesKey(data, "f11")) { this.setView("inbox"); return true; }
    if (this.images.isOpen && matchesKey(data, "escape")) { this.images.close(); return true; }
    if (!this.focused || this.view === "conversation") return false;
    if (matchesKey(data, "escape")) { this.back(); return true; }
    const view = this.view;
    if (view === "task" && tabs.includes("task") && (data === "g" || data === "l")) { this.setView(data === "g" ? "graph" : "timeline"); return true; }
    if ((view === "graph" || view === "timeline") && this.lifecycle.handleInput(data)) { this.lifecycleScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (view === "inbox" && this.inboxView.handleInput(data)) return true;
    if (view === "agents") {
      if (this.agentsView.handleKey(data)) { this.options.changed(); return true; }
      // `t` on an agent whose transcript is not offered says why instead of typing.
      const entry = this.agentsView.entry();
      if (data === "t" && entry?.kind === "agent") { void this.openAgentTranscript(entry.row.key); return true; }
    }
    if (view === "agent") {
      if (getKeybindings().matches(data, "app.tools.expand")) {
        this.toolsExpanded = !this.toolsExpanded; this.conversation?.expandTools(this.toolsExpanded); this.options.changed(); return true;
      }
      if (data === "t") { void this.openAgentTranscript(this.agentKey); return true; }
    }
    if (view === "evidence" && data === "a") { this.setView("artifacts"); return true; }
    if (view === "evidence" && matchesKey(data, "enter")) { this.proofEnter(); return true; }
    if (view === "artifacts" && this.artifacts.handleInput(data)) { this.artifactScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (matchesKey(data, "enter") || matchesKey(data, "tab")) { this.focused = false; this.options.changed(); return true; }
    if ((view === "evidence" || view === "agents" || view === "inbox") && data === "?") { this.legend = !this.legend; this.options.changed(); return true; }
    if (view === "usage" && data === "c") { this.usageChart = !this.usageChart; this.options.changed(); return true; }
    const scroll = view === "task" ? this.taskScroll : view === "usage" ? this.usageScroll : view === "artifacts" ? this.artifactScroll
      : view === "graph" || view === "timeline" ? this.lifecycleScroll : view === "agent" ? this.conversation?.scroll
      : view === "changes" ? this.changesScroll : view === "evidence" ? this.evidenceScroll : view === "inbox" ? this.inboxScroll : undefined;
    if (view === "inbox" && (data === "j" || data === "k" || matchesKey(data, "up") || matchesKey(data, "down"))) {
      this.inbox.move(data === "j" || matchesKey(data, "down") ? 1 : -1);
      const row = this.inbox.current && this.inboxView.rows.get(inboxIdentity(this.inbox.current));
      if (row !== undefined) scroll?.scrollTo(row, { disableFollow: true });
      this.options.changed(); return true;
    }
    if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      scroll?.scrollBy(matchesKey(data, "up") ? -3 : matchesKey(data, "down") ? 3 : matchesKey(data, "pageUp") ? -12 : 12); this.options.changed(); return true;
    }
    if ((data === "j" || data === "k") && (view === "changes" || view === "evidence")) {
      if (view === "changes") void this.selectRelative(data === "j" ? 1 : -1);
      else this.criterionIndex = Math.max(0, Math.min(this.proofRows() - 1, this.criterionIndex + (data === "j" ? 1 : -1)));
      this.options.changed(); return true;
    }
    if ((view === "evidence" || view === "usage") && data === "r") { void this.inspection.refresh(true); return true; }
    if (view === "task" && data === "r") { void this.task.refresh(true); return true; }
    if (view === "changes") {
      if (data === "/") { void this.search(); return true; }
      if (data === "o") { void this.repository(); return true; }
      if (data === "c") { const choices = this.review.comparisons; const next = choices[(choices.indexOf(this.review.comparison) + 1) % choices.length]; if (next) { this.lastPatch = ""; void this.review.setComparison(next); } return true; }
      if (data === "r") { this.inspection.invalidate(); void this.review.refresh().then(() => this.acceptPatch()); return true; }
      if (data === "n" || data === "p") { this.moveHunk(data === "n" ? 1 : -1); return true; }
      if (data === "a") { this.askHunk(); return true; }
      if (data === "f") { this.changes?.cycleFilter(); this.options.changed(); return true; }
    }
    if (data.length === 1 && VIEW_LETTERS[view].includes(data)) return true; // Defined here, not offered now.
    if (data.startsWith("\x1b")) return false; // Terminal replies/focus events are not typing.
    this.focused = false; return false;
  }
  dispose(): void {
    clearInterval(this.timer); clearTimeout(this.debounce); clearTimeout(this.noticeTimer);
    this.chatPresentation = undefined;
    this.history.dispose(); this.scopeAbort.abort(); this.images.dispose(); this.review.dispose(); this.inspection.dispose(); this.task.dispose(); this.inboxView.dispose(); this.lifecycle.dispose(); this.artifacts.dispose(); this.ui = undefined;
  }
}
