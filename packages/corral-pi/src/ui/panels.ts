// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import {
  getKeybindings, isKeyRelease, matchesKey, Text, TuiAltScreen, visibleWidth, type Component, type Keybinding, type Terminal, type TuiMouseEvent,
} from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { PanelTerminal, PANEL_WIDTH, RAIL_MIN, RAIL_MAX, MIN_MAIN, type PanelSurface } from "./panel-terminal.ts";
import { ACTIVITY_VIEWS, NativeWorkbench, plainClick, type MainView } from "./workbench.ts";
import { navRows, renderNav } from "./nav-panel.ts";
import { activityKey, activitySidebar } from "./activity-sidebar.ts";
import type { HistoryRead } from "./history.ts";
import { panelTheme } from "./panels-theme.ts";
import { type Appearance, type Density, PACKAGED } from "./appearance.ts";
import { ActionPalette, type HostAction } from "./action-palette.ts";
import { setPanelAppearance } from "./panels-view.ts";
import { setCanvasAppearance } from "./panel-canvas.ts";
import { composerLayout, composerTitle, overviewLines, statusBar, splitRow } from "./panels-chrome.ts";
import { pagerLabel, scrollWindow, selectionWindow, type PageWindow } from "./panels-pager.ts";
import {
  box, pad, panelLines, plainLabel, diffLine, styledPanelLine, PANEL_TABS, PANEL_BG, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, RESET,
  type FilesState, type PanelSnapshot, type PanelTab, type PanelLine,
} from "./panels-view.ts";

export interface NativePanelsOptions {
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly inspection?: () => Promise<unknown>;
  readonly lifecycle?: () => Promise<unknown>;
  readonly artifacts?: () => Promise<unknown>;
  readonly artifactViewer?: (reference: string, viewer: "show" | "serve") => Promise<string>;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly readTask?: () => Promise<unknown>;
  readonly snapshot: () => PanelSnapshot | null;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
}
interface TabHit { readonly from: number; readonly to: number; readonly tab: PanelTab }
interface PagerHit { readonly y: number; readonly from: number; readonly to: number }
// Pi's own viewport bindings (paging, prompt jumps, jump-to-live) scroll the transcript and keep inspector focus.
const VIEWPORT_KEYS: readonly Keybinding[] = ["tui.altScreen.pageUp", "tui.altScreen.pageDown", "tui.altScreen.halfPageUp",
  "tui.altScreen.halfPageDown", "tui.altScreen.lineUp", "tui.altScreen.lineDown", "tui.altScreen.previousPrompt",
  "tui.altScreen.nextPrompt", "tui.altScreen.top", "tui.altScreen.bottom"];

export class NativePanels implements PanelSurface {
  readonly workbench: NativeWorkbench;
  sidebarEnabled = true;
  sidebarWidth = PANEL_WIDTH;
  private drag: { readonly start: number } | undefined;
  private separatorAt = 0;
  private navHits: ReadonlyMap<number, MainView> = new Map();
  appearance: Appearance = "dark";
  density: Density = "comfortable";
  private nativeAction: ((id: "app.model.select" | "app.thinking.cycle") => void) | undefined;
  private nativeKeys: ((id: "app.model.select" | "app.thinking.cycle") => string) | undefined;
  tab: PanelTab = "Context";
  focused = false;
  expanded = false;
  offset = 0;
  private terminal: PanelTerminal | undefined;
  private ui: ExtensionUIContext | undefined;
  private requestRender: ((force?: boolean) => void) | undefined;
  private editorFocused: (() => boolean) | undefined;
  private activeSelection: (() => boolean) | undefined;
  private hasOverlay: (() => boolean) | undefined;
  private prompts = 0;
  private inspecting = false;
  private pointerOrigin: "main" | "sidebar" | undefined;
  private queued = false;
  private disposed = false;
  private files: FilesState = { kind: "unread" };
  private contentRows: readonly PanelLine[] = [];
  private tabHits: TabHit[] = [];
  private overviewOffset = 0;
  private filesStart = 0;
  private activityStart = 0;
  private activityCursor: string | undefined;
  private historyExpanded = false;
  private activityFocused = false;
  private get navigatingActivity(): boolean { return this.tab === "Context" && this.focused && this.activityFocused; }
  private railPager: PagerHit | undefined;
  private inlinePager: PagerHit | undefined;

  constructor(private readonly options: NativePanelsOptions) {
    this.workbench = new NativeWorkbench({ ...options, railVisible: () => this.wide, canInteract: () => this.canInteract && !this.activeSelection?.(), changed: () => {
      const review = this.workbench.review;
      this.files = review.error && !review.checkedAt ? { kind: "error", reason: plainLabel(review.error) }
        : review.refreshing && !review.checkedAt ? { kind: "loading" } : { kind: "ready", files: review.files };
      this.refresh();
    } });
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
  private tabs(): string {
    let x = 0;
    this.tabHits = [];
    return PANEL_TABS.map(tab => {
      this.tabHits.push({ from: x, to: x + tab.length, tab });
      x += tab.length + 1;
      return (tab === this.tab ? PANEL_BLUE + "\x1b[1;4m" : PANEL_MUTED) + tab + "\x1b[22;24m" + PANEL_TEXT;
    }).join(" ");
  }
  private lines() { return panelLines(this.tab, this.options.identity, this.options.snapshot(), this.files); }
  private choose(tab: PanelTab): void {
    this.tab = tab; this.activityFocused = false;
    this.offset = 0;
    if (tab === "Files") { this.workbench.setView("changes"); if (this.files.kind === "unread") void this.loadFiles(); }
    this.refresh();
  }
  private activityRows(): PanelLine[] {
    return activitySidebar(this.options.snapshot(), this.activityCursor ?? (this.workbench.selectedActivity && activityKey(this.workbench.selectedActivity)), this.historyExpanded);
  }
  private actionKey(action: PanelLine["action"]): string | undefined {
    return action?.kind === "helper" || action?.kind === "job" ? activityKey(action) : action?.kind === "history" ? "history" : undefined;
  }
  private selectActivity(): void {
    const keys = this.activityRows().map(row => this.actionKey(row.action)).filter(key => key !== undefined);
    if (this.activityCursor === undefined || !keys.includes(this.activityCursor)) this.activityCursor = keys[0];
  }
  private browseActivity(): void {
    this.tab = "Context"; this.focused = true; this.activityFocused = true; this.workbench.focused = false;
    this.sidebarEnabled = true; this.selectActivity();
    this.terminal?.refreshLayout(); this.refresh();
  }
  async loadFiles(): Promise<void> { await this.workbench.review.refresh(); }
  private displayed(): { readonly rows: readonly PanelLine[]; readonly overview: boolean } {
    if (this.navigatingActivity && !this.wide) return { rows: this.activityRows(), overview: false };
    if (this.tab === "Context" && (this.wide && !this.focused || this.navigatingActivity)) return { rows: overviewLines(this.options.snapshot(), this.files,
      { name: this.workbench.scopeLabel, required: this.workbench.review.scopeRequired },
      this.navigatingActivity ? this.activityCursor : this.workbench.selectedActivity && activityKey(this.workbench.selectedActivity), this.historyExpanded), overview: true };
    const rows = this.tab === "Files" ? this.workbench.files() : this.lines();
    return { rows: this.wide ? rows : rows.filter(line => !line.heading), overview: false };
  }
  private inlineCapacity(rows: number): number {
    if (this.navigatingActivity) return Math.max(2, Math.min(12, Math.floor(rows / 2) - 2));
    return this.focused || this.expanded ? Math.min(this.expanded ? 4 : 2, Math.max(1, Math.floor(rows / 8))) : 0;
  }
  private get capacity(): number {
    const rows = this.terminal?.rows ?? 0;
    return this.wide ? Math.max(0, rows - 5) : this.inlineCapacity(rows);
  }
  private wrapped(width: number): { readonly rows: readonly PanelLine[]; readonly overview: boolean } {
    const displayed = this.displayed();
    const { overview } = displayed;
    const rows = this.density === "compact" ? displayed.rows.filter(row => row.text.trim() !== "") : displayed.rows;
    return { overview, rows: rows.flatMap(row => row.text.trim() === "" ? [row]
      : new Text(row.text, 0, 0).render(Math.max(1, width - (row.indent ?? 0) - (row.continuationIndent ?? 0)))
        .map((text, index) => ({ ...row, text, indent: (row.indent ?? 0) + (index ? row.continuationIndent ?? 0 : 0) }))) };
  }
  private page(capacity: number, width: number): { readonly window: PageWindow; readonly pager: string } {
    if (this.navigatingActivity) this.selectActivity();
    const { rows, overview } = this.wrapped(width);
    let window: PageWindow;
    if (this.navigatingActivity) {
      const selected = rows.findIndex(row => this.actionKey(row.action) === this.activityCursor && row.action !== undefined);
      window = selectionWindow(rows, capacity, selected, this.activityStart);
      if (capacity > 0) this.activityStart = window.start;
    } else if (this.tab === "Files") {
      const selected = rows.findIndex(row => row.action?.kind === "diff" && row.action.path === this.workbench.review.selected);
      window = selectionWindow(rows, capacity, selected, this.filesStart);
      if (capacity > 0) this.filesStart = window.start;
    } else window = scrollWindow(rows, capacity, overview ? this.overviewOffset : this.offset);
    this.contentRows = window.rows;
    return { window, pager: capacity > 0 ? pagerLabel(window, rows.length) : "" };
  }
  private styled(rows: readonly PanelLine[]): string[] {
    return rows.map(styledPanelLine);
  }
  renderSidebar(width: number, height: number): readonly string[] {
    // At rest the rail is quick navigation; F6 opens the detailed inspector in place.
    if (this.tab === "Context" && !this.focused) {
      const { identity } = this.options, wb = this.workbench;
      const frame = renderNav(navRows({ taskBound: wb.task.bound, proof: !!identity.task && !!identity.run,
        snapshot: this.options.snapshot(), review: wb.review, inspection: wb.inspection, inbox: wb.inbox, view: wb.view }),
        view => view === wb.view || view === "activity" && ACTIVITY_VIEWS.has(wb.view), width, height,
        this.drag ? `⇔ ${width} cols` : "GO TO");
      this.navHits = frame.hits;
      return frame.lines;
    }
    this.navHits = new Map();
    const inner = width - 4, { window, pager } = this.page(Math.max(0, height - 5), inner);
    const hint = this.focused ? "↑/↓ · Enter open · Esc" : "F8 agents/jobs · F6 context";
    this.railPager = pager ? { y: height - 2, from: inner - visibleWidth(pager), to: inner } : undefined;
    const lines = [this.tabs(), "", ...this.styled(window.rows)];
    while (lines.length < height - 3) lines.push("");
    lines.push(splitRow(PANEL_MUTED + hint + PANEL_TEXT, pager ? PANEL_BLUE + pager + PANEL_TEXT : "", inner));
    return box(this.workbench.review.scopeRequired ? "Choose project" : this.workbench.scopeLabel, lines, width, height, this.focused);
  }
  renderInline(width: number, rows: number): string[] {
    this.inlinePager = undefined;
    if (this.wide) return [];
    const { window, pager } = this.page(this.inlineCapacity(rows), Math.max(1, width - 2));
    const tabs = PANEL_TEXT + " " + this.tabs(), room = width - visibleWidth(tabs) - 2;
    const hints = this.focused ? ["↑↓ Enter · Esc", "↑↓ · Esc", ""] : ["F6 inspect", ""];
    const hint = hints.find(text => visibleWidth(pager) + (pager && text ? 2 : 0) + visibleWidth(text) <= room) ?? "";
    const right = (pager ? PANEL_BLUE + pager + (hint ? "  " : "") : "") + PANEL_MUTED + hint;
    // click() receives inline x - 1, matching the one-cell inset before the first tab.
    const from = width - visibleWidth(right) - 1;
    if (pager) this.inlinePager = { y: 0, from, to: from + visibleWidth(pager) };
    return [PANEL_BG + splitRow(tabs, right, width) + RESET,
      ...this.styled(window.rows).map(line => PANEL_BG + pad("  " + line, width) + RESET)];
  }
  private move(delta: number): void {
    if (this.navigatingActivity) {
      this.selectActivity();
      const keys = [...new Set(this.activityRows().map(row => this.actionKey(row.action)).filter(key => key !== undefined))];
      this.activityCursor = keys[Math.max(0, Math.min(keys.length - 1, keys.indexOf(this.activityCursor ?? "") + delta))];
    } else if (this.tab === "Files" && this.files.kind === "ready") {
      const visible = this.workbench.review.visible;
      const index = visible.findIndex(file => file.path === this.workbench.review.selected);
      const file = visible[Math.max(0, Math.min(visible.length - 1, index + delta))];
      if (file) { void this.workbench.openFile(file.path); this.workbench.focused = false; }
    } else if (this.capacity > 0) {
      const { rows, overview } = this.wrapped(this.wide ? (this.terminal?.railWidth ?? PANEL_WIDTH) - 4 : Math.max(1, (this.terminal?.columns ?? 80) - 2));
      const current = scrollWindow(rows, this.capacity, overview ? this.overviewOffset : this.offset).offset;
      const next = scrollWindow(rows, this.capacity, current + delta).offset;
      if (overview) this.overviewOffset = next; else this.offset = next;
    }
    this.refresh();
  }
  async inspect(): Promise<void> {
    if (!this.canInteract) return;
    if (this.navigatingActivity) {
      this.selectActivity();
      const action = this.activityRows().find(row => row.action && this.actionKey(row.action) === this.activityCursor)?.action;
      if (action) this.activate(action);
      return;
    }
    if (!this.ui || !this.canInteract || this.inspecting) return;
    this.inspecting = true;
    try {
    if (this.tab === "Files") {
      if (this.workbench.review.scopeRequired) { await this.workbench.repository(); return; }
      if (this.files.kind === "unread" || this.files.kind === "error") { await this.loadFiles(); return; }
      if (this.files.kind === "ready") {
        const file = this.workbench.review.current;
        if (file) {
          const ui = this.ui;
          try {
            await this.workbench.openFile(file.path);
            this.focused = false;
          }
          catch (error) { ui.notify(error instanceof Error ? error.message : String(error), "error"); }
          return;
        }
      }
    }
    await this.detail(this.tab, this.lines().map(line => line.text));
    } catch (error) {
      if (!this.disposed) this.ui?.notify(error instanceof Error ? error.message : String(error), "error");
    } finally { this.inspecting = false; }
  }
  private async detail(title: string, source: readonly string[], kind: "text" | "diff" = "text"): Promise<void> {
    const ui = this.ui;
    if (!ui) return;
    this.focused = false;
    await ui.custom<void>((tui, _theme, _keys, done) => {
      let offset = 0, max = 0;
      return {
        render(width) {
          const height = Math.max(4, Math.floor(tui.terminal.rows * 0.8));
          const lines = source.flatMap(line => new Text(kind === "diff" ? diffLine(line, _theme)
            : line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " "), 0, 0).render(Math.max(1, width - 4)));
          max = Math.max(0, lines.length - height + 3); offset = Math.min(offset, max);
          return box(title + " · Esc close", [...lines.slice(offset, offset + height - 3), "↑/↓ scroll · Esc close"], width, height, true);
        },
        handleInput(data) {
          if (matchesKey(data, "escape") || data === "q" || matchesKey(data, "enter")) { done(); return; }
          if (matchesKey(data, "up")) offset = Math.max(0, offset - 1);
          if (matchesKey(data, "down")) offset = Math.min(max, offset + 1);
          if (matchesKey(data, "pageUp")) offset = Math.max(0, offset - 8);
          if (matchesKey(data, "pageDown")) offset = Math.min(max, offset + 8);
          tui.requestRender();
        },
        handleMouse(event) {
          if (event.type !== "wheel") return undefined;
          offset = Math.max(0, Math.min(max, offset + (event.wheelDelta ?? 0)));
          tui.requestRender(); return { handled: true };
        },
        invalidate() {},
      };
    }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
    this.refresh();
  }
  private click(x: number, y: number, inline: boolean): void {
    if (!this.canInteract) return;
    const pager = inline ? this.inlinePager : this.railPager;
    if (pager && y === pager.y && x >= pager.from && x < pager.to) {
      if (x === pager.from) this.move(-1);
      else if (x === pager.to - 1) this.move(1);
      return;
    }
    if (y === (inline ? 0 : 1)) {
      const tab = this.tabHits.find(hit => x >= hit.from && x < hit.to)?.tab;
      if (tab) { if (inline) this.focused = true; this.choose(tab); }
      return;
    }
    const row = this.contentRows[y - (inline ? 1 : 3)];
    if (!row) return;
    const action = row.action;
    if (!action) { if (this.tab !== "Files") void this.inspect(); return; }
    this.activate(action);
  }
  private activate(action: NonNullable<PanelLine["action"]>): void {
    switch (action.kind) {
      case "diff": {
        if (this.files.kind !== "ready") return;
        const index = this.files.files.findIndex(file => file.path === action.path);
        if (index < 0) return; // A refresh may have removed the displayed path.
        this.choose("Files"); this.focused = false;
        void this.workbench.openFile(action.path); this.refresh(); return;
      }
      case "helpers": this.browseActivity(); return;
      case "helper":
        this.activityCursor = activityKey(action); this.focused = false;
        this.workbench.openHelpers(action.request); this.refresh(); return;
      case "job":
        this.activityCursor = activityKey(action); this.focused = false;
        this.workbench.openJob(action.job); this.refresh(); return;
      case "history": this.historyExpanded = !this.historyExpanded; this.selectActivity(); this.refresh(); return;
      case "scope": void this.workbench.repository(); return;
      case "files": this.focused = true; this.choose("Files"); return;
      default: {
        const unreachable: never = action;
        throw new Error(`Unknown native panel action: ${String(unreachable)}`);
      }
    }
  }
  inlineMouse(event: TuiMouseEvent): { handled: true } | undefined {
    if (!this.canInteract) return undefined;
    if (event.type === "wheel") { this.move(event.wheelDelta ?? 0); return { handled: true }; }
    if (plainClick(event)) { this.click(event.x - 1, event.y, true); return { handled: true }; }
    return undefined;
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
        if (x >= 0 && this.canInteract && mouse[4] === "M") {
          const button = Number(mouse[1]);
          if (button & 64) this.move(button & 1 ? 3 : -3);
          else if (button === 0 && this.navHits.size) { const view = this.navHits.get(Number(mouse[3]) - 1); if (view) this.openNav(view); }
          else if (button === 0) this.click(x - 2, Number(mouse[3]) - 1, false);
        }
        return true; // Physical rail cells must not enter Pi's smaller main viewport.
      }
    }
    if (this.drag && matchesKey(data, "escape")) { const start = this.drag.start; this.drag = undefined; this.resizeRail(start); this.refresh(); return true; }
    if (!this.canInteract || mouse) return false;
    if (this.wide && (matchesKey(data, "alt+,") || matchesKey(data, "alt+."))) {
      this.resizeRail((this.terminal?.railWidth ?? this.sidebarWidth) + (matchesKey(data, "alt+.") ? 2 : -2)); return true;
    }
    if (matchesKey(data, "f8")) { this.browseActivity(); return true; }
    if (matchesKey(data, "f7") || matchesKey(data, "f9") || matchesKey(data, "f10") || matchesKey(data, "f11")) {
      this.focused = false;
      if (matchesKey(data, "f7")) this.tab = "Files";
      return this.workbench.handleInput(data);
    }
    if (!this.focused && this.workbench.handleInput(data)) return true;
    if (matchesKey(data, "ctrl+b")) {
      if ((this.terminal?.physical.columns ?? 0) >= 105) this.sidebarEnabled = !this.sidebarEnabled;
      else this.expanded = !this.expanded;
      this.terminal?.refreshLayout(); return true;
    }
    if (matchesKey(data, "f6")) { this.activityFocused = false; this.focused = !this.focused; this.refresh(); return true; }
    if (!this.focused) return false;
    if (matchesKey(data, "escape")) { this.focused = false; this.refresh(); return true; }
    if (matchesKey(data, "left") || matchesKey(data, "right")) {
      const index = (PANEL_TABS.indexOf(this.tab) + (matchesKey(data, "right") ? 1 : PANEL_TABS.length - 1)) % PANEL_TABS.length;
      const tab = PANEL_TABS[index]; if (tab) this.choose(tab); return true;
    }
    if (matchesKey(data, "up") || matchesKey(data, "down") || this.navigatingActivity && (data === "j" || data === "k")) { this.move(matchesKey(data, "down") || data === "j" ? 1 : -1); return true; }
    if (data === "o") { void this.workbench.repository(); return true; }
    if (matchesKey(data, "enter")) { void this.inspect(); return true; }
    const keys = getKeybindings();
    if (VIEWPORT_KEYS.some(key => keys.matches(data, key))) return false;
    // Typing/abort shortcuts return to the actual Pi editor; never silently lose pasted input.
    this.focused = false; this.refresh(); return false;
  }
  private openNav(view: MainView): void {
    this.focused = false; this.workbench.setView(view); this.refresh();
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
    pi.on("ui_prompt_start", () => { this.prompts++; this.focused = false; this.refresh(); });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); this.refresh(); });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui;
      this.workbench.bind(ctx.ui);
      this.prompts = 0;
      this.focused = false;
      setPanelAppearance(this.appearance); setCanvasAppearance(this.appearance);
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode(), this.appearance));
      const owner = this;
      ctx.ui.setEditorComponent((tui, theme, keys) => {
        const editor = new class extends sdk.CustomEditor {
          private composer: ReturnType<typeof composerLayout> | undefined;
          private bottomRow = -1;
          dispatchNative(id: "app.model.select" | "app.thinking.cycle"): void { this.actionHandlers.get(id)?.(); }
          setPaddingX(padding: number): void { super.setPaddingX(owner.density === "compact" ? 0 : Math.max(2, padding)); }
          handleInput(data: string): void {
            if (owner.canInteract && matchesKey(data, "f2")) { void owner.palette(); return; }
            // A message sent from another view returns to Chat, so its reply and any refusal notice show.
            if (owner.workbench.view !== "conversation" && this.getText().trim()
                && keys.getKeys("tui.input.submit").some(key => matchesKey(data, key))) {
              owner.workbench.setView("conversation"); owner.workbench.focused = false;
            }
            super.handleInput(data);
          }
          protected renderTopBorder(width: number, hidden: number): string {
            if (hidden) return super.renderTopBorder(width, hidden);
            const followUp = keys.getKeys("app.message.followUp")[0];
            const title = owner.workbench.view === "activity" ? "Message main conversation · agent views are read-only" : composerTitle(owner.options.snapshot());
            const hint = `F2 actions${followUp ? ` · ${followUp} follow-up` : ""}`;
            const label = pad(` ${title}${hint && visibleWidth(title) + visibleWidth(hint) + 7 <= width ? ` · ${hint}` : ""} `,
              Math.max(0, width - 3)).trimEnd();
            return PANEL_MUTED + "┌─" + label + "─".repeat(Math.max(0, width - visibleWidth(label) - 3)) + "┐";
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            const snapshot = owner.options.snapshot();
            const submit = keys.getKeys("tui.input.submit")[0] ?? "Enter";
            this.composer = composerLayout(snapshot, width, hidden, `${submit} ${snapshot?.streaming ? "steer" : "send"}${owner.workbench.view === "activity" ? " to main" : ""}`);
            return this.composer.line;
          }
          handleMouse(event: TuiMouseEvent) {
            if (event.type === "click" && event.button === "left") {
              owner.focused = false; owner.workbench.focused = false; owner.refresh();
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
            return lines.map(line => {
              const framed = line.startsWith(" ") && line.endsWith(" ")
                ? PANEL_MUTED + "│" + PANEL_TEXT + line.slice(1, -1) + PANEL_MUTED + "│" : line;
              return ctx.ui.theme.bg("userMessageBg", pad(framed, width));
            });
          }
        }(tui, theme, keys);
        editor.setPaddingX(2);
        owner.nativeAction = id => editor.dispatchNative(id);
        owner.updateDensity = () => editor.setPaddingX(2);
        owner.nativeKeys = id => keys.getKeys(id).join(", ");
        owner.editorFocused = () => editor.focused;
        owner.requestRender = force => tui.requestRender(force);
        owner.hasOverlay = () => tui.hasOverlay();
        owner.activeSelection = tui instanceof TuiAltScreen ? () => tui.hasActiveSelection() : () => false;
        return editor;
      });
      ctx.ui.setFooter((tui, _theme, data) => {
        const unsubscribe = data.onBranchChange(() => tui.requestRender());
        ctx.ui.setHeader(() => ({
          render: () => [],
          invalidate() {},
        }));
        return {
          render: width => statusBar(this.options.identity, this.options.snapshot(), data.getExtensionStatuses(), width),
          invalidate() {}, dispose: unsubscribe,
        };
      });
      void this.loadFiles();
      ctx.ui.setWidget("corral-session-inspector", (tui): Component => ({
        render: width => this.renderInline(width, tui.terminal.rows),
        handleMouse: (event: TuiMouseEvent) => this.inlineMouse(event),
        invalidate() {},
      }), { placement: "aboveEditor" });
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
      description: "Native session inspector: context, files, plan, evidence, usage, inbox, history, project, refresh or toggle (agents/helpers/jobs focus Context activity)",
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
  private async panelCommand(args: string): Promise<void> {
    const value = args.trim().toLowerCase();
    if (value === "history") { await this.workbench.openHistory(); return; }
    if (value === "project") { await this.workbench.repository(); this.refresh(); return; }
    if (value === "helpers" || value === "agents" || value === "jobs") { this.browseActivity(); return; }
    if (value === "usage") { this.focused = false; this.workbench.setView("usage"); return; }
    if (value === "evidence") { this.focused = false; this.workbench.setView("evidence"); return; }
    if (value === "inbox") { this.focused = false; this.workbench.setView("inbox"); this.refresh(); return; }
    if (value === "refresh") { await this.workbench.inspection.refresh(true); await this.loadFiles(); if (this.workbench.view === "inbox") await this.workbench.inbox.refresh(); return; }
    if (value === "toggle") { this.sidebarEnabled = !this.sidebarEnabled; this.terminal?.refreshLayout(); return; }
    const tab = PANEL_TABS.find(tab => tab.toLowerCase() === value);
    if (tab) this.choose(tab);
    this.focused = true; this.refresh();
  }
  actions(): HostAction[] {
    const scope = `${this.options.identity.role} · ${this.options.identity.project}`;
    const ready = (): string | undefined => !this.ui || !this.nativeAction ? "Interactive editor unavailable" : undefined;
    const action = (id: string, label: string, shortcut: string, run: HostAction["run"], disabled = ready): HostAction =>
      ({ id, label, shortcut, scope, run, disabled });
    const items = [
      ...([ ["context", "Inspect context", "F6"], ["files", "Changed files", "F7"], ["agents", "Agent observations", ""],
        ["jobs", "Activity and jobs", "F8"], ["usage", "Measured usage", ""], ["evidence", "Evidence", "F10"], ["inbox", "Decision inbox", "F11"],
        ["plan", "Plan ledger", ""], ["project", "Choose project/worktree", "o (files)"], ["refresh", "Refresh observations", "r (view)"],
        ["toggle", "Toggle inspector rail", "Ctrl+B"] ] as const).map(([id, label, key]) =>
        action(`/panels ${id}`, label, key, () => this.panelCommand(id))),
      action("app.model.select", "Select model", this.nativeKeys?.("app.model.select") ?? "", () => this.nativeAction?.("app.model.select")),
      action("app.thinking.cycle", "Cycle effort", this.nativeKeys?.("app.thinking.cycle") ?? "", () => this.nativeAction?.("app.thinking.cycle"),
        () => !this.options.snapshot()?.effortAvailable ? "Effort unavailable for current model" : ready()),
      action("inbox.answer-flow", "Inspect established answer flow (not sent)", "a (inbox)", () => {
        this.workbench.setView("inbox"); this.workbench.focused = true; this.workbench.handleInput("a");
      }, () => {
        if (ready()) return ready();
        const role = this.options.identity.role, item = this.workbench.inbox.current;
        if (role !== "pm" && role !== "assistant") return `Role ${role} cannot answer; trusted owner flow required`;
        if (!item?.pending || !item.response) return "No pending selected item with an answer route";
        if (role === "pm" && (!item.pm_eligible || item.scope !== this.options.identity.project)) return "Owner-only or outside PM scope";
        return undefined;
      }),
    ];
    for (const appearance of Object.keys(PACKAGED) as Appearance[]) items.push(action(`appearance.${appearance}`,
      `Appearance: ${appearance}${appearance === this.appearance ? " (current)" : ""}`, "", () => {
        this.appearance = appearance; setPanelAppearance(appearance); setCanvasAppearance(appearance);
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
    this.focused = false;
    const action = await ui.custom<HostAction | undefined>((tui, theme, _keys, done) =>
      new ActionPalette(this.actions(), theme, done, () => tui.requestRender(), Math.max(1, Math.min(8, Math.floor((tui.terminal.rows - 5) / 2)))));
    if (!action || this.disposed) return;
    const reason = action.disabled();
    if (reason) { ui.notify(reason, "warning"); return; }
    try { await action.run(); } catch (error) { ui.notify(plainLabel(String(error)), "error"); }
    this.refresh();
  }
}
