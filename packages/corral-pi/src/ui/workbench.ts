import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { basename, relative, resolve } from "node:path";
import { chooseProject, projectScopes } from "./project-scopes.ts";
import type { HostIdentity } from "../roles/types.ts";
import type { JobControl } from "../host/job-controls.ts";
import { LIVE_CHILD_STATES } from "../build/types.ts";
import type { BuildPanelState, BuildUi, FleetControl } from "../build/types.ts";
import { target } from "./chrome.ts";
import type { ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { HStack, VStack, ScrollView, getKeybindings, matchesKey, type Component, type OverlayOptions, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { anchoredRow } from "./reading-anchor.ts";
import { NativeImageViewer } from "./image-viewer.ts";
import { ReviewStore, comparisonLabel, fileStatus, type PatchSnapshot, type ReviewBases } from "./review-store.ts";
import { ChangesView, changesModel, type ChangeAuthor, type ChangesModel } from "./changes-view.ts";
import { LifecycleView } from "./lifecycle-view.ts";
import { ArtifactView } from "./artifact-view.ts";
import { agentsModel, isLive, liveCount, statusLabel, visibleCommands, type AgentActions, type AgentRow, type AgentsModel } from "./agents-model.ts";
import { AgentsView, type AgentsAction } from "./agents-view.ts";
import { AgentConversation } from "./agent-conversation.ts";
import { HelperChat } from "./helper-chat.ts";
import { openOutputPager, pagerLines, type JobFreeze } from "./job-detail.ts";
import { agentColor } from "./agent-hues.ts";
import { renderProof, SPLIT_MIN } from "./proof-view.ts";
import { gateCheck, proofItems, reviewChecks, type SectionId } from "./proof-parts.ts";
import { usageBackAt, usageLines } from "./usage-view.ts";
import { usageModel, type UsageIdentity } from "./usage-model.ts";
import { TaskStore } from "./task-store.ts";
import { taskHeadline, taskLines } from "./task-view.ts";
import { HistoryNavigator, type HistoryRead } from "./history.ts";
import {
  boardBadge, changesBadge, headerFrame, mainTabs, proofBadge, tabRow, tabSpec, COMPACT_COLUMNS, NOTICE_MS, SHORT_ROWS,
  type HeaderHit, type HeaderInput, type HeaderStatus, type Notice, type TabHit, type TabId, type TabSpec,
} from "./header.ts";
import { FILTER_KEY, keyLabel, piKey, viewKey, type HintState, type KeyContext } from "./keymap.ts";
import { emptyChatLines, jumpLabel, labelJumpControl, TranscriptWatch } from "./chat-overlay.ts";
import { openPager } from "./pager.ts";
import type { BuildController } from "./build/build-controller.ts";
import { BoardView, boardDetailLines, type BoardAction } from "./build/board-view.ts";
import { childIdOf, childKey } from "./build/contract.ts";
import { childActions, childAuthors, fleetExtras, shownFleet, steerable } from "./build/fleet-rows.ts";
import { childPrompts, type ChildCards } from "./fleet-prompts.ts";
import type { SideSource } from "./sidebar.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_LINE, PANEL_TEXT, type PanelSnapshot } from "./panels-view.ts";
import { rule, spinner, VIEW_INSET } from "./frame.ts";
import { popupPlacement } from "./popup-placement.ts";
import { shiftRows } from "./selection.ts";

/** Below this many terminal rows the rule under the tab row goes first. */
export const RULE_ROWS = 16;
/** The rail beside the main view: the sidebar while it shows as a rail. */
export interface RailSource {
  /** Cells of the rail while it shows (the `│` column on its left not included), else undefined. */
  width(): number | undefined;
  /** `height` rows of the rail, each `width` cells. */
  render(width: number, height: number): readonly string[];
  /** Where the rail's rows start on screen and how many there are, after each layout. */
  placed(top: number, height: number): void;
}
/**
 * The rail column: a `│` in the line colour, then the rail. It renders the height of its last layout and asks for
 * one more render when the layout gives it another height, so the rail always fills the rows between the tab
 * rule and the dock exactly.
 */
function railColumn(source: RailSource, top: () => number, estimate: () => number): Component {
  let height = 0;
  const content: Component = {
    render: () => {
      const width = source.width() ?? 0, rows = Math.max(1, height || estimate());
      const lines = source.render(width, rows);
      return Array.from({ length: rows }, (_, row) => PANEL_BG + PANEL_LINE + "│" + PANEL_TEXT + (lines[row] ?? PANEL_BG + " ".repeat(width)));
    },
    invalidate() {},
  };
  return new class extends ScrollView {
    override updateLayout(contentHeight: number, viewportHeight: number, requestRender: () => void): void {
      super.updateLayout(contentHeight, viewportHeight, requestRender);
      source.placed(top(), viewportHeight);
      if (viewportHeight !== height) { height = viewportHeight; requestRender(); }
    }
  }(content, { follow: "none", primary: false, scrollbar: "hidden" });
}
/**
 * `component` one cell in from the main view's left edge, like the header, the tab row and the other views, with one
 * blank cell on the right (where a scroll bar draws). It renders `VIEW_INSET` cells narrower, so it wraps instead of
 * overflowing, and pointer columns move with it; the view's layout thresholds add `VIEW_INSET` back.
 */
function inset(component: Component): Component {
  return {
    render: width => component.render(Math.max(1, width - VIEW_INSET)).map(line => " " + line),
    handleMouse: event => component.handleMouse?.({ ...event, x: Math.max(0, event.x - 1), width: Math.max(1, event.width - VIEW_INSET) }),
    selectionRows: (width, y) => shiftRows(component.selectionRows?.(Math.max(1, width - VIEW_INSET), y), 0, 1),
    invalidate: () => component.invalidate(),
  };
}

export function plainClick(event: TuiMouseEvent): boolean {
  return event.type === "click" && event.button === "left" && !event.shift && !event.alt && !event.ctrl && (event.clickCount ?? 1) === 1;
}
export type MainView = "task" | "conversation" | "agents" | "agent" | "changes" | "evidence" | "timeline" | "usage" | "artifacts" | "board";
/**
 * The tab a view highlights: one agent's conversation highlights Agents, Timeline highlights Task, Artifacts
 * highlights Proof, Usage highlights Chat.
 */
const TAB_OF: Readonly<Record<MainView, TabId>> = {
  task: "task", timeline: "task", conversation: "conversation", agents: "agents", agent: "agents",
  changes: "changes", evidence: "evidence", artifacts: "evidence", usage: "conversation", board: "board",
};
/** A sub-view's place, read on its highlighted tab when the row has room and in the footer's lead. */
const PLACE: Readonly<Partial<Record<MainView, string>>> = {
  timeline: "Task › Events", artifacts: "Proof › Artifacts", usage: "Chat › Usage",
};
/** The footer and F1 context of each view while it has the focus. */
const CONTEXT_OF: Readonly<Record<MainView, KeyContext>> = {
  task: "task", timeline: "lifecycle", conversation: "composer", agents: "agents", agent: "agent",
  changes: "changes", evidence: "proof", artifacts: "artifacts", usage: "usage", board: "board",
};
const LETTERS: Readonly<Record<string, string>> = { Conflict: "U", New: "?", Renamed: "R", Deleted: "D", Added: "A", Modified: "M" };
/** A child conversation's own keys; each acts only while the child's row offers it, else the letter is typing. */
const CHILD_KEYS: Readonly<Record<string, "answer" | "steer" | "cancel" | "model">> = { a: "answer", s: "steer", x: "cancel", m: "model" };
type ChildActs = Required<Pick<AgentActions, "steer" | "cancel" | "model" | "answer">>;
/** The most recent notices `recentNotices()` keeps for the F2 pager. */
const NOTICE_LOG = 50;
/** One logged notice: the sentence shown, its tone and time, and the exact raw detail it carried, if any. */
export interface NoticeEntry {
  readonly at: number; readonly text: string; readonly tone: Notice["tone"];
  /** The unsanitised record (stack, exit text, a host answer), kept whole for the Recent notices pager. */
  readonly detail?: string;
}

/** Why this build session cannot file board tasks, or undefined when it can. */
export function boardUnavailable(state: BuildPanelState | undefined): string | undefined {
  if (!state) return undefined;
  if (!state.session.workspaceRoot) return "This session is outside a corral workspace, so it cannot file board tasks.";
  if (!state.session.project) return "This session has no configured corral project, so it cannot file board tasks.";
  return undefined;
}
/** The exit status that replaces header row 1's state word; pending reads as a prompt only while its dialog is hidden. */
function exitStatus(phase: BuildPanelState["exit"]["phase"], docked: boolean): HeaderStatus | undefined {
  const quiet = (long: string, short: string): HeaderStatus => ({ long, short, tone: "muted", clickable: false });
  switch (phase) {
    case "pending": return docked ? undefined : { long: "exit choice pending", short: "choose exit", tone: "warning", clickable: true };
    case "rebasing": return quiet("Working · rebase", "rebase");
    case "verifying": return quiet("Working · full gate", "gate");
    case "repairing": return quiet("Working · gate repair", "repair");
    case "landing": return quiet("Working · land", "land");
    case "kept": return quiet("Idle · kept on branch", "kept");
    case "landed": return quiet("Idle · landed", "landed");
    case "failed": return { long: "Idle · exit failed", short: "exit failed", tone: "error", clickable: false };
    case "none": return undefined;
  }
}
/** What landing from the session needs in this project. */
const POLICY: Readonly<Record<BuildPanelState["settings"]["exitPolicy"], { readonly long: string; readonly short: string }>> = {
  gate: { long: "land: full gate", short: "land: gate" },
  review: { long: "land: review + full gate", short: "land: review" },
  none: { long: "landing off", short: "land: off" },
};
/** A bare session has no corral project and no gate: Land fast-forwards the branch it started from, locally. */
function barePolicy(session: BuildPanelState["session"]): { readonly long: string; readonly short: string } {
  return session.repository === false ? { long: "bare · not a Git repository", short: "bare · no Git" }
    : { long: "bare · land: local fast-forward, no gate", short: "bare · no gate" };
}

export interface WorkbenchOptions {
  readonly jobControl?: JobControl;
  readonly identity: HostIdentity;
  readonly artifacts?: () => Promise<unknown>;
  /** `corral artifact show` into a private file: the artifact's exact bytes, for the Artifacts view's text and image preview. */
  readonly artifactFile?: (reference: string) => Promise<string>;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly readTask?: () => Promise<unknown>;
  /** Physical terminal size for the chrome tiers; without a terminal the render width stands in and rows count as tall. */
  readonly terminal?: () => { readonly columns: number; readonly rows: number } | undefined;
  /** Called when the main view takes the logical focus, so the sidebar and the queue give it up. */
  readonly onFocusMain?: () => void;
  readonly cwd: string; readonly snapshot: () => PanelSnapshot | null;
  readonly changed: () => void; readonly canInteract: () => boolean;
  /** A dialog or overlay holds the keys: transcripts keep a paused reader's position under it. */
  readonly modal?: () => boolean;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
  readonly reviewBases?: ReviewBases;
  /** A build session's controls; every build surface below is absent without them. */
  readonly build?: BuildUi;
  /** Docked dialogs, the message target and conversation entries of a build session. */
  readonly buildController?: BuildController;
  /** The child runtime of a session that is not a build session, read when a child is acted on (the host builds it after the panels); absent: its children are read-only. */
  readonly fleet?: () => FleetControl | undefined;
  /** The host's cards for the fleet's questions while a terminal is attached; an answer prompt answers through them. */
  readonly childCards?: ChildCards;
  /** The sidebar as a rail beside the main view; without it the main view takes the full width. */
  readonly rail?: RailSource;
  /** While the agent works: the header state word's spinner and the turn's elapsed time (activity.ts). */
  readonly mark?: () => HeaderInput["mark"];
  /** Told each frame whether any helper, child or job runs, so the shared clock keeps their spinners and times moving. */
  readonly live?: (live: boolean) => void;
}
/** Main-area composition via the packaged SDK's public mainView hook; editor stays SDK-owned. */
export class NativeWorkbench {
  view: MainView = "conversation"; focused = false;
  readonly review: ReviewStore;
  readonly task: TaskStore;
  private taskScroll: ScrollView | undefined;
  readonly lifecycle: LifecycleView;
  readonly history: HistoryNavigator;
  private lifecycleScroll: ScrollView | undefined;
  private usageScroll: ScrollView | undefined;
  readonly artifacts: ArtifactView; private artifactScroll: ScrollView | undefined;
  private evidenceScroll: ScrollView | undefined; private criterionIndex = 0; private criterionHits = new Map<number, number>();
  readonly images: NativeImageViewer;
  /** F8: helper calls and jobs, with keyed selection. */
  readonly agentsView: AgentsView;
  /** F10 in a build session: the board tasks this session filed. */
  readonly boardView: BoardView | undefined;
  /** Steer, cancel, model and answer on child rows: a build session's controls, else the session's own fleet runtime once the host has built it. */
  private readonly buildActs: ChildActs | undefined;
  private fleetActs: { readonly fleet: FleetControl; readonly acts: ChildActs } | undefined;
  /** Resolved Changes roots, so a symlinked path still matches the session worktree. */
  private readonly realRoots = new Map<string, string>();
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
  /** Chat's empty state, new-rows count and unread mark (chat-overlay.ts); undefined until Pi mounts its transcript. */
  private watch: TranscriptWatch | undefined;
  /** The same new-rows count for the open agent conversation, which follows its end too. */
  private childWatch: TranscriptWatch | undefined;
  /** The main view's width in the last frame: at another width both conversations rewrap and their counts restart. */
  private watchedWidth = 0;
  /** Pi's renderer, whose jump-to-latest control carries the new-rows count. */
  private chatTui: InteractiveChatPresentation["tui"] | undefined;
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
  private readonly notices: NoticeEntry[] = [];
  /** Whether the last header frame drew the current notice in its state place. */
  private noticeDrawn = false;
  /** Rows of the header in the last frame: the rail starts below them, the tab row and its rule. */
  private headerRows = 0;
  get scopeLabel(): string { return this.review.scopeRequired ? "Choose project" : this.projectName ?? this.review.name; }
  /** The panel-originated notice of the moment, if any: header row 1 shows it, or the toast when the header could not. */
  get currentNotice(): Notice | undefined { return this.shownNotice; }
  /** True when the last header frame drew the current notice; the panels' toast shows it only while this is false. */
  get noticeInHeader(): boolean { return this.shownNotice !== undefined && this.noticeDrawn; }
  /** The last NOTICE_LOG notices with their time and raw detail, oldest first, for the F2 `Recent notices` pager. */
  recentNotices(): readonly NoticeEntry[] { return this.notices; }
  /** True while a pager or detail sheet the workbench opened is on screen; the footer lists its keys. */
  get pagerOpen(): boolean { return this.pagers > 0; }
  /** Runs `show` (a read-only overlay) with the pager marked open. */
  private async inPager(show: () => Promise<void>): Promise<void> {
    this.pagers++; this.options.changed();
    try { await show(); } finally { this.pagers--; this.options.changed(); }
  }
  constructor(private readonly options: WorkbenchOptions) {
    this.history = new HistoryNavigator(options.readHistory, options.canInteract, (text, tone) => this.notice(text, tone), () => this.popupPlace());
    this.images = new NativeImageViewer(() => options.canInteract(), () => { this.focused = this.images.isOpen || this.view !== "conversation"; options.changed(); });
    const focused = () => this.focused;
    this.lifecycle = new LifecycleView(options.identity, () => options.snapshot()?.bound ?? false, options.changed,
      options.canInteract, () => options.snapshot()?.inspector, { legend: () => this.legend, rows: () => this.lifecycleScroll?.viewportHeight ?? 0, focused });
    this.artifacts = new ArtifactView({ identity: options.identity, load: options.artifacts, file: options.artifactFile, exportSession: options.exportSession,
      openImage: source => this.images.activate(source), bound: () => options.snapshot()?.bound ?? false, changed: options.changed,
      canInteract: options.canInteract, legend: () => this.legend, focused });
    this.task = new TaskStore(options.identity, options.readTask, () => options.changed());
    // Every role starts on Chat with the keys in the composer, so the first typed word is never a view key; a
    // dispatched role's empty Chat says it waits for corral's first instruction. F5 or Alt+1 opens Task.
    this.projectName = options.identity.project || undefined;
    const { task, run } = options.identity;
    this.review = new ReviewStore(options.projectDir ?? options.cwd, () => options.changed(), options.readTask && task && run ? async () => {
      await this.task.refresh();
      const bound = this.task.run;
      if (this.task.stale || !bound) throw new Error(this.task.error ?? "Canonical candidate unavailable; r to reconcile");
      return { task, run, root: bound.worktree, base: bound.base_sha, ...(bound.head_sha ? { head: bound.head_sha } : {}) };
    } : undefined, !!options.readProjects && !options.identity.task && !options.projectDir && !options.build, !!options.projectDir || !!options.build,
      options.build ? this.buildBases(options.reviewBases) : options.reviewBases);
    this.launchRoot = this.review.root;
    this.agentsView = new AgentsView({
      model: () => this.agentsModel(), now: () => Date.now(), short: () => this.size(0).rows < SHORT_ROWS,
      legend: () => this.legend, freeze: () => this.jobFreeze, color: agentColor,
      click: event => options.canInteract() && plainClick(event), focus: () => { this.focusMain(); options.changed(); },
      act: (action, key) => this.agentAction(action, key), focused,
    });
    const build = options.build, controller = options.buildController;
    if (build && controller) {
      this.buildActs = childActions({
        fleet: build.fleet,
        point: id => { controller.point(id); },
        question: id => { controller.open("question", id); },
        ...this.childDialogs(),
      });
      // A bare session has no board: no Board tab, view or actions.
      const board = build.board;
      if (board) this.boardView = new BoardView({
        tasks: () => board.tasks(), proposed: () => board.proposed(),
        unavailable: () => boardUnavailable(options.snapshot()?.build),
        now: () => Date.now(), short: () => this.size(0).rows < SHORT_ROWS,
        click: event => options.canInteract() && plainClick(event), focus: () => { this.focusMain(); options.changed(); },
        act: (action, id) => this.boardAction(action, id), focused,
      });
    }
  }
  /** A build session's `since session` base is the commit the session started from, while Changes reads its worktree. */
  private buildBases(persisted: ReviewBases | undefined): ReviewBases {
    const local = new Map<string, string>();
    return {
      get: root => {
        const state = this.options.snapshot()?.build, base = state?.session.base;
        if (state && base && /^[a-f0-9]{40,64}$/.test(base) && this.realRoot(root) === this.realRoot(state.session.worktree)) return base;
        return persisted ? persisted.get(root) : local.get(root);
      },
      set: (root, head) => { if (persisted) persisted.set(root, head); else local.set(root, head); },
    };
  }
  /** History's list popup, then its read-only pagers; the footer lists the overlay's keys while any of them is open. */
  async openHistory(): Promise<void> {
    const ui = this.ui;
    if (ui) await this.inPager(() => this.history.open(ui));
  }
  /** Where an overlay list (History) sits on the actual terminal, as the F1 and F2 popups do. */
  popupPlace(): OverlayOptions { return popupPlacement(this.options.terminal?.()); }
  bind(ui: ExtensionUIContext): void {
    this.ui = ui; this.images.bind(); void this.task.refresh();
    // A build session launches in its worktree; should the session record name another directory, Changes follows the record.
    const worktree = this.options.build ? this.options.snapshot()?.build?.session.worktree : undefined;
    if (worktree !== undefined && this.realRoot(worktree) !== this.realRoot(this.review.root)) void this.review.setRepository(worktree);
  }
  invalidateFiles(): void {
    this.task.invalidate();
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
  /** Rows above an expanded image: the header only when short, since the tab row hides while the image is open. */
  private chromeRows(width: number): number {
    return this.compact(width).short ? 1 : 2;
  }
  setView(view: MainView): void {
    if (view === "board" && !this.boardView) return;
    if (this.view === "agent" && view !== "agent") { this.conversation?.remember(); this.leaveChild(); }
    if (view === "conversation") this.watch?.back();
    this.images.close();
    this.view = view; this.focused = view !== "conversation";
    if (this.focused) this.options.onFocusMain?.();
    clearInterval(this.timer);
    this.timer = undefined;
    if (view !== "conversation") {
      if (view === "changes") void this.review.refresh();
      if (view === "task") void this.task.refresh(true);
      if (view === "evidence") void this.task.refresh();
      if (view === "artifacts") void this.artifacts.refresh();
      let ticks = 0;
      this.timer = setInterval(() => {
        ticks++;
        if (this.view === "changes" && ticks % 2 === 0) void this.review.refresh();
        if ((this.view === "evidence" && ticks % 10 === 0) || (this.view === "task" && ticks % 15 === 0)) void this.task.refresh();
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
    if (this.view === "agent") { this.conversation?.remember(); if (key !== this.agentKey) this.leaveChild(); }
    this.agentsView.select(key); this.agentKey = key;
    // Another agent (or the same one, read again) starts its new-rows count from what it shows now.
    this.conversation?.restore(key); this.childWatch?.rewrapped();
    this.setView("agent");
  }
  /**
   * A build child's conversation; a child that can take a message becomes the composer's receiver, and a notice
   * says so. Leaving the conversation for any other view gives the message back to the parent (`leaveChild`).
   */
  openChild(id: string): void {
    const build = this.options.build, child = build?.fleet.snapshot().children.find(entry => entry.id === id);
    this.openAgent(childKey(id));
    if (!build || !child || !steerable(child) || build.target() === id) return;
    build.setTarget(id);
    if (build.target() === id) this.notice(`Messages now go to ${plainLabel(id)}`);
  }
  /** The open child conversation is left: while that child receives the message, the parent receives it again. */
  private leaveChild(): void {
    const build = this.options.build, id = this.agentKey === undefined ? undefined : childIdOf(this.agentKey);
    if (!build || id === undefined || build.target() !== id) return;
    if (this.options.buildController) this.options.buildController.reset(); else build.setTarget("parent");
  }
  /** F9: Chat, and in a build session the parent receives the message again. */
  chat(): void {
    this.setView("conversation");
    this.options.buildController?.reset();
  }
  /** A tab click: the same as the tab's key. */
  openTab(id: TabId): void {
    if (id === "agents") this.openAgents(); else if (id === "conversation") this.chat(); else this.setView(id);
  }
  /**
   * Panel feedback for NOTICE_MS, in header row 1's state place (or the panels' toast when the header could not
   * draw it), and kept with its time in `recentNotices()`. `detail` is the exact record behind the sentence (a stack,
   * exit text, a receipt): the header and toast show only `text`, Recent notices shows both.
   */
  notice(text: string, tone: Notice["tone"] = "text", detail?: string): void {
    clearTimeout(this.noticeTimer);
    const notice: Notice = { text: plainLabel(text), tone };
    this.shownNotice = notice; this.noticeDrawn = false;
    this.notices.push({ at: Date.now(), text: notice.text, tone, ...(detail ? { detail } : {}) });
    if (this.notices.length > NOTICE_LOG) this.notices.splice(0, this.notices.length - NOTICE_LOG);
    this.noticeTimer = setTimeout(() => this.clearNotice(notice), NOTICE_MS);
    this.noticeTimer.unref();
    this.options.changed();
  }
  /** Takes `notice` off the screen before its time, if it is still the one shown; the log keeps it. */
  clearNotice(notice: Notice): void {
    if (this.shownNotice !== notice) return;
    clearTimeout(this.noticeTimer); this.noticeTimer = undefined;
    this.shownNotice = undefined; this.noticeDrawn = false;
    this.options.changed();
  }
  private get childActs(): ChildActs | undefined {
    if (this.buildActs) return this.buildActs;
    const fleet = this.options.fleet?.();
    if (!fleet) return undefined;
    if (this.fleetActs?.fleet === fleet) return this.fleetActs.acts;
    const dialogs = this.childDialogs();
    const cards = this.options.childCards;
    const acts = childActions({ fleet, ...childPrompts({ fleet, input: title => this.ui ? this.ui.input(title) : Promise.resolve(undefined), ...dialogs,
      ...(cards ? { cards } : {}) }), ...dialogs });
    this.fleetActs = { fleet, acts };
    return acts;
  }
  private childDialogs() {
    return {
      confirm: (title: string, message: string) => this.ui ? this.ui.confirm(title, message) : Promise.resolve(false),
      select: (title: string, choices: readonly string[]) => this.ui ? this.ui.select(title, [...choices]) : Promise.resolve(undefined),
      notice: (text: string, tone: "text" | "success" | "warning" | "error") => this.notice(text, tone),
    };
  }
  agentsModel(): AgentsModel {
    const snapshot = this.options.snapshot(), now = Date.now();
    const extra = fleetExtras(shownFleet(snapshot), now, !!this.options.build || !!this.options.fleet?.());
    return agentsModel(snapshot, now, { jobControl: !!this.options.jobControl, ...(extra ? { extra } : {}) });
  }
  private agentRow(key: string | undefined): AgentRow | undefined {
    return key === undefined ? undefined : this.agentsModel().agents.find(row => row.key === key);
  }
  private agentAction(action: AgentsAction, key: string): void {
    const child = this.childActs ? childIdOf(key) : undefined;
    if (action === "open") { if (child !== undefined) this.openChild(child); else this.openAgent(key); return; }
    if (action === "transcript") { void this.openAgentTranscript(key); return; }
    if (this.childActs && child !== undefined && (action === "steer" || action === "cancel" || action === "model" || action === "answer")) {
      this.childActs[action](key); return;
    }
    const job = this.agentsModel().jobs.find(row => row.key === key)?.job;
    if (job === undefined) return;
    if (action === "output" || action === "search") void this.openJobOutput(job, action === "search");
    else if (action === "follow") this.toggleFollow(job);
    else if (action === "peek" || action === "background" || action === "stop") void this.controlJob(job, action);
  }
  private boardAction(action: BoardAction, id: string | undefined): void {
    const build = this.options.build, controller = this.options.buildController;
    if (!build || !controller) return;
    if (action === "review") { controller.open("plan"); return; }
    if (action === "plan") { controller.planDraft(); return; }
    if (action === "refresh") { this.refreshBoard(); return; }
    const task = build.board?.tasks().find(item => item.id === id), ui = this.ui;
    if (!task || !ui || !this.options.canInteract()) return;
    void openOutputPager(ui, `${plainLabel(task.id)} · ${plainLabel(task.title)}`, boardDetailLines(task, Date.now()));
  }
  /** Rereads the board status this session filed; a failed read is an error notice. A bare session has no board. */
  refreshBoard(): void {
    const board = this.options.build?.board;
    if (!board) return;
    board.refresh().then(() => this.options.changed(),
      error => this.notice(`Board status unavailable: ${plainLabel(error instanceof Error ? error.message : String(error))}`, "error"));
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
    if (!job?.detail || !snapshot?.bound) { this.notice("Command output unavailable from this host", "warning"); return; }
    const lines = pagerLines(job);
    let start = lines.length;
    if (search) {
      const query = await ui.input("Search retained command output");
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
    const target = { job, session };
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
  private headerInput(): HeaderInput {
    const snapshot = this.options.snapshot(), controller = this.options.buildController;
    if (this.options.build && controller && snapshot?.build) return this.buildHeader(snapshot, snapshot.build, controller);
    const identity = this.options.identity, r = this.review;
    const state: HeaderInput["state"] = !snapshot ? "Unavailable" : snapshot.streaming ? "Working" : "Idle";
    const head = taskHeadline(identity, this.task);
    const elsewhere = r.scopeRequired || r.root !== this.launchRoot;
    const context = head ? { left: `${head.id} · ${head.title}`, right: head.chips.join(" · ") } : {
      left: `${identity.project || "workspace"} · ${basename(this.options.cwd)}${!elsewhere && r.branch !== "unknown" ? ` (${r.branch})` : ""}`,
      ...(elsewhere ? { right: `Changes: ${this.scopeLabel}` } : {}),
    };
    const mark = this.options.mark?.();
    return {
      title: target(identity), state, context,
      ...(this.shownNotice ? { notice: this.shownNotice } : {}),
      ...(mark ? { mark } : {}),
    };
  }
  /**
   * A build session's header: `build · <name>`, the `▲ n needs you` chip, the exit status in place of the
   * state word, and row 2 with the project, the worktree and the exit policy.
   */
  private buildHeader(snapshot: PanelSnapshot, build: BuildPanelState, controller: BuildController): HeaderInput {
    const session = build.session, r = this.review, mark = this.options.mark?.();
    const live = build.fleet.children.some(child => LIVE_CHILD_STATES.includes(child.state));
    const asking = controller.questions().length;
    const docked = controller.dialog()?.key.startsWith("exit:") ?? false;
    const status = exitStatus(build.exit.phase, docked);
    const place = session.here ? `checkout (${plainLabel(session.branch ?? "detached")})` : plainLabel(relative(session.repoRoot, session.worktree) || ".");
    const policy = session.bare ? barePolicy(session) : POLICY[build.settings.exitPolicy];
    const elsewhere = r.scopeRequired || this.realRoot(r.root) !== this.realRoot(session.worktree);
    const scope = elsewhere ? `Changes: ${this.scopeLabel} · ` : "";
    return {
      title: `build · ${plainLabel(build.record.title)}`, state: snapshot.streaming || live ? "Working" : "Idle",
      context: { left: `${plainLabel(session.project ?? basename(session.repoRoot))} · ${place}`, right: scope + policy.long, rightShort: scope + policy.short },
      ...(asking ? { attention: { text: `▲ ${asking} needs you`, short: `▲ ${asking}`, tone: "warning" as const } } : {}),
      ...(status ? { status } : {}),
      ...(this.shownNotice ? { notice: this.shownNotice } : {}),
      // Children at work while the parent is idle still spin the state word, without a turn time.
      ...(mark ? { mark } : live ? { mark: { glyph: PANEL_BLUE + spinner(Date.now()) } } : {}),
    };
  }
  /**
   * The tab row's specs: count badges, the unread mark on Chat while the transcript grew behind another view, and
   * the open sub-view's place on its tab (`Task › Events`), which gives way to the tab's own label when tight.
   */
  private tabSpecs(): TabSpec[] {
    const r = this.review, data = this.task.current, gate = data && gateCheck(data), review = data && reviewChecks(data)[0];
    const live = liveCount(this.agentsModel());
    const badge = (id: TabId): TabSpec["badge"] => {
      switch (id) {
        case "task": case "conversation": return undefined;
        case "agents": return live ? { text: String(live), tone: "accent" } : undefined;
        case "changes": return changesBadge({ checked: r.checkedAt !== undefined, files: r.files.length, scopeRequired: r.scopeRequired, error: r.error !== undefined });
        case "evidence": return proofBadge({ ...(gate ? { gate } : {}), ...(review ? { review } : {}), stale: this.task.stale, error: this.task.error !== undefined });
        case "board": {
          const board = this.options.build?.board, tasks = board?.tasks() ?? [];
          return boardBadge({ open: tasks.filter(task => task.phase !== "landed").length,
            alert: tasks.some(task => task.phase === "blocked" || task.phase === "failed"), proposed: board?.proposed() !== undefined });
        }
      }
    };
    const unread = this.view !== "conversation" && (this.watch?.unread() ?? false), place = this.placeOf(this.view), open = this.tabOf(this.view);
    return this.tabs().map(id => {
      const spec = tabSpec(id, badge(id), id === "conversation" && unread);
      return place && id === open ? { ...spec, label: place, short: spec.label } : spec;
    });
  }
  /** The tab a view highlights; Events belongs to Task in a task-bound session and to Chat otherwise. */
  private tabOf(view: MainView): TabId {
    return view === "timeline" && !this.options.identity.task ? "conversation" : TAB_OF[view];
  }
  private placeOf(view: MainView): string | undefined {
    return view === "timeline" && !this.options.identity.task ? "Chat › Events" : PLACE[view];
  }
  /** The main tabs of this role; a build session without a board (bare) has no Board tab. */
  private tabs(): readonly TabId[] {
    return mainTabs(this.options.identity).filter(id => id !== "board" || this.boardView !== undefined);
  }
  private chrome(): { readonly header: Component; readonly tabs: Component } {
    let hits: readonly TabHit[] = [];
    let headerHits: readonly HeaderHit[] = [];
    const header: Component = {
      render: width => {
        // Renders before the transcript, so a paused reader keeps the same passage at a new width.
        const main = this.mainWidth(width);
        if (this.watchedWidth && this.watchedWidth !== main) { this.watch?.rewrapped(); this.childWatch?.rewrapped(); }
        this.watchedWidth = main;
        this.anchorTranscript(main);
        this.options.live?.(liveCount(this.agentsModel()) > 0);
        // Pi's jump-to-latest control carries the count of rows that arrived below a paused reader.
        this.chatTui ??= this.chatPresentation?.().tui;
        if (this.chatTui && this.watch) labelJumpControl(this.chatTui, this.jumpText);
        const frame = headerFrame(this.headerInput(), width, this.compact(width));
        headerHits = frame.hits; this.headerRows = frame.lines.length; this.noticeDrawn = frame.noticeShown;
        return frame.lines;
      },
      // The attention chip docks the oldest build question; a pending exit status reopens its choice.
      handleMouse: event => {
        if (!this.options.canInteract() || !plainClick(event)) return undefined;
        const hit = headerHits.find(item => item.row === event.y && event.x >= item.from && event.x < item.to);
        const controller = this.options.buildController;
        if (hit && controller) controller.open(hit.id === "attention" ? "question" : "exit");
        else return undefined;
        return { handled: true };
      }, invalidate() {},
    };
    const tabs: Component = {
      render: width => {
        // The open tab's key cap is accent only while its view has the keys (Chat: while the composer has them).
        const row = tabRow(this.tabSpecs(), this.tabOf(this.view), width, this.view === "conversation" || this.focused);
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
  /** The scroll view whose end the jump control and End reach: Chat's transcript, or the open agent conversation. */
  private activeWatch(): TranscriptWatch | undefined {
    if (this.images.isOpen) return undefined;
    return this.view === "conversation" ? this.watch : this.view === "agent" ? this.childWatch : undefined;
  }
  /** End jumps while the draft is empty, and in an agent conversation that has the keys; elsewhere it is the editor's. */
  private endJumps(): boolean { return (this.view === "agent" && this.focused) || !this.ui?.getEditorText(); }
  /** The jump control's words: the rows that arrived below the reader of the open conversation, and the key that jumps. */
  private readonly jumpText = (): string => {
    const bottom = piKey("tui.altScreen.bottom");
    return jumpLabel(this.activeWatch()?.newRows ?? 0, this.endJumps() ? "End" : bottom && keyLabel(bottom));
  };
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
    if (transcript instanceof ScrollView) { this.transcript = transcript; this.watch = new TranscriptWatch(transcript, this.options.modal); }
    const terminal = this.options.terminal;
    this.changes = new ChangesView({ model: () => this.changesModel(), click: event => this.options.canInteract() && plainClick(event),
      focus: () => { this.focusMain(); this.options.changed(); }, step: delta => void this.selectRelative(delta),
      select: path => void this.review.select(path).then(() => { this.acceptPatch(); this.changesScroll?.scrollToStart(); }),
      pickHunk: index => { this.hunk = index; this.options.changed(); }, color: agentColor,
      ...(this.options.build ? { authors: () => this.changeAuthors() } : {}),
      ...(terminal ? { rows: () => this.size(0).rows } : {}), focused: () => this.focused });
    this.changesScroll = this.changes.diff;
    const chat = new HelperChat(() => this.chatPresentation?.().tui, this.options.cwd, presentation);
    chat.expandTools(this.toolsExpanded);
    this.conversation = new AgentConversation({ row: () => this.agentRow(this.agentKey), chat, color: agentColor, now: () => Date.now(),
      click: event => this.options.canInteract() && plainClick(event), focus: () => { this.focusMain(); this.options.changed(); },
      back: () => this.openAgents(), answer: (key: string) => this.agentAction("answer", key) });
    if (this.agentKey !== undefined) this.conversation.restore(this.agentKey);
    this.childWatch = new TranscriptWatch(this.conversation.scroll, this.options.modal);
    // Task, Proof, lifecycle, Usage and Artifacts draw from their first column: `inset` puts them one cell in, as the frame is.
    this.evidenceScroll = new ScrollView(inset({ render: width => this.renderEvidence(width), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      const index = this.criterionHits.get(event.y); if (index !== undefined) this.criterionIndex = index;
      this.focusMain(); this.options.changed(); return { handled: true };
    }, invalidate() {} }), { primary: true, follow: "none", scrollbar: "auto" });
    // Sub-views: a click on the back line (`‹ Task`, `‹ Proof`, `‹ Chat`) goes back as Esc does.
    this.lifecycleScroll = new ScrollView(inset({ render: width => this.lifecycle.render(width), handleMouse: event => {
      const back = this.lifecycle.back;
      if (this.options.canInteract() && plainClick(event) && event.y === back.row && event.x >= back.from && event.x < back.to) { this.back(); return { handled: true }; }
      const result = this.lifecycle.handleMouse(event);
      if (result?.handled) { this.focusMain(); this.options.changed(); }
      return result;
    }, invalidate() {} }), { primary: true, follow: "none", scrollbar: "auto" });
    this.usageScroll = new ScrollView(inset({ render: width => {
      const snapshot = this.options.snapshot(), id = this.options.identity, build = snapshot?.build?.session.name;
      const identity: UsageIdentity = { role: id.role, project: id.project, ...(id.task ? { task: id.task } : {}), ...(id.run ? { run: id.run } : {}),
        ...(id.assistant ? { assistant: id.assistant } : {}), ...(build ? { build } : {}) };
      return usageLines(usageModel({ identity, snapshot }), width,
        { legend: this.legend, bodyRows: this.usageScroll?.viewportHeight ?? 0 }).map(row => row.text);
    }, handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      if (usageBackAt(event.x, event.y)) this.back(); else { this.focusMain(); this.options.changed(); }
      return { handled: true };
    }, invalidate() {} }), { primary: true, follow: "none", scrollbar: "auto" });
    this.artifactScroll = new ScrollView(inset({ render: width => this.artifacts.render(width), handleMouse: event => {
      if (this.options.canInteract() && plainClick(event) && this.artifacts.backAt(event.x, event.y)) { this.back(); return { handled: true }; }
      const result = this.artifacts.handleMouse(event);
      if (result?.handled) { this.focusMain(); this.options.changed(); }
      return result;
    }, invalidate() {} }), { primary: true, follow: "none", scrollbar: "auto" });
    this.taskScroll = new ScrollView(inset({ render: width => taskLines(this.options.identity, this.task, width, { now: Date.now(), legend: this.legend }), handleMouse: event => {
      if (!this.options.canInteract() || !plainClick(event)) return undefined;
      this.focusMain(); this.options.changed(); return { handled: true };
    }, invalidate() {} }), { primary: true, follow: "none", scrollbar: "auto" });
    // An empty Chat says who receives the message and how to send, find the keys and quit; it goes with the first row.
    const send = piKey("tui.input.submit"), quit = piKey("app.clear");
    const emptyChat: Component = { render: width => emptyChatLines({ identity: this.options.identity, ...(send ? { send: keyLabel(send) } : {}),
      ...(quit ? { quit: keyLabel(quit) } : {}) }, width), invalidate() {} };
    const chrome = this.chrome();
    const views = new class extends VStack {
      // Laid out by height only: the rail's row beside it never needs this stack's natural height.
      override render(): string[] { return []; }
    }([
      { component: this.taskScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "task" && !this.images.isOpen },
      { component: emptyChat, basis: "auto", shrink: 0,
        visible: viewport => this.view === "conversation" && !this.images.isOpen && (this.watch?.empty(viewport.width) ?? false) },
      { component: transcript, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "conversation" && !this.images.isOpen },
      { component: this.images, basis: 0, grow: 1, minSize: 1, visible: viewport => {
        this.images.setViewportHeight(Math.max(0, viewport.height - this.chromeRows(viewport.width))); return this.images.isOpen;
      } },
      { component: this.changes.root, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "changes" },
      { component: this.agentsView.scroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agents" },
      { component: this.conversation.scroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "agent" },
      { component: this.evidenceScroll, basis: 0, grow: 1, minSize: 1,
        // The split follows the main view's width; the inset only narrows the detail column.
        visible: viewport => { this.proofWide = this.mainWidth(viewport.width) >= SPLIT_MIN; return this.view === "evidence"; } },
      { component: this.usageScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "usage" },
      { component: this.lifecycleScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "timeline" },
      { component: this.artifactScroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "artifacts" && !this.images.isOpen },
      ...(this.boardView ? [{ component: this.boardView.scroll, basis: 0, grow: 1, minSize: 1, visible: () => this.view === "board" }] : []),
    ]);
    const rail = this.options.rail;
    // The rail fills the rows between the tab rule and the dock; header, tabs and rule span the full width.
    const body = rail ? new HStack([
      { component: views, basis: 0, grow: 1, shrink: 1, minSize: 1 },
      { component: railColumn(rail, () => this.bodyTop(),
        () => { const rows = this.size(0).rows; return Number.isFinite(rows) ? rows - 12 : 1; }),
        basis: "auto", shrink: 0, visible: () => rail.width() !== undefined },
    ]) : views;
    const tabRule: Component = { render: width => [rule(width)], invalidate() {} };
    return new VStack([
      { component: chrome.header, basis: "auto", shrink: 0 },
      // An expanded image takes the tab row's place; Esc or F9 brings both back.
      { component: chrome.tabs, basis: 1, shrink: 0, visible: () => !this.images.isOpen },
      { component: tabRule, basis: 1, shrink: 0, visible: () => this.ruled() },
      { component: body, basis: 0, grow: 1, minSize: 1 },
    ]);
  }
  /** True while the rule under the tab row shows: the tab row shows and the terminal has RULE_ROWS rows or more. */
  private ruled(): boolean { return !this.images.isOpen && this.size(0).rows >= RULE_ROWS; }
  /** The first screen row of the main view: below the header, the tab row and its rule. */
  bodyTop(): number { return this.headerRows + (this.images.isOpen ? 0 : 1) + (this.ruled() ? 1 : 0); }
  /** Columns of the main view at a frame `width`: less the rail and its `│` while the rail shows. */
  private mainWidth(width: number): number {
    const rail = this.options.rail?.width();
    return rail === undefined ? width : Math.max(1, width - rail - 1);
  }
  private renderEvidence(width: number): string[] {
    this.criterionIndex = Math.max(0, Math.min(this.criterionIndex, this.proofRows() - 1));
    const frame = renderProof(this.task, this.options.identity.run, this.criterionIndex, width, this.legend, this.proofOpen, Date.now(), this.proofWide, this.focused);
    this.criterionHits = frame.hits;
    return frame.lines;
  }
  /** Selectable Proof sections (list rows when wide, picker steps when narrow). */
  private proofRows(): number {
    return Math.max(1, proofItems(this.task.current, this.options.identity.run).length);
  }
  private proofEnter(): void {
    const item = proofItems(this.task.current, this.options.identity.run)[this.criterionIndex];
    if (item === undefined) return;
    if (!this.proofOpen.delete(item)) this.proofOpen.add(item);
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
      // Read-only bounded text, never an SDK switchSession/open/resume call; the pager replaces control characters.
      await this.inPager(async () => {
        await openPager(ui, { title: `Transcript · ${plainLabel(row.name)} · read-only`, lines: text.slice(0, 32 * 1024).split("\n"), wrap: true });
      });
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
      head: this.displayedHead, base: this.displayedBase, file: this.displayedFile }, this.scopeLabel, this.hunk, Date.now(), this.changeAuthors(), this.changes?.filter ?? "all");
    return this.shownChanges;
  }
  /** Build children that edited files, only while Changes reads the session worktree; any other scope has no authors. */
  private changeAuthors(): ChangeAuthor[] {
    const state = this.options.snapshot()?.build;
    if (!this.options.build || !state || this.review.scopeRequired || this.realRoot(this.review.root) !== this.realRoot(state.session.worktree)) return [];
    return childAuthors(state.fleet);
  }
  private realRoot(path: string): string {
    let real = this.realRoots.get(path);
    if (real === undefined) {
      try { real = realpathSync(path); } catch { real = resolve(path); }
      this.realRoots.set(path, real);
    }
    return real;
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
  /** The view one step back: an agent's conversation returns to Agents, Events to Task (Chat without a task), Artifacts to Proof, the rest to Chat. */
  private back(): void {
    if (this.view === "agent") this.openAgents();
    else if (this.view === "timeline" && this.options.identity.task) this.setView("task");
    else if (this.view === "artifacts") this.setView("evidence");
    else this.setView("conversation");
  }
  /** The footer and F1 context of the open view while it has the focus. */
  mainContext(): KeyContext { return this.images.isOpen ? "image" : CONTEXT_OF[this.view]; }
  /** Where the open view is, as the footer's lead and the tab label name it: `Changes`, `Task › Events`, `Agents › alpha`. */
  private place(): string {
    const tab = tabSpec(this.tabOf(this.view)).label;
    if (this.view === "agent") { const row = this.agentRow(this.agentKey); return row ? `${tab} › ${row.name}` : tab; }
    return this.placeOf(this.view) ?? tab;
  }
  /**
   * What the footer hints for `context`; the panels own the focus, the queue and the owner-question dock
   * (`questions` waiting). A view that has the keys leads its footer with its name.
   */
  hintState(context: KeyContext, queue: HintState["queue"], questions = 0): HintState {
    const identity = this.options.identity, build = this.options.build, controller = this.options.buildController;
    const selection = context === "agents" || context === "agent" ? this.agentsView.actions() : context === "board" ? this.boardView?.actions() : undefined;
    const target = controller?.target();
    const lead = this.focused && !this.images.isOpen && this.view !== "conversation" && context === CONTEXT_OF[this.view] ? this.place() : undefined;
    return {
      context, streaming: this.options.snapshot()?.streaming ?? false, queue, chat: this.view === "conversation" && !this.images.isOpen,
      mode: { taskBound: !!identity.task, lifecycle: !!identity.task },
      ...(selection ? { selection } : {}), ...(questions > 0 ? { questions } : {}), ...(lead !== undefined ? { lead } : {}),
      authors: (this.shownChanges?.authors.length ?? 0) > 0, legend: this.legend,
      ...(build && controller ? { build: {
        ...(target ? { target } : {}), steerable: build.fleet.snapshot().children.some(steerable),
        proposed: build.board?.proposed() !== undefined, choices: controller.dialog()?.options.length ?? 0,
        exit: controller.dialog()?.key.startsWith("exit:") ?? false,
      } } : {}),
    };
  }
  /** The sidebar's inputs: the agents view's rows, the Changes file list and the plan. */
  sideSource(): SideSource {
    const snapshot = this.options.snapshot(), model = this.agentsModel(), r = this.review, authors = this.changeAuthors();
    return {
      agents: model.sources.helpers || this.options.build || model.agents.length > 0 ? model.agents.map(row => ({
        key: row.key, name: row.name, hue: row.hue, profile: row.profile, glyph: row.status.glyph, tone: row.status.tone, activity: row.activity,
        settled: isLive(row.state) ? undefined : row.state === "done" ? "ok" as const : "failed" as const,
      })) : null,
      jobs: model.sources.jobs ? visibleCommands(model.jobs).map(row => ({
        key: row.key, command: row.command, glyph: row.status.glyph, tone: row.status.tone, time: statusLabel(row.status),
        settled: row.live ? undefined : row.status.tone === "success" ? "ok" as const : "failed" as const,
      })) : null,
      files: {
        scope: this.scopeLabel,
        state: r.scopeRequired ? "choose" : r.error !== undefined ? "error" : r.checkedAt === undefined ? "loading" : "ok",
        ...(r.error !== undefined ? { error: r.error } : {}),
        list: r.visible.map(file => {
          const stat = r.stats.get(file.path), who = authors.filter(author => author.paths.has(file.path));
          // Flagged when an author edited it outside its own files, or when two authors edited it.
          const flag = who.length ? { who: who.map(author => author.hue), outside: who.length > 1 || who.some(author => author.paths.get(file.path) === true) } : {};
          return { path: file.path, letter: LETTERS[fileStatus(file)] ?? "M", added: stat?.added, removed: stat?.removed, ...flag };
        }),
      },
      plan: { progress: snapshot?.ledger ?? null, stale: snapshot?.ledgerStale ?? false,
        ...(snapshot?.ledgerContinued !== undefined ? { continued: snapshot.ledgerContinued } : {}) },
      bound: snapshot?.bound ?? false,
      color: agentColor,
      ...(this.focused && this.selectedAgent() ? { current: this.selectedAgent()!.key } : {}),
    };
  }
  /** The agent the Agents view selects, or whose conversation is open; undefined elsewhere or on a job row. */
  selectedAgent(): AgentRow | undefined {
    if (this.view === "agent") return this.agentRow(this.agentKey);
    if (this.view !== "agents") return undefined;
    const entry = this.agentsView.entry();
    return entry?.kind === "agent" ? this.agentRow(entry.row.key) : undefined;
  }
  /** The selected agent's model, call id and session file, in the read-only pager; false when there is nothing to show. */
  async openAgentDetails(): Promise<boolean> {
    const ui = this.ui, row = this.selectedAgent();
    if (!ui || !row?.meta.length) return false;
    await this.inPager(() => openOutputPager(ui, plainLabel(row.name), row.meta.map(plainLabel), 0));
    return true;
  }
  /** The scroll region of a main view, for page keys. */
  private scrollOf(view: MainView): ScrollView | undefined {
    switch (view) {
      case "task": return this.taskScroll;
      case "usage": return this.usageScroll;
      case "artifacts": return this.artifactScroll;
      case "timeline": return this.lifecycleScroll;
      case "agent": return this.conversation?.scroll;
      case "agents": return this.agentsView.scroll;
      case "changes": return this.changesScroll;
      case "evidence": return this.evidenceScroll;
      case "board": return this.boardView?.scroll;
      case "conversation": return undefined;
    }
  }
  /** PgUp/PgDn while a docked dialog holds the keys: the open view scrolls, Chat and an expanded image leave them to Pi. */
  scrollPage(data: string): boolean {
    const scroll = this.images.isOpen ? undefined : this.scrollOf(this.view);
    if (!scroll) return false;
    scroll.scrollBy(matchesKey(data, "pageUp") ? -12 : 12); this.options.changed();
    return true;
  }
  /**
   * Keys for the main area. View keys (F-keys and Alt+1..Alt+6 in tab order) work from anywhere; End jumps a paused
   * Chat to its latest row while the draft is empty. A view that has the keys runs only the keys it offers now:
   * Esc goes back, Tab gives the keys to the composer, Enter without an action does nothing, `/` always starts a
   * command in the composer, and any other typing (a letter the view does not offer included) goes to the composer.
   */
  handleInput(data: string): boolean {
    const tabs = this.tabs();
    const alt = tabs.findIndex((_, index) => { const key = viewKey(index); return key !== undefined && matchesKey(data, key); });
    if (alt >= 0) { this.openTab(tabs[alt]!); return true; }
    if (matchesKey(data, "f9")) { this.chat(); return true; }
    if (matchesKey(data, "f5") && tabs.includes("task")) { this.setView("task"); return true; }
    if (matchesKey(data, "f7")) { this.setView("changes"); return true; }
    if (matchesKey(data, "f8")) { this.openAgents(); return true; }
    if (matchesKey(data, "f10") && tabs.includes("evidence")) { this.setView("evidence"); return true; }
    if (matchesKey(data, "f10") && tabs.includes("board")) { this.setView("board"); return true; }
    if (this.images.isOpen && matchesKey(data, "escape")) { this.images.close(); return true; }
    // End: the open conversation (Chat or one agent's) back to its latest row, as the jump control says.
    const active = this.activeWatch();
    if (active && !active.following && matchesKey(data, "end") && this.endJumps()) { active.jump(); this.options.changed(); return true; }
    if (!this.focused || this.view === "conversation") return false;
    if (matchesKey(data, "escape")) { this.back(); return true; }
    if (matchesKey(data, "tab")) { this.focused = false; this.options.changed(); return true; }
    if (data === "/") { this.focused = false; this.options.changed(); return false; }
    const view = this.view;
    if (view === "task" && tabs.includes("task") && data === "l") { this.setView("timeline"); return true; }
    if (view === "timeline" && this.lifecycle.handleInput(data)) { this.lifecycleScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (view === "agents" && this.agentsView.handleKey(data)) { this.options.changed(); return true; }
    if (view === "board" && this.boardView?.handleKey(data)) { this.options.changed(); return true; }
    if (view === "agent") {
      if (getKeybindings().matches(data, "app.tools.expand")) {
        this.toolsExpanded = !this.toolsExpanded; this.conversation?.expandTools(this.toolsExpanded); this.options.changed(); return true;
      }
      // A build child's own keys: answer, steer, cancel and model, each only while its row offers it.
      const key = this.agentKey, offered = this.agentsView.actions();
      const action = this.childActs && key !== undefined && childIdOf(key) !== undefined ? CHILD_KEYS[data] : undefined;
      if (key !== undefined && action && offered.includes(action)) { this.agentAction(action, key); return true; }
      if (data === "t" && offered.includes("transcript")) { void this.openAgentTranscript(this.agentKey); return true; }
    }
    if (view === "evidence" && data === "a") { this.setView("artifacts"); return true; }
    if (view === "evidence" && matchesKey(data, "enter")) { this.proofEnter(); return true; }
    if (view === "artifacts" && this.artifacts.handleInput(data)) { this.artifactScroll?.scrollTo(0, { disableFollow: true }); return true; }
    if (matchesKey(data, "enter")) return true; // No Enter action here: nothing happens, Tab gives the keys to the composer.
    if ((view === "task" || view === "evidence" || view === "agents" || view === "artifacts" || view === "usage" || view === "timeline")
      && data === "?") { this.legend = !this.legend; this.options.changed(); return true; }
    const scroll = this.scrollOf(view);
    if (matchesKey(data, "up") || matchesKey(data, "down") || matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      // A page is the viewport less one row of overlap, and a line step never exceeds the viewport, so no row is skipped.
      const height = Math.max(1, scroll?.viewportHeight ?? 1), page = Math.max(1, height - 1), line = Math.min(3, height);
      scroll?.scrollBy(matchesKey(data, "up") ? -line : matchesKey(data, "down") ? line : matchesKey(data, "pageUp") ? -page : page); this.options.changed(); return true;
    }
    if ((data === "j" || data === "k") && (view === "changes" || view === "evidence")) {
      if (view === "changes") void this.selectRelative(data === "j" ? 1 : -1);
      else this.criterionIndex = Math.max(0, Math.min(this.proofRows() - 1, this.criterionIndex + (data === "j" ? 1 : -1)));
      this.options.changed(); return true;
    }
    if (view === "evidence" && data === "r") { void this.task.refresh(true); return true; }
    if (view === "task" && data === "r") { void this.task.refresh(true); return true; }
    if (view === "changes") {
      if (matchesKey(data, FILTER_KEY)) { void this.search(); return true; }
      if (data === "o") { void this.repository(); return true; }
      if (data === "c") { const choices = this.review.comparisons; const next = choices[(choices.indexOf(this.review.comparison) + 1) % choices.length]; if (next) { this.lastPatch = ""; void this.review.setComparison(next); } return true; }
      if (data === "r") { this.task.invalidate(); void this.review.refresh().then(() => this.acceptPatch()); return true; }
      if (data === "n" || data === "p") { this.moveHunk(data === "n" ? 1 : -1); return true; }
      if (data === "a") { this.askHunk(); return true; }
      // Filtering by agent is offered only while the comparison has authors.
      if (data === "f" && (this.shownChanges?.authors.length ?? 0) > 0) { this.changes?.cycleFilter(); this.options.changed(); return true; }
    }
    if (data.startsWith("\x1b")) return false; // Terminal replies, focus events and unbound chords are not typing.
    this.focused = false; this.options.changed(); return false;
  }
  dispose(): void {
    clearInterval(this.timer); clearTimeout(this.debounce); clearTimeout(this.noticeTimer);
    this.chatPresentation = undefined;
    this.history.dispose(); this.scopeAbort.abort(); this.images.dispose(); this.review.dispose(); this.task.dispose(); this.artifacts.dispose(); this.ui = undefined;
  }
}
