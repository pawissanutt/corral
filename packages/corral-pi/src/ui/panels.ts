// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, isKeyRelease, matchesKey, TuiAltScreen, visibleWidth, type Component, type Terminal, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import type { BuildUi } from "../build/types.ts";
import { PanelTerminal, PANEL_BREAKPOINT, PANEL_WIDTH, RAIL_MIN, RAIL_MAX, MIN_MAIN, type PanelSurface } from "./panel-terminal.ts";
import { NativeWorkbench, plainClick, type MainView } from "./workbench.ts";
import type { HistoryRead } from "./history.ts";
import type { ReviewBases } from "./review-store.ts";
import type { JobControl } from "../host/job-controls.ts";
import type { QueueControl } from "../host/queue-control.ts";
import { panelTheme } from "./panels-theme.ts";
import { type Appearance, type Density, PACKAGED } from "./appearance.ts";
import { ActionPalette, type HostAction } from "./action-palette.ts";
import { setCanvasAppearance } from "./panel-canvas.ts";
import { setAgentHueAppearance } from "./agent-hues.ts";
import { statusBar } from "./panels-chrome.ts";
import { composerBar, composerTitle, type ComposerHit } from "./composer-chrome.ts";
import { chromeTier, COMPACT_COLUMNS, SHORT_ROWS } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, keyLabel, piKey, queueFocusKey, type FooterHit, type HintState, type KeyContext } from "./keymap.ts";
import { KeysList } from "./keys-list.ts";
import { QueueDock } from "./queue-dock.ts";
import { Sidebar, type SideHit, type SideTarget } from "./sidebar.ts";
import { inspectorActivity } from "./inspector.ts";
import { openOutputPager } from "./job-detail.ts";
import { pad, plainLabel, setPanelAppearance, PANEL_MUTED, type PanelSnapshot } from "./panels-view.ts";

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
}
/** `/panels <target>` names for main views; history, receipts, project, refresh and toggle are actions. */
const PANEL_VIEWS: Readonly<Record<string, MainView>> = {
  chat: "conversation", files: "changes", changes: "changes", agents: "agents", activity: "agents", jobs: "agents", helpers: "agents",
  usage: "usage", evidence: "evidence", proof: "evidence", inbox: "inbox", task: "task", graph: "graph", timeline: "timeline",
};
/** Views that read the bound task; sessions without one do not offer them. */
const TASK_VIEWS: Readonly<Partial<Record<MainView, true>>> = { task: true, evidence: true, graph: true, timeline: true };
const PANEL_TARGETS = "chat, agents, changes, task, proof, inbox, usage, graph, timeline, history, receipts, project, refresh or toggle";

/** Where keys go when no Pi dialog or overlay is open: Pi's editor, the main view, the sidebar or the queue. */
export type PanelFocus = "composer" | "main" | "sidebar" | "queue";

export class NativePanels implements PanelSurface {
  readonly workbench: NativeWorkbench;
  readonly sidebar: Sidebar;
  readonly queue: QueueDock;
  sidebarEnabled = true;
  sidebarWidth = PANEL_WIDTH;
  private drag: { readonly start: number } | undefined;
  private separatorAt = 0;
  private railHits: ReadonlyMap<number, readonly SideHit[]> = new Map();
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

  constructor(private readonly options: NativePanelsOptions) {
    this.sidebar = new Sidebar(() => this.refresh());
    this.queue = new QueueDock({
      items: () => this.options.snapshot()?.queue ?? [], control: () => this.options.queueControl,
      draft: () => this.ui?.getEditorText() ?? "", setDraft: text => this.ui?.setEditorText(text),
      focusComposer: () => this.focusComposer(), notice: (text, tone) => this.workbench.notice(text, tone),
      changed: () => this.refresh(),
    });
    this.workbench = new NativeWorkbench({ ...options,
      terminal: () => this.terminal ? { columns: this.terminal.physical.columns, rows: this.terminal.rows } : undefined,
      onFocusMain: () => { this.sidebar.focused = false; this.queue.focused = false; this.closeKeyDrawer(); },
      canInteract: () => this.canInteract && !this.activeSelection?.(), changed: () => this.refresh() });
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
    return this.queue.focused ? "queue" : this.sidebar.focused ? "sidebar" : this.workbench.focused ? "main" : "composer";
  }
  layoutChanged(): void { this.refresh(); }
  refresh(): void {
    if (this.queued || this.disposed) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      if (this.disposed) return;
      this.requestRender?.();
      this.terminal?.redrawSidebar();
    });
  }
  dispose(): void {
    this.disposed = true;
    this.workbench.dispose();
    this.ui = undefined;
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
  /** Back to Pi's editor: the main view, the sidebar and the queue give up the focus. */
  focusComposer(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false;
    this.closeKeyDrawer(); this.refresh();
  }
  /** F6: focus the sidebar (opening the drawer when there is no rail), or the next tab once it has the focus. */
  private sidebarKey(): void {
    if (this.sidebar.focused) this.sidebar.nextTab();
    else {
      this.queue.focused = false; this.workbench.focused = false; this.sidebar.focused = true;
      if (!this.wide && !this.sidebar.drawer) { this.sidebar.drawer = true; this.drawerByKey = true; }
    }
    this.refresh();
  }
  /** Alt+↓: the queue takes the focus with the last editable item selected; false when nothing is editable. */
  private focusQueue(): boolean {
    if (!this.queue.focusLast()) return false;
    this.sidebar.focused = false; this.workbench.focused = false; this.closeKeyDrawer(); this.refresh();
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
    if (target.kind === "agent" || target.kind === "job") this.workbench.openAgents(target.key);
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
  /** The collapsed sidebar row or its drawer (only without a rail), then the queue: one widget above the editor. */
  private dock(): Component {
    let sideRows = 0;
    let sideHits: ReadonlyMap<number, readonly SideHit[]> = new Map();
    return {
      render: width => {
        const rows = this.terminal?.rows ?? 24;
        this.queue.setCapacity(rows);
        let side: readonly string[] = [];
        sideHits = new Map();
        if (!this.wide) {
          const source = this.workbench.sideSource();
          const frame = this.sidebar.drawer ? this.sidebar.drawerFrame(source, width, rows)
            : this.sidebar.row(source, width, chromeTier(this.terminal?.physical.columns ?? width) === "tiny");
          side = frame.lines; sideHits = frame.hits;
        }
        sideRows = side.length;
        return [...side, ...this.queue.render(width)];
      },
      handleMouse: event => {
        if (event.y < sideRows) {
          if (this.canInteract && plainClick(event)) this.sideClick(sideHits.get(event.y)?.find(hit => event.x >= hit.from && event.x < hit.to), false);
          return { handled: true };
        }
        if (!this.canInteract) return undefined;
        const result = this.queue.handleMouse({ ...event, y: event.y - sideRows });
        if (this.queue.focused) { this.sidebar.focused = false; this.workbench.focused = false; this.closeKeyDrawer(); this.refresh(); }
        return result;
      },
      invalidate() {},
    };
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
      const column = Number(mouse[2]) - 1, x = column - this.terminal.columns - 1;
      const button = Number(mouse[1]);
      const release = mouse[4] === "m" || (button & 3) === 3;
      // The separator column (x = -1) resizes the rail: drag, Esc restores, double-click resets.
      if (this.drag) {
        if (release) { this.drag = undefined; this.refresh(); }
        else if (button & 32) this.resizeRail(this.terminal.physical.columns - column - 1);
        return true;
      }
      if (x === -1 && mouse[4] === "M" && button === 0 && this.canInteract) {
        const now = Date.now();
        if (now - this.separatorAt < 400) { this.separatorAt = 0; this.resizeRail(PANEL_WIDTH); }
        else { this.separatorAt = now; this.drag = { start: this.terminal.railWidth }; this.refresh(); }
        return true;
      }
      if (mouse[4] === "M" && !(button & 96) && !release) this.pointerOrigin = x >= 0 ? "sidebar" : "main";
      const origin = this.pointerOrigin;
      if (release) this.pointerOrigin = undefined;
      // A main-pane selection must receive its release even when dragged into the rail.
      if (origin === "main") return false;
      if (x >= 0 || origin === "sidebar") {
        if (x >= 0 && this.canInteract && mouse[4] === "M" && button === 0) {
          const row = Number(mouse[3]) - 1;
          this.sideClick(this.railHits.get(row)?.find(hit => x >= hit.from && x < hit.to), true);
        }
        return true; // Physical rail cells must not enter Pi's smaller main viewport.
      }
    }
    if (this.drag && matchesKey(data, "escape")) { const start = this.drag.start; this.drag = undefined; this.resizeRail(start); this.refresh(); return true; }
    if (!this.canInteract || mouse) return false;
    if (matchesKey(data, "f1")) { void this.showKeys(); return true; }
    if (matchesKey(data, "f6")) { this.sidebarKey(); return true; }
    if (matchesKey(data, "ctrl+b")) { this.toggleSidebar(); return true; }
    if (this.wide && (matchesKey(data, "alt+,") || matchesKey(data, "alt+."))) {
      this.resizeRail((this.terminal?.railWidth ?? this.sidebarWidth) + (matchesKey(data, "alt+.") ? 2 : -2)); return true;
    }
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
    pi.on("ui_prompt_start", () => { this.prompts++; this.refresh(); });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); this.refresh(); });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui;
      this.workbench.bind(ctx.ui);
      this.prompts = 0;
      setPanelAppearance(this.appearance); setCanvasAppearance(this.appearance); setAgentHueAppearance(this.appearance);
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode(), this.appearance));
      const owner = this;
      ctx.ui.setEditorComponent((tui, theme, keys) => {
        const editor = new class extends sdk.CustomEditor {
          private composer: { readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit } | undefined;
          private bottomRow = -1;
          dispatchNative(id: "app.model.select" | "app.thinking.cycle"): void { this.actionHandlers.get(id)?.(); }
          setPaddingX(padding: number): void { super.setPaddingX(owner.density === "compact" ? 0 : Math.max(2, padding)); }
          handleInput(data: string): void {
            if (owner.canInteract && matchesKey(data, "f2")) { void owner.palette(); return; }
            // The dequeue binding pulls the owner's last queued input back above the draft.
            if (keys.getKeys("app.message.dequeue").some(key => matchesKey(data, key)) && owner.editQueued()) return;
            // A message sent from another view returns to Chat, so its reply and any refusal notice show.
            if (owner.workbench.view !== "conversation" && this.getText().trim()
                && keys.getKeys("tui.input.submit").some(key => matchesKey(data, key))) {
              owner.workbench.setView("conversation"); owner.workbench.focused = false;
            }
            super.handleInput(data);
          }
          protected renderTopBorder(width: number, hidden: number): string {
            if (hidden) return super.renderTopBorder(width, hidden);
            const tiny = (owner.terminal?.physical.columns ?? width) < COMPACT_COLUMNS;
            const label = pad(` ${tiny ? "Message" : composerTitle(owner.options.snapshot())} `, Math.max(0, width - 3)).trimEnd();
            return PANEL_MUTED + "──" + label + "─".repeat(Math.max(0, width - visibleWidth(label) - 2));
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            this.composer = composerBar(owner.options.snapshot(), width, hidden, (owner.terminal?.rows ?? SHORT_ROWS) < SHORT_ROWS);
            return this.composer.line;
          }
          handleMouse(event: TuiMouseEvent) {
            if (event.type === "click" && event.button === "left") {
              owner.focusComposer();
              if (owner.canInteract && event.y === this.bottomRow) {
                const hit = (range: { from: number; to: number } | undefined) => range && event.x >= range.from && event.x < range.to;
                // Use the same native actions as Ctrl+L / Shift+Tab; selection still commits through host preflight.
                if (hit(this.composer?.model)) { this.actionHandlers.get("app.model.select")?.(); return { handled: true }; }
                if (hit(this.composer?.effort)) { this.actionHandlers.get("app.thinking.cycle")?.(); return { handled: true }; }
              }
            }
            return super.handleMouse(event);
          }
          render(width: number): string[] {
            this.borderColor = text => ctx.ui.theme.fg("borderAccent", text);
            const lines = super.render(width);
            this.bottomRow = this.composer ? lines.indexOf(this.composer.line) : -1;
            // No side rules: terminal row selection would copy them into the message text.
            return lines.map(line => ctx.ui.theme.bg("userMessageBg", pad(line, width)));
          }
        }(tui, theme, keys);
        editor.setPaddingX(2);
        owner.nativeAction = id => editor.dispatchNative(id);
        owner.updateDensity = () => editor.setPaddingX(2);
        owner.nativeKeys = id => keys.getKeys(id).map(key => keyLabel(key)).join(", ");
        owner.editorFocused = () => editor.focused;
        owner.requestRender = force => tui.requestRender(force);
        owner.hasOverlay = () => tui.hasOverlay();
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
        let statusRows = 0;
        let hits: readonly FooterHit[] = [];
        return {
          // The status bar, then one row of the focused surface's keys ending with F1.
          render: width => {
            const status = statusBar(this.options.identity, this.options.snapshot(), data.getExtensionStatuses(), width);
            const context = this.footerContext();
            const row = footerRow(contextHints(this.workbench.hintState(context, this.queueState())),
              chromeTier(this.terminal?.physical.columns ?? width), width, footerBase(context));
            statusRows = status.length; hits = row.hits;
            return [...status, row.line];
          },
          handleMouse: event => {
            if (event.y !== statusRows) return undefined;
            const hit = plainClick(event) ? hits.find(item => event.x >= item.from && event.x < item.to) : undefined;
            if (hit?.hint.data) this.replay(hit.hint.data);
            return { handled: true };
          },
          invalidate() {}, dispose: unsubscribe,
        };
      });
      void this.loadFiles();
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
  private async panelCommand(args: string): Promise<void> {
    const value = args.trim().toLowerCase();
    if (value === "history") { await this.workbench.openHistory(); return; }
    if (value === "receipts") { await this.showReceipts(); return; }
    if (value === "project") { await this.workbench.repository(); this.refresh(); return; }
    if (value === "refresh") { await this.workbench.inspection.refresh(true); await this.loadFiles(); if (this.workbench.view === "inbox") await this.workbench.inbox.refresh(); return; }
    if (value === "toggle") { this.toggleSidebar(); return; }
    const view = PANEL_VIEWS[value];
    if (view && (!TASK_VIEWS[view] || this.options.identity.task)) {
      if (view === "agents") this.workbench.openAgents(); else this.workbench.setView(view);
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
      panel("changes", "Changes", "F7"),
      ...(taskBound ? [panel("task", "Task", "F5"), panel("proof", "Proof", "F10")] : []),
      panel("inbox", "Inbox", "F11"),
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
  private themeCtor: typeof Sdk.Theme | undefined;
  private updateDensity: (() => void) | undefined;
  private async palette(): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    this.overlay = "palette"; this.refresh();
    let action: HostAction | undefined;
    try {
      action = await ui.custom<HostAction | undefined>((tui, theme, _keys, done) =>
        new ActionPalette(this.actions(), theme, done, () => tui.requestRender(), Math.max(1, Math.min(12, tui.terminal.rows - 8))));
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
      jobs: this.options.snapshot()?.jobs != null, queueControl: this.queue.controllable, lifecycle: !!identity.task });
    this.overlay = "keys"; this.refresh();
    let data: string | undefined;
    try {
      data = await ui.custom<string | undefined>((tui, theme, _keys, done) =>
        new KeysList(groups, theme, done, () => tui.requestRender(), Math.max(3, Math.min(18, tui.terminal.rows - 9))));
    } finally { this.overlay = undefined; this.refresh(); }
    if (data && !this.disposed) this.replay(data);
  }
}
