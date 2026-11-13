// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, isKeyRelease, matchesKey, TuiAltScreen, visibleWidth, type Component, type OverlayHandle, type OverlayOptions, type Terminal, type TUI, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import type { BuildUi, FleetControl } from "../build/types.ts";
import { PanelTerminal, PANEL_BREAKPOINT, PANEL_WIDTH, RAIL_MIN, RAIL_MAX, MIN_MAIN, type PanelSurface } from "./panel-terminal.ts";
import { NativeWorkbench, boardUnavailable, plainClick, RULE_ROWS, type MainView } from "./workbench.ts";
import type { HistoryRead } from "./history.ts";
import type { ReviewBases } from "./review-store.ts";
import type { JobControl } from "../host/job-controls.ts";
import type { QueueControl } from "../host/queue-control.ts";
import { panelTheme, userLabel } from "./panels-theme.ts";
import { type Appearance, type Density, PACKAGED } from "./appearance.ts";
import { ActionPalette, type HostAction } from "./action-palette.ts";
import { setCanvasAppearance } from "./panel-canvas.ts";
import { agentColor, setAgentHueAppearance } from "./agent-hues.ts";
import { statusBar } from "./panels-chrome.ts";
import { composerBar, composerBottom, composerRow, composerTitle, composerTop, type ComposerHit } from "./composer-chrome.ts";
import { chromeTier, COMPACT_COLUMNS, SHORT_ROWS, type Notice } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, keyLabel, piKey, queueFocusKey, targetKey, type FooterHit, type HintState, type KeyContext } from "./keymap.ts";
import { KeysList } from "./keys-list.ts";
import { rule } from "./frame.ts";
import { QueueDock } from "./queue-dock.ts";
import { Sidebar, type SideHit, type SideTarget } from "./sidebar.ts";
import { inspectorActivity } from "./inspector.ts";
import { openOutputPager } from "./job-detail.ts";
import { pad, plainLabel, setPanelAppearance, PANEL_ERROR, PANEL_RAISED, PANEL_RULE, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, type PanelSnapshot } from "./panels-view.ts";
import { BuildController } from "./build/build-controller.ts";
import { BUILD_BOARD_ENTRY, BUILD_EXIT_ENTRY, BUILD_FINISH_ENTRY, buildEntryRenderer, type BuildEntry } from "./build/build-entries.ts";
import { choiceRows } from "./build/choice-dock.ts";
import { childIdOf, type BuildCardSource } from "./build/contract.ts";
import { steerable } from "./build/fleet-rows.ts";

export interface NativePanelsOptions {
  readonly build?: BuildUi;
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly inspection?: () => Promise<unknown>;
  readonly lifecycle?: () => Promise<unknown>;
  readonly artifacts?: () => Promise<unknown>;
  readonly artifactViewer?: (reference: string, viewer: "show" | "serve") => Promise<string>;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly jobControl?: JobControl;
  readonly queueControl?: QueueControl;
  readonly readTask?: () => Promise<unknown>;
  readonly snapshot: () => PanelSnapshot | null;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
  readonly reviewBases?: ReviewBases;
  /** The host's delivery of composer text to the current child target; false when the child or the session cannot take it. */
  readonly deliverBuildInput?: (text: string) => Promise<boolean>;
}
/** `/panels <target>` names for main views; history, receipts, project, refresh and toggle are actions. */
const PANEL_VIEWS: Readonly<Record<string, MainView>> = {
  chat: "conversation", files: "changes", changes: "changes", agents: "agents", activity: "agents", jobs: "agents", helpers: "agents",
  usage: "usage", evidence: "evidence", proof: "evidence", inbox: "inbox", task: "task", graph: "graph", timeline: "timeline", board: "board",
};
/** Views that read the bound task; sessions without one do not offer them. */
const TASK_VIEWS: Readonly<Partial<Record<MainView, true>>> = { task: true, evidence: true, graph: true, timeline: true };
const PANEL_TARGETS = "chat, agents, changes, task, proof, inbox, usage, graph, timeline, history, receipts, project, refresh or toggle";

/** Where keys go when no Pi dialog or overlay is open: Pi's editor, the main view, the sidebar, the queue or a docked build dialog. */
export type PanelFocus = "composer" | "main" | "sidebar" | "queue" | "choice";

/** The session's build controls, with a steer to the current target delivered through the host, which refuses it once the session closes or unbinds. */
function routedBuild(build: BuildUi, deliver: ((text: string) => Promise<boolean>) | undefined): BuildUi {
  if (!deliver) return build;
  const fleet = build.fleet;
  const routed: FleetControl = {
    snapshot: () => fleet.snapshot(),
    steer: (id, text) => id === build.target() ? deliver(text) : fleet.steer(id, text),
    cancel: id => fleet.cancel(id), setModel: (id, model) => fleet.setModel(id, model), answer: (id, text) => fleet.answer(id, text),
    models: () => fleet.models(), profiles: () => fleet.profiles(), subscribe: listener => fleet.subscribe(listener),
  };
  return { fleet: routed, exit: build.exit, board: build.board, target: () => build.target(), setTarget: target => build.setTarget(target) };
}
/** Session cost with every child's known cost; `partial` when the parent's or a child's cost that ran is unknown. */
function buildCost(snapshot: PanelSnapshot | null): { readonly total: number; readonly partial: boolean } | undefined {
  const children = snapshot?.build?.fleet.children ?? [];
  const priced = children.filter(child => child.usage.cost !== undefined);
  if (!snapshot || (snapshot.cost === undefined && !priced.length)) return undefined;
  return {
    total: (snapshot.cost ?? 0) + priced.reduce((sum, child) => sum + (child.usage.cost ?? 0), 0),
    partial: snapshot.cost === undefined || children.some(child => child.usage.cost === undefined && child.usage.turns > 0),
  };
}

export class NativePanels implements PanelSurface {
  readonly workbench: NativeWorkbench;
  readonly sidebar: Sidebar;
  readonly queue: QueueDock;
  sidebarEnabled = true;
  sidebarWidth = PANEL_WIDTH;
  private drag: { readonly start: number } | undefined;
  private separatorAt = 0;
  private railHits: ReadonlyMap<number, readonly SideHit[]> = new Map();
  /** Screen rows of the rail in the last layout: the first row and how many. */
  private railTop = 0;
  private railRows = 0;
  /** The notice toast at the top right of the main view, while one shows. */
  private toast: { readonly handle: OverlayHandle; readonly notice: Notice } | undefined;
  /** True while `hasOverlay` asks the TUI about every overlay but the toast. */
  private probing = false;
  private tui: TUI | undefined;
  /** Rows of the status bar in the last frame. */
  private statusRows = 1;
  /** The drawer was opened by F6 and closes when the sidebar gives up the focus. */
  private drawerByKey = false;
  /** A dialog or pager the panels opened (Pi raises a prompt for it too): its footer shows its own keys. */
  private overlay: "palette" | "keys" | "pager" | undefined;
  appearance: Appearance = "dark";
  density: Density = "comfortable";
  private nativeAction: ((id: "app.model.select" | "app.thinking.cycle") => void) | undefined;
  private nativeKeys: ((id: "app.model.select" | "app.thinking.cycle") => string) | undefined;
  private terminal: PanelTerminal | undefined;
  private ui: ExtensionUIContext | undefined;
  private requestRender: ((force?: boolean) => void) | undefined;
  private editorFocused: (() => boolean) | undefined;
  private activeSelection: (() => boolean) | undefined;
  private hasOverlay: (() => boolean) | undefined;
  private prompts = 0;
  private pointerOrigin: "main" | "sidebar" | undefined;
  private queued = false;
  private disposed = false;
  /** A build session's docked dialogs, message target and conversation entries; absent in every other role. */
  readonly build: BuildController | undefined;
  private readonly unsubscribeBuild: readonly (() => void)[] = [];
  private pi: ExtensionAPI | undefined;
  /** Entries the controller posted before Pi was installed. */
  private readonly posts: BuildEntry<unknown>[] = [];

  constructor(private readonly options: NativePanelsOptions) {
    this.sidebar = new Sidebar(() => this.refresh());
    const build = options.build ? routedBuild(options.build, options.deliverBuildInput) : undefined;
    this.build = build ? new BuildController({
      ui: build, state: () => this.options.snapshot()?.build, streaming: () => this.options.snapshot()?.streaming ?? false,
      overlay: () => !this.canInteract, draft: () => this.ui?.getEditorText() ?? "", setDraft: text => this.ui?.setEditorText(text),
      focusComposer: () => this.focusComposer(), focusDock: () => this.focusChoice(),
      notice: (text, tone) => this.workbench.notice(text, tone), post: entry => this.post(entry),
      showBoard: () => { this.workbench.setView("board"); }, openChild: id => this.workbench.openChild(id),
      color: agentColor, now: () => Date.now(), changed: () => this.refresh(),
    }) : undefined;
    this.queue = new QueueDock({
      items: () => this.options.snapshot()?.queue ?? [], control: () => this.options.queueControl,
      draft: () => this.ui?.getEditorText() ?? "", setDraft: text => this.ui?.setEditorText(text),
      focusComposer: () => this.focusComposer(), notice: (text, tone) => this.workbench.notice(text, tone),
      changed: () => this.refresh(),
      // A build session's queue names the parent as every item's receiver: children never take queued input.
      ...(build ? { receiver: () => "parent" } : {}),
    });
    this.workbench = new NativeWorkbench({ ...options, ...(build && this.build ? { build, buildController: this.build } : {}),
      terminal: () => this.terminal ? { columns: this.terminal.physical.columns, rows: this.terminal.rows } : undefined,
      onFocusMain: () => { this.sidebar.focused = false; this.queue.focused = false; this.blurDock(); this.closeKeyDrawer(); },
      canInteract: () => this.canInteract && !this.activeSelection?.(), changed: () => this.refresh(),
      rail: {
        width: () => this.wide && this.terminal ? this.terminal.railWidth : undefined,
        render: (width, height) => this.renderSidebar(width, height),
        placed: (top, height) => { this.railTop = top; this.railRows = height; },
      } });
    if (build) {
      const sync = () => { this.build?.sync(); this.refresh(); };
      this.unsubscribeBuild = [build.fleet.subscribe(sync), build.exit.subscribe(sync), build.board.subscribe(sync)];
    }
  }
  mainView(transcript: Component, presentation?: () => InteractiveChatPresentation): Component { return this.workbench.mount(transcript, presentation); }
  wrap(physical: Terminal): PanelTerminal {
    if (this.terminal) throw new Error("native panels already have a terminal");
    this.terminal = new PanelTerminal(physical, this);
    return this.terminal;
  }
  get wide(): boolean { return this.terminal?.sidebarVisible ?? false; }
  get canInteract(): boolean {
    return !this.disposed && this.prompts === 0 && !this.hasOverlay?.() && this.editorFocused?.() !== false;
  }
  /** The logical focus: Pi's editor always holds the TUI focus; the panels route keys by this. */
  get focus(): PanelFocus {
    if (this.build?.dock.focused && this.build.dialog()) return "choice";
    return this.queue.focused ? "queue" : this.sidebar.focused ? "sidebar" : this.workbench.focused ? "main" : "composer";
  }
  layoutChanged(): void { this.refresh(); }
  refresh(): void {
    if (this.queued || this.disposed) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      if (this.disposed) return;
      this.syncToast();
      this.requestRender?.();
    });
  }
  /**
   * The workbench's notice as a toast: raised surface, rounded border in the rule colour, at the top right of the
   * main view. It never takes the focus; a new notice replaces it and it leaves when the notice expires.
   */
  private syncToast(): void {
    const notice = this.workbench.currentNotice, tui = this.tui, terminal = this.terminal;
    if (this.toast?.notice === notice) return;
    this.toast?.handle.hide(); this.toast = undefined;
    if (!notice || !tui || !terminal) return;
    const main = terminal.mainColumns, width = Math.max(6, Math.min(visibleWidth(notice.text) + 4, main - 2));
    const tone = notice.tone === "success" ? PANEL_SUCCESS : notice.tone === "warning" ? PANEL_WARNING : notice.tone === "error" ? PANEL_ERROR : PANEL_TEXT;
    const lines = [
      `${PANEL_RAISED}${PANEL_RULE}╭${"─".repeat(width - 2)}╮`,
      `${PANEL_RAISED}${PANEL_RULE}│${tone} ${pad(notice.text, width - 4)}${PANEL_RAISED} ${PANEL_RULE}│`,
      `${PANEL_RAISED}${PANEL_RULE}╰${"─".repeat(width - 2)}╯`,
    ].map(line => line + "\x1b[0m");
    const handle = tui.showOverlay({ render: () => lines, invalidate() {} }, {
      row: this.workbench.bodyTop(), col: Math.max(0, main - 1 - width), width, nonCapturing: true, visible: () => !this.probing,
    });
    this.toast = { handle, notice };
  }
  dispose(): void {
    this.disposed = true;
    this.toast?.handle.hide(); this.toast = undefined; this.tui = undefined;
    for (const unsubscribe of this.unsubscribeBuild) unsubscribe();
    this.build?.dispose();
    this.workbench.dispose();
    this.ui = undefined; this.pi = undefined;
    this.requestRender = undefined;
    this.editorFocused = undefined;
    this.hasOverlay = undefined; this.activeSelection = undefined;
    this.nativeAction = undefined; this.nativeKeys = undefined; this.updateDensity = undefined;
  }
  async loadFiles(): Promise<void> { await this.workbench.review.refresh(); }
  /** The rail: Context, Files and Plan over the same rows as the agents view and Changes. */
  renderSidebar(width: number, height: number): readonly string[] {
    const frame = this.sidebar.rail(this.workbench.sideSource(), width, height, this.drag ? width : undefined);
    this.railHits = frame.hits;
    return frame.lines;
  }
  private closeKeyDrawer(): void {
    if (!this.drawerByKey) return;
    this.drawerByKey = false; this.sidebar.drawer = false;
  }
  /** Back to Pi's editor: the main view, the sidebar, the queue and a docked dialog give up the focus. */
  focusComposer(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false; this.blurDock();
    this.closeKeyDrawer(); this.refresh();
  }
  private blurDock(): void { if (this.build) this.build.dock.focused = false; }
  /** A docked build dialog takes the keys; it already marked itself focused. */
  private focusChoice(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false;
    this.closeKeyDrawer(); this.refresh();
  }
  /** F6: focus the sidebar (opening the drawer when there is no rail), or the next tab once it has the focus. */
  private sidebarKey(): void {
    if (this.sidebar.focused) this.sidebar.nextTab();
    else {
      this.queue.focused = false; this.workbench.focused = false; this.blurDock(); this.sidebar.focused = true;
      if (!this.wide && !this.sidebar.drawer) { this.sidebar.drawer = true; this.drawerByKey = true; }
    }
    this.refresh();
  }
  /** Alt+↓: the queue takes the focus with the last editable item selected; false when nothing is editable. */
  private focusQueue(): boolean {
    if (!this.queue.focusLast()) return false;
    this.sidebar.focused = false; this.workbench.focused = false; this.blurDock(); this.closeKeyDrawer(); this.refresh();
    return true;
  }
  /** Ctrl+B: where the rail fits it hides or returns; elsewhere the collapsed row opens or closes its drawer. */
  private toggleSidebar(): void {
    if ((this.terminal?.physical.columns ?? 0) >= PANEL_BREAKPOINT) {
      this.sidebarEnabled = !this.sidebarEnabled;
      this.sidebar.drawer = false; this.drawerByKey = false;
      if (!this.sidebarEnabled) this.sidebar.focused = false;
      this.terminal?.refreshLayout();
    } else {
      this.sidebar.drawer = !this.sidebar.drawer; this.drawerByKey = false;
      if (!this.sidebar.drawer) this.sidebar.focused = false;
    }
    this.refresh();
  }
  private openSideTarget(target: SideTarget): void {
    // A build child waiting for an answer docks its question; every other agent opens in the agents view.
    const id = this.build && target.kind === "agent" ? childIdOf(target.key) : undefined;
    const asking = id !== undefined && this.options.snapshot()?.build?.fleet.children.some(child => child.id === id && child.state === "needs");
    if (id !== undefined && asking) this.build?.open("question", id);
    else if (target.kind === "agent" || target.kind === "job") this.workbench.openAgents(target.key);
    else if (target.kind === "file") void this.workbench.openFile(target.path);
    else if (target.kind === "scope") void this.workbench.repository();
    else if (this.options.identity.task) this.workbench.setView("task");
    this.refresh();
  }
  /** A click on a sidebar row: the toggle hides the rail or closes the drawer; tabs, pager and items go to the sidebar. */
  private sideClick(hit: SideHit | undefined, rail: boolean): void {
    if (!hit) return;
    if (hit.toggle && rail) { this.toggleSidebar(); return; }
    if (hit.toggle) {
      this.sidebar.drawer = !this.sidebar.drawer; this.drawerByKey = false;
      if (!this.sidebar.drawer) this.sidebar.focused = false;
      this.refresh(); return;
    }
    const target = this.sidebar.click(hit, this.workbench.sideSource());
    if (target) this.openSideTarget(target);
  }
  /**
   * One widget above the editor, in the mock's dock order: a docked build dialog, then the queue, then (only
   * without a rail and without a dialog) the collapsed sidebar row or its drawer, each with its own rule.
   */
  private dock(): Component {
    let choiceRowsShown = 0, queueRows = 0;
    let sideHits: ReadonlyMap<number, readonly SideHit[]> = new Map();
    return {
      render: width => {
        const rows = this.terminal?.rows ?? 24;
        this.queue.setCapacity(rows);
        const queue = this.queue.render(width);
        let choice: string[] = [];
        if (this.build?.dialog()) {
          this.build.dock.setLimit(choiceRows(rows, this.chromeRows(width, rows, queue.length)), rows < SHORT_ROWS);
          choice = this.build.dock.render(width);
        }
        let side: readonly string[] = [];
        sideHits = new Map();
        if (!this.wide && !choice.length) {
          const source = this.workbench.sideSource();
          const frame = this.sidebar.drawer ? this.sidebar.drawerFrame(source, width, rows)
            : this.sidebar.row(source, width, chromeTier(this.terminal?.physical.columns ?? width) === "tiny", rows);
          side = frame.lines; sideHits = frame.hits;
        }
        choiceRowsShown = choice.length; queueRows = queue.length;
        return [...choice, ...queue, ...side];
      },
      handleMouse: event => {
        if (event.y < choiceRowsShown) return this.canInteract ? this.build?.dock.handleMouse(event) : { handled: true };
        const y = event.y - choiceRowsShown;
        if (y >= queueRows) {
          const row = y - queueRows;
          if (this.canInteract && plainClick(event)) this.sideClick(sideHits.get(row)?.find(hit => event.x >= hit.from && event.x < hit.to), false);
          return { handled: true };
        }
        if (!this.canInteract) return undefined;
        const result = this.queue.handleMouse({ ...event, y });
        if (this.queue.focused) { this.sidebar.focused = false; this.workbench.focused = false; this.blurDock(); this.closeKeyDrawer(); this.refresh(); }
        return result;
      },
      invalidate() {},
    };
  }
  /**
   * Rows the rest of the screen keeps while a dialog is docked: header (one row when short or tiny), tabs and
   * their rule, the composer box (its bar folds away when short), the status bar, the footer and its rule, and
   * the queue.
   */
  private chromeRows(width: number, rows: number, queueRows: number): number {
    const header = rows < SHORT_ROWS || (this.terminal?.physical.columns ?? width) < COMPACT_COLUMNS ? 1 : 2;
    const rules = rows >= RULE_ROWS ? 2 : 0;
    return header + 1 + (rows < SHORT_ROWS ? 3 : 4) + this.statusRows + 1 + rules + queueRows;
  }
  private queueState(): HintState["queue"] {
    const items = this.options.snapshot()?.queue ?? [];
    return { items: items.length, editable: items.filter(item => item.editable).length, control: this.queue.controllable };
  }
  /**
   * Whose keys the footer lists. What the panels opened comes first, since Pi reports it as a prompt as well;
   * then a Pi dialog, then another overlay, then the focused surface.
   */
  private footerContext(): KeyContext {
    if (this.overlay) return this.overlay;
    if (this.workbench.pagerOpen) return "pager";
    if (this.prompts > 0) return "dialog";
    if (this.hasOverlay?.()) return "pager";
    const focus = this.focus;
    return focus === "main" ? this.workbench.mainContext() : focus;
  }
  /** Runs a key as if typed, after the current input or click has been handled. */
  private replay(data: string): void {
    queueMicrotask(() => { if (!this.disposed) this.terminal?.replay(data); });
  }
  handleInput(data: string): boolean {
    if (isKeyRelease(data)) return false;
    const mouse = /^\x1b\[<(\d+);(\d+);(\d+)([Mm])$/.exec(data);
    if (mouse && this.wide && this.terminal) {
      const column = Number(mouse[2]) - 1, row = Number(mouse[3]) - 1, x = column - this.terminal.mainColumns - 1;
      const inRail = row >= this.railTop && row < this.railTop + this.railRows;
      const button = Number(mouse[1]);
      const release = mouse[4] === "m" || (button & 3) === 3;
      // The rail's `│` column (x = -1) resizes the rail: drag, Esc restores, double-click resets.
      if (this.drag) {
        if (release) { this.drag = undefined; this.refresh(); }
        else if (button & 32) this.resizeRail(this.terminal.physical.columns - column - 1);
        return true;
      }
      if (inRail && x === -1 && mouse[4] === "M" && button === 0 && this.canInteract) {
        const now = Date.now();
        if (now - this.separatorAt < 400) { this.separatorAt = 0; this.resizeRail(PANEL_WIDTH); }
        else { this.separatorAt = now; this.drag = { start: this.terminal.railWidth }; this.refresh(); }
        return true;
      }
      if (mouse[4] === "M" && !(button & 96) && !release) this.pointerOrigin = inRail && x >= 0 ? "sidebar" : "main";
      const origin = this.pointerOrigin;
      if (release) this.pointerOrigin = undefined;
      // A main-pane selection must receive its release even when dragged into the rail.
      if (origin === "main") return false;
      if ((inRail && x >= 0) || origin === "sidebar") {
        if (inRail && x >= 0 && this.canInteract && mouse[4] === "M" && button === 0) {
          this.sideClick(this.railHits.get(row - this.railTop)?.find(hit => x >= hit.from && x < hit.to), true);
        }
        return true; // Rail cells are the sidebar's: Pi's views never receive them.
      }
    }
    if (this.drag && matchesKey(data, "escape")) { const start = this.drag.start; this.drag = undefined; this.resizeRail(start); this.refresh(); return true; }
    if (!this.canInteract || mouse) return false;
    // A docked build dialog is modal for keys: page keys scroll the open view and Ctrl+C stays Pi's.
    if (this.focus === "choice") {
      if (this.build?.dock.handleKey(data)) return true;
      if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) return this.workbench.scrollPage(data);
      return !matchesKey(data, "ctrl+c");
    }
    if (matchesKey(data, "f1")) { void this.showKeys(); return true; }
    if (matchesKey(data, "f6")) { this.sidebarKey(); return true; }
    if (matchesKey(data, "ctrl+b")) { this.toggleSidebar(); return true; }
    if (this.wide && (matchesKey(data, "alt+,") || matchesKey(data, "alt+."))) {
      this.resizeRail((this.terminal?.railWidth ?? this.sidebarWidth) + (matchesKey(data, "alt+.") ? 2 : -2)); return true;
    }
    const receiverKey = this.build ? targetKey() : undefined;
    if (receiverKey && matchesKey(data, receiverKey)) { this.build?.cycleTarget(); return true; }
    const focusKey = queueFocusKey();
    if (focusKey && matchesKey(data, focusKey) && this.focusQueue()) return true;
    if (this.queue.focused) {
      if (this.queue.handleKey(data)) return true;
      // A printable key left the queue and reaches the editor; function keys still open views.
      if (!data.startsWith("\x1b")) return false;
    }
    if (this.sidebar.focused) {
      const result = this.sidebar.handleKey(data, this.workbench.sideSource());
      if (result.open) { this.openSideTarget(result.open); return true; }
      if (result.leave) { this.focusComposer(); return true; }
      if (result.handled) return true;
      if (decodeKittyPrintable(data) !== undefined || !data.startsWith("\x1b")) { this.focusComposer(); return false; }
    }
    return this.workbench.handleInput(data);
  }
  /** Clamped to RAIL_MIN–RAIL_MAX and the main pane's MIN_MAIN; Pi re-lays its width. */
  resizeRail(width: number): void {
    const max = Math.max(RAIL_MIN, Math.min(RAIL_MAX, (this.terminal?.physical.columns ?? 0) - MIN_MAIN - 1));
    const next = Math.max(RAIL_MIN, Math.min(max, Math.round(width)));
    if (next === this.sidebarWidth) return;
    this.sidebarWidth = next; this.terminal?.refreshLayout();
  }
  install(pi: ExtensionAPI, sdk: typeof Sdk): void {
    this.themeCtor = sdk.Theme;
    this.pi = pi;
    pi.registerMarkdownTransformer(userLabel, { messageTypes: ["user"] });
    if (this.build) {
      const entries = buildEntryRenderer(() => this.build);
      for (const type of [BUILD_EXIT_ENTRY, BUILD_BOARD_ENTRY, BUILD_FINISH_ENTRY]) pi.registerMessageRenderer(type, entries);
      // The latest `build_finish` call's card shows live exit progress; older cards read as superseded.
      pi.on("tool_execution_start", event => {
        if (event.toolName === "build_finish" && event.parentToolCallId === undefined) this.build?.noteFinish(event.toolCallId);
      });
    }
    pi.on("ui_prompt_start", () => { this.prompts++; this.refresh(); });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); this.refresh(); });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui;
      this.workbench.bind(ctx.ui);
      this.prompts = 0;
      setPanelAppearance(this.appearance); setCanvasAppearance(this.appearance); setAgentHueAppearance(this.appearance);
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode(), this.appearance));
      // The header's state word carries Working; Pi's loader row above the composer would repeat it.
      ctx.ui.setWorkingVisible(false);
      const owner = this;
      ctx.ui.setEditorComponent((tui, theme, keys) => {
        /**
         * The composer: a rounded box with its title set into the top border, `›` before the draft and the model
         * bar inside, in accent while it has the focus. Pi's own top and bottom border rows carry the box's top
         * and the bar; the box's bottom border follows the bar, and the bar folds away in a short terminal.
         */
        const editor = new class extends sdk.CustomEditor {
          private composer: { readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit } | undefined;
          private bottom = "";
          private bottomRow = -1;
          private hiddenAbove = 0;
          dispatchNative(id: "app.model.select" | "app.thinking.cycle"): void { this.actionHandlers.get(id)?.(); }
          /** The margin, `│` and blank before the content, then `› `; compact density drops the blank after `›`. */
          get inset(): number { return owner.density === "compact" ? 4 : 5; }
          setPaddingX(_padding: number): void { super.setPaddingX(this.inset); }
          private get accent(): boolean { return this.focused && owner.focus === "composer"; }
          handleInput(data: string): void {
            if (owner.canInteract && matchesKey(data, "f2")) { void owner.palette(); return; }
            // The dequeue binding pulls the owner's last queued input back above the draft.
            if (keys.getKeys("app.message.dequeue").some(key => matchesKey(data, key)) && owner.editQueued()) return;
            // A child receives Enter; `/` commands always reach Pi, so `/finish` works with any receiver.
            const build = owner.build;
            if (build?.target() && !this.isShowingAutocomplete()) {
              const text = this.getText();
              if (keys.getKeys("tui.input.submit").some(key => matchesKey(data, key)) && text.trim() && !text.trimStart().startsWith("/")) {
                void build.submit(text); return;
              }
              if (keys.getKeys("app.message.followUp").some(key => matchesKey(data, key))) {
                owner.workbench.notice("Only the parent takes queued follow-ups", "warning"); return;
              }
            }
            // A message sent from another view returns to Chat, so its reply and any refusal notice show.
            if (owner.workbench.view !== "conversation" && this.getText().trim()
                && keys.getKeys("tui.input.submit").some(key => matchesKey(data, key))) {
              owner.workbench.setView("conversation"); owner.workbench.focused = false;
            }
            super.handleInput(data);
          }
          protected renderTopBorder(width: number, hidden: number): string {
            this.hiddenAbove = hidden;
            // The receiver stays at every width (it decides where Enter goes); the input disposition goes first.
            const tiny = (owner.terminal?.physical.columns ?? width) < COMPACT_COLUMNS, target = owner.build?.composerTarget();
            return composerTop(composerTitle(tiny ? null : owner.options.snapshot(), target), width, this.accent, hidden ? `↑ ${hidden} more` : "");
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            if ((owner.terminal?.rows ?? SHORT_ROWS) < SHORT_ROWS) {
              this.composer = undefined;
              this.bottom = composerBottom(width, this.accent, hidden ? `↓ ${hidden} more` : "");
            } else {
              this.composer = composerBar(owner.options.snapshot(), width, hidden, this.accent);
              this.bottom = this.composer.line;
            }
            return this.bottom;
          }
          handleMouse(event: TuiMouseEvent) {
            if (event.type === "click" && event.button === "left") {
              owner.focusComposer();
              if (owner.canInteract && this.composer && event.y === this.bottomRow) {
                const hit = (range: { from: number; to: number } | undefined) => range && event.x >= range.from && event.x < range.to;
                // Use the same native actions as Ctrl+L / Shift+Tab; selection still commits through host preflight.
                if (hit(this.composer.model)) { this.actionHandlers.get("app.model.select")?.(); return { handled: true }; }
                if (hit(this.composer.effort)) { this.actionHandlers.get("app.thinking.cycle")?.(); return { handled: true }; }
              }
            }
            // The box's bottom border follows the bar: rows below the bar are one further down than Pi counts them.
            return super.handleMouse(this.composer && this.bottomRow >= 0 && event.y > this.bottomRow ? { ...event, y: event.y - 1 } : event);
          }
          render(width: number): string[] {
            const lines = super.render(width), end = lines.indexOf(this.bottom), accent = this.accent;
            this.bottomRow = this.composer ? end : -1;
            if (end < 0) return lines;
            const rows = lines.slice(1, end).map((line, index) => composerRow(line, this.inset, width, accent, index === 0 && this.hiddenAbove === 0));
            return [lines[0]!, ...rows, lines[end]!, ...(this.composer ? [composerBottom(width, accent)] : []), ...lines.slice(end + 1)];
          }
        }(tui, theme, keys);
        editor.setPaddingX(editor.inset);
        owner.nativeAction = id => editor.dispatchNative(id);
        owner.updateDensity = () => editor.setPaddingX(editor.inset);
        owner.nativeKeys = id => keys.getKeys(id).map(key => keyLabel(key)).join(", ");
        owner.editorFocused = () => editor.focused;
        owner.requestRender = force => tui.requestRender(force);
        owner.tui = tui;
        // The toast never takes the focus, so it does not count as an overlay that holds the keys.
        owner.hasOverlay = () => {
          owner.probing = true;
          try { return tui.hasOverlay(); } finally { owner.probing = false; }
        };
        owner.activeSelection = tui instanceof TuiAltScreen ? () => tui.hasActiveSelection() : () => false;
        return editor;
      });
      ctx.ui.setWidget("corral-dock", () => this.dock(), { placement: "aboveEditor" });
      ctx.ui.setFooter((tui, _theme, data) => {
        const unsubscribe = data.onBranchChange(() => tui.requestRender());
        ctx.ui.setHeader(() => ({
          render: () => [],
          invalidate() {},
        }));
        let footerAt = 0;
        let hits: readonly FooterHit[] = [];
        return {
          // The status bar, a rule (not below RULE_ROWS rows), then one row of the focused surface's keys ending with F1.
          render: width => {
            const snapshot = this.options.snapshot(), cost = this.build ? buildCost(snapshot) : undefined;
            const status = statusBar(this.options.identity, cost && snapshot ? { ...snapshot, cost: cost.total } : snapshot,
              data.getExtensionStatuses(), width, cost?.partial ?? false);
            const context = this.footerContext();
            const row = footerRow(contextHints(this.workbench.hintState(context, this.queueState())),
              chromeTier(this.terminal?.physical.columns ?? width), width, footerBase(context));
            const ruled = (this.terminal?.rows ?? RULE_ROWS) >= RULE_ROWS;
            this.statusRows = status.length; footerAt = status.length + (ruled ? 1 : 0); hits = row.hits;
            return [...status, ...(ruled ? [rule(width) + "\x1b[0m"] : []), row.line];
          },
          handleMouse: event => {
            if (event.y !== footerAt) return undefined;
            const hit = plainClick(event) ? hits.find(item => event.x >= item.from && event.x < item.to) : undefined;
            if (hit?.hint.data) this.replay(hit.hint.data);
            return { handled: true };
          },
          invalidate() {}, dispose: unsubscribe,
        };
      });
      void this.loadFiles();
      for (const entry of this.posts.splice(0)) pi.sendMessage(entry, { triggerTurn: false });
      this.build?.sync();
      this.refresh();
    });
    // Input from any source (editor, bridge submit_input, extensions) shows its turn in Chat.
    pi.on("input", () => {
      if (this.workbench.view !== "conversation") { this.workbench.setView("conversation"); this.workbench.focused = false; this.refresh(); }
      return { action: "continue" };
    });
    pi.on("agent_start", () => { this.refresh(); });
    pi.on("agent_end", () => { this.refresh(); });
    pi.on("turn_end", () => { this.refresh(); });
    pi.on("model_select", () => { this.refresh(); });
    pi.on("thinking_level_select", () => { this.refresh(); });
    pi.on("tool_result", () => { this.workbench.invalidateFiles(); this.refresh(); });
    pi.registerCommand("panels", {
      description: `Native views and actions: ${PANEL_TARGETS}`,
      handler: async (args, ctx) => {
        if (ctx.mode !== "tui" || !this.terminal) { ctx.ui.notify("Native panels require the interactive Corral host.", "warning"); return; }
        await this.panelCommand(args);
      },
    });
    pi.registerCommand("actions", {
      description: "Search permitted host actions and session appearance/density (F2)",
      handler: async (_args, ctx) => {
        if (ctx.mode !== "tui" || !this.terminal) { ctx.ui.notify("Actions require native panels; plain UI is unchanged.", "warning"); return; }
        await this.palette();
      },
    });
  }
  private editableQueue(): boolean {
    return (this.options.snapshot()?.queue ?? []).some(item => item.editable);
  }
  /** The dequeue binding: the owner's last queued input returns above the draft; false leaves the key to Pi. */
  editQueued(): boolean {
    if (!this.canInteract || !this.editableQueue()) return false;
    void this.queue.editLast();
    return true;
  }
  /** The cards' live source for the build tool renderers and conversation entries; undefined outside build. */
  buildCards(): BuildCardSource | undefined { return this.build; }
  /**
   * `/finish`: the controller opens the exit choice, posts its result card and docks the dialog. False when the
   * interactive panels are not running, so the host opens the exit itself.
   */
  finish(): boolean {
    if (!this.build || !this.ui || this.disposed) return false;
    this.build.openFinish();
    return true;
  }
  /** `/board`: the Board view, when the interactive panels are running. */
  showBoard(): void {
    if (!this.build || !this.ui || this.disposed) return;
    this.workbench.setView("board");
  }
  /** Conversation entries join without starting a turn; while the parent streams Pi adds them after its turn. */
  private post(entry: BuildEntry<unknown>): void {
    if (this.pi && this.ui) this.pi.sendMessage(entry, { triggerTurn: false });
    else this.posts.push(entry);
  }
  private async panelCommand(args: string): Promise<void> {
    const value = args.trim().toLowerCase();
    if (value === "history") { await this.workbench.openHistory(); return; }
    if (value === "receipts") { await this.showReceipts(); return; }
    if (value === "project") { await this.workbench.repository(); this.refresh(); return; }
    if (value === "refresh") { await this.workbench.inspection.refresh(true); await this.loadFiles(); if (this.workbench.view === "inbox") await this.workbench.inbox.refresh(); return; }
    if (value === "toggle") { this.toggleSidebar(); return; }
    const view = value === "board" && !this.build ? undefined : PANEL_VIEWS[value];
    if (view && (!TASK_VIEWS[view] || this.options.identity.task)) {
      if (view === "agents") this.workbench.openAgents(); else if (view === "conversation") this.workbench.chat(); else this.workbench.setView(view);
      this.refresh(); return;
    }
    if (view) { this.ui?.notify(`/panels ${value}: this session has no bound task`, "warning"); return; }
    this.ui?.notify(`/panels ${plainLabel(value) || "(empty)"}: choose ${PANEL_TARGETS}`, "warning");
  }
  /** The bounded host receipt and event observations, read-only. */
  private async showReceipts(): Promise<void> {
    const ui = this.ui, info = this.options.snapshot()?.inspector;
    if (!ui) return;
    if (!info) { this.workbench.notice("Host receipts and events unavailable from this host", "warning"); return; }
    this.overlay = "pager"; this.refresh();
    try { await openOutputPager(ui, "Host receipts and events · observations, not task settlement", inspectorActivity(info).map(plainLabel)); }
    finally { this.overlay = undefined; this.refresh(); }
  }
  actions(): HostAction[] {
    const identity = this.options.identity, taskBound = !!identity.task;
    const scope = `${identity.role} · ${identity.project}`;
    const ready = (): string | undefined => !this.ui || !this.nativeAction ? "Interactive editor unavailable" : undefined;
    const queued = (): string | undefined => ready() ?? (this.editableQueue() ? undefined : "No queued input of yours to change");
    const action = (id: string, label: string, shortcut: string, run: HostAction["run"], disabled = ready): HostAction =>
      ({ id, label, shortcut, scope, run, disabled });
    const panel = (id: string, label: string, shortcut = "") => action(`/panels ${id}`, label, shortcut, () => this.panelCommand(id));
    const dequeue = piKey("app.message.dequeue"), focus = queueFocusKey();
    const items = [
      panel("chat", "Chat", "F9"),
      panel("agents", "Agents: helper calls and jobs", "F8"),
      action("agents.details", "Agent details", "", async () => { await this.workbench.openAgentDetails(); }, () => {
        if (ready()) return ready();
        const row = this.workbench.selectedAgent();
        return !row ? "No agent selected in Agents" : row.meta.length ? undefined : "No details for this agent";
      }),
      panel("changes", "Changes", "F7"),
      ...(taskBound ? [panel("task", "Task", "F5"), panel("proof", "Proof", "F10")] : []),
      ...(this.build ? this.buildActions(action, ready) : [panel("inbox", "Inbox", "F11")]),
      panel("usage", "Usage"),
      panel("history", "Browse history"),
      ...(taskBound ? [panel("graph", "Lifecycle graph"), panel("timeline", "Lifecycle events")] : []),
      panel("receipts", "Host receipts and events"),
      panel("project", "Choose project/worktree"),
      panel("refresh", "Refresh observations"),
      panel("toggle", "Collapse or expand the sidebar", "Ctrl+B"),
      action("sidebar.focus", "Sidebar: focus, then the next tab", "F6", () => this.sidebarKey()),
      action("queue.edit", "Edit the last queued input", dequeue ? keyLabel(dequeue) : "", () => { void this.queue.editLast(); }, queued),
      action("queue.focus", "Focus the queue", focus ? keyLabel(focus) : "", () => { this.focusQueue(); }, queued),
      action("keys", "Show keys", "F1", () => this.showKeys()),
      action("app.model.select", "Select model", this.nativeKeys?.("app.model.select") ?? "", () => this.nativeAction?.("app.model.select")),
      action("app.thinking.cycle", "Cycle effort", this.nativeKeys?.("app.thinking.cycle") ?? "", () => this.nativeAction?.("app.thinking.cycle"),
        () => !this.options.snapshot()?.effortAvailable ? "Effort unavailable for current model" : ready()),
      action("inbox.answer-flow", "Inspect established answer flow (not sent)", "", () => {
        this.workbench.setView("inbox"); this.workbench.focused = true; this.workbench.handleInput("a");
      }, () => {
        if (ready()) return ready();
        const role = identity.role, item = this.workbench.inbox.current;
        if (role !== "pm" && role !== "assistant") return `Role ${role} cannot answer; trusted owner flow required`;
        if (!item?.pending || !item.response) return "No pending selected item with an answer route";
        if (role === "pm" && (!item.pm_eligible || item.scope !== identity.project)) return "Owner-only or outside PM scope";
        return undefined;
      }),
    ];
    for (const appearance of Object.keys(PACKAGED) as Appearance[]) items.push(action(`appearance.${appearance}`,
      `Appearance: ${appearance}${appearance === this.appearance ? " (current)" : ""}`, "", () => {
        this.appearance = appearance; setPanelAppearance(appearance); setCanvasAppearance(appearance); setAgentHueAppearance(appearance);
        if (this.ui && this.themeCtor) this.ui.setTheme(panelTheme(this.themeCtor, this.ui.theme.getColorMode(), appearance));
        this.requestRender?.(true);
        this.refresh();
      }));
    for (const density of ["comfortable", "compact"] as const) items.push(action(`density.${density}`,
      `Density: ${density}${density === this.density ? " (current)" : ""}`, "", () => {
        this.density = density; this.updateDensity?.(); this.refresh();
      }));
    return items;
  }
  /** F2 in a build session: Board, the message receiver, open questions, Finish and board planning, each with its unavailable reason. */
  private buildActions(action: (id: string, label: string, shortcut: string, run: HostAction["run"], disabled?: () => string | undefined) => HostAction,
    ready: () => string | undefined): HostAction[] {
    const build = this.build, ui = this.options.build;
    if (!build || !ui) return [];
    const receiver = targetKey(), unavailable = () => ready() ?? boardUnavailable(this.options.snapshot()?.build);
    return [
      action("/panels board", "Open board", "F10", () => this.panelCommand("board")),
      action("build.target", "Change who receives the message", receiver ? keyLabel(receiver) : "", () => build.cycleTarget(),
        () => ready() ?? (ui.fleet.snapshot().children.some(steerable) ? undefined : "no agent is running")),
      ...build.questions().map(child => action(`build.answer.${child.id}`, `Answer ${plainLabel(child.id)}'s question`, "",
        () => { build.open("question", child.id); })),
      action("build.finish", "Finish: choose how this leaves the session", "", () => { build.openFinish(); }, () => ready() ?? build.finishUnavailable()),
      action("build.plan.review", "Review the proposed board plan", "", () => { build.open("plan"); },
        () => ready() ?? (ui.board.proposed() ? undefined : "no plan is waiting")),
      action("build.plan.draft", "Plan board tasks from this conversation", "", () => build.planDraft(), unavailable),
      action("build.board.refresh", "Refresh board status", "", () => ui.board.refresh(), unavailable),
    ];
  }
  private themeCtor: typeof Sdk.Theme | undefined;
  private updateDensity: (() => void) | undefined;
  private async palette(): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    this.overlay = "palette"; this.refresh();
    let action: HostAction | undefined;
    try {
      // A popup centred near the top, as the mock's backdrop places it: at most 78 cells wide.
      action = await ui.custom<HostAction | undefined>((tui, _theme, _keys, done) =>
        new ActionPalette(this.actions(), done, () => tui.requestRender(), Math.max(1, Math.min(12, tui.terminal.rows - 8))),
      { overlay: true, overlayOptions: () => this.popupPlace() });
    } finally { this.overlay = undefined; this.refresh(); }
    if (!action || this.disposed) return;
    const reason = action.disabled();
    if (reason) { this.workbench.notice(reason, "warning"); return; }
    try { await action.run(); } catch (error) { this.workbench.notice(plainLabel(String(error)), "error"); }
    this.refresh();
  }
  /** F1: the keys this mode offers; Enter or a click runs a key as if typed. */
  private async showKeys(): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    const identity = this.options.identity;
    const groups = keyGroups({ taskBound: !!identity.task, answer: identity.role === "pm" || identity.role === "assistant",
      jobs: this.options.snapshot()?.jobs != null, queueControl: this.queue.controllable, lifecycle: !!identity.task,
      ...(this.build ? { build: true } : {}) });
    this.overlay = "keys"; this.refresh();
    let data: string | undefined;
    try {
      data = await ui.custom<string | undefined>((tui, _theme, _keys, done) =>
        new KeysList(groups, done, () => tui.requestRender(), Math.max(3, Math.min(22, tui.terminal.rows - 8))),
      { overlay: true, overlayOptions: () => this.popupPlace() });
    } finally { this.overlay = undefined; this.refresh(); }
    if (data && !this.disposed) this.replay(data);
  }
  /** Where the F1 and F2 popups sit: centred, at most 78 cells wide, 12% down (one row down when short). */
  private popupPlace(): OverlayOptions {
    const columns = this.terminal?.physical.columns ?? 80, rows = this.terminal?.rows ?? 24;
    const top = rows < SHORT_ROWS ? 1 : Math.round(rows * 0.12);
    return { anchor: "top-center", width: Math.max(20, Math.min(78, columns - 4)), offsetY: top, maxHeight: Math.max(4, rows - top - 1) };
  }
}
