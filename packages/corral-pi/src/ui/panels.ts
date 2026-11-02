// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import {
  getKeybindings, isKeyRelease, matchesKey, Text, TuiAltScreen, visibleWidth, type Component, type Keybinding, type Terminal, type TuiMouseEvent,
} from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { PanelTerminal, PANEL_WIDTH, type PanelSurface } from "./panel-terminal.ts";
import { NativeWorkbench, plainClick } from "./workbench.ts";
import { activityKey, activitySidebar } from "./activity-sidebar.ts";
import { panelTheme } from "./panels-theme.ts";
import { composerLayout, composerTitle, overviewLines, panelFooter, splitRow } from "./panels-chrome.ts";
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
  readonly readTranscript?: (id: string) => Promise<string>;
  readonly snapshot: () => PanelSnapshot | null;
  readonly readInbox?: (signal: AbortSignal) => Promise<unknown>;
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
  tab: PanelTab = "Context";
  focused = false;
  expanded = false;
  offset = 0;
  private terminal: PanelTerminal | undefined;
  private ui: ExtensionUIContext | undefined;
  private requestRender: (() => void) | undefined;
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
  private railPager: PagerHit | undefined;
  private inlinePager: PagerHit | undefined;

  constructor(private readonly options: NativePanelsOptions) {
    this.workbench = new NativeWorkbench({ ...options, canInteract: () => this.canInteract && !this.activeSelection?.(), changed: () => {
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
  }
  private tabs(): string {
    let x = 0;
    this.tabHits = [];
    return PANEL_TABS.map(tab => {
      this.tabHits.push({ from: x, to: x + tab.length, tab });
      x += tab.length + 1;
      return (tab === this.tab ? PANEL_BLUE : PANEL_MUTED) + tab + PANEL_TEXT;
    }).join(" ");
  }
  private lines() { return panelLines(this.tab, this.options.identity, this.options.snapshot(), this.files); }
  private choose(tab: PanelTab): void {
    this.tab = tab;
    this.offset = 0;
    if (tab === "Files") { this.workbench.setView("changes"); if (this.files.kind === "unread") void this.loadFiles(); }
    if (tab === "Agents") this.selectActivity();
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
    this.tab = "Agents"; this.focused = true; this.workbench.focused = false;
    this.sidebarEnabled = true; this.selectActivity();
    this.terminal?.refreshLayout(); this.refresh();
  }
  async loadFiles(): Promise<void> { await this.workbench.review.refresh(); }
  private displayed(): { readonly rows: readonly PanelLine[]; readonly overview: boolean } {
    if (this.wide && this.tab === "Context" && !this.focused) return { rows: overviewLines(this.options.snapshot(), this.files,
      { name: this.workbench.scopeLabel, required: this.workbench.review.scopeRequired },
      this.workbench.selectedActivity && activityKey(this.workbench.selectedActivity), this.historyExpanded), overview: true };
    const rows = this.tab === "Files" ? this.workbench.files() : this.tab === "Agents" ? this.activityRows() : this.lines();
    return { rows: this.wide || this.tab === "Agents" ? rows : rows.filter(line => !line.heading), overview: false };
  }
  private inlineCapacity(rows: number): number {
    if (this.tab === "Agents" && this.focused) return Math.max(2, Math.min(12, Math.floor(rows / 2) - 2));
    return this.focused || this.expanded ? Math.min(this.expanded ? 4 : 2, Math.max(1, Math.floor(rows / 8))) : 0;
  }
  private get capacity(): number {
    const rows = this.terminal?.rows ?? 0;
    return this.wide ? Math.max(0, rows - 5) : this.inlineCapacity(rows);
  }
  private wrapped(width: number): { readonly rows: readonly PanelLine[]; readonly overview: boolean } {
    const { rows, overview } = this.displayed();
    return { overview, rows: rows.flatMap(row => row.text.trim() === "" ? [row]
      : new Text(row.text, 0, 0).render(Math.max(1, width - (row.indent ?? 0) - (row.continuationIndent ?? 0)))
        .map((text, index) => ({ ...row, text, indent: (row.indent ?? 0) + (index ? row.continuationIndent ?? 0 : 0) }))) };
  }
  private page(capacity: number, width: number): { readonly window: PageWindow; readonly pager: string } {
    const { rows, overview } = this.wrapped(width);
    let window: PageWindow;
    if (this.tab === "Agents") {
      this.selectActivity();
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
    if (this.tab === "Agents") {
      this.selectActivity();
      const keys = [...new Set(this.activityRows().map(row => this.actionKey(row.action)).filter(key => key !== undefined))];
      this.activityCursor = keys[Math.max(0, Math.min(keys.length - 1, keys.indexOf(this.activityCursor ?? "") + delta))];
    } else if (this.tab === "Files" && this.files.kind === "ready") {
      const visible = this.workbench.review.visible;
      const index = visible.findIndex(file => file.path === this.workbench.review.selected);
      const file = visible[Math.max(0, Math.min(visible.length - 1, index + delta))];
      if (file) { void this.workbench.openFile(file.path); this.workbench.focused = false; }
    } else if (this.capacity > 0) {
      const { rows, overview } = this.wrapped(this.wide ? PANEL_WIDTH - 4 : Math.max(1, (this.terminal?.columns ?? 80) - 2));
      const current = scrollWindow(rows, this.capacity, overview ? this.overviewOffset : this.offset).offset;
      const next = scrollWindow(rows, this.capacity, current + delta).offset;
      if (overview) this.overviewOffset = next; else this.offset = next;
    }
    this.refresh();
  }
  async inspect(): Promise<void> {
    if (!this.canInteract) return;
    if (this.tab === "Agents") {
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
      const x = Number(mouse[2]) - 1 - this.terminal.columns - 1;
      const button = Number(mouse[1]);
      const release = mouse[4] === "m" || (button & 3) === 3;
      if (mouse[4] === "M" && !(button & 96) && !release) this.pointerOrigin = x >= 0 ? "sidebar" : "main";
      const origin = this.pointerOrigin;
      if (release) this.pointerOrigin = undefined;
      // A main-pane selection must receive its release even when dragged into the rail.
      if (origin === "main") return false;
      if (x >= 0 || origin === "sidebar") {
        if (x >= 0 && this.canInteract && mouse[4] === "M") {
          const button = Number(mouse[1]);
          if (button & 64) this.move(button & 1 ? 3 : -3);
          else if (button === 0) this.click(x - 2, Number(mouse[3]) - 1, false);
        }
        return true; // Physical rail cells must not enter Pi's smaller main viewport.
      }
    }
    if (!this.canInteract || mouse) return false;
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
    if (matchesKey(data, "f6")) { this.focused = !this.focused; this.refresh(); return true; }
    if (!this.focused) return false;
    if (matchesKey(data, "escape")) { this.focused = false; this.refresh(); return true; }
    if (matchesKey(data, "left") || matchesKey(data, "right")) {
      const index = (PANEL_TABS.indexOf(this.tab) + (matchesKey(data, "right") ? 1 : PANEL_TABS.length - 1)) % PANEL_TABS.length;
      const tab = PANEL_TABS[index]; if (tab) this.choose(tab); return true;
    }
    if (matchesKey(data, "up") || matchesKey(data, "down") || this.tab === "Agents" && (data === "j" || data === "k")) { this.move(matchesKey(data, "down") || data === "j" ? 1 : -1); return true; }
    if (data === "o") { void this.workbench.repository(); return true; }
    if (matchesKey(data, "enter")) { void this.inspect(); return true; }
    const keys = getKeybindings();
    if (VIEWPORT_KEYS.some(key => keys.matches(data, key))) return false;
    // Typing/abort shortcuts return to the actual Pi editor; never silently lose pasted input.
    this.focused = false; this.refresh(); return false;
  }
  install(pi: ExtensionAPI, sdk: typeof Sdk): void {
    pi.on("ui_prompt_start", () => { this.prompts++; this.focused = false; this.refresh(); });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); this.refresh(); });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui;
      this.workbench.bind(ctx.ui);
      this.prompts = 0;
      this.focused = false;
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode()));
      const owner = this;
      ctx.ui.setEditorComponent((tui, theme, keys) => {
        const editor = new class extends sdk.CustomEditor {
          private composer: ReturnType<typeof composerLayout> | undefined;
          private bottomRow = -1;
          setPaddingX(padding: number): void { super.setPaddingX(Math.max(2, padding)); }
          protected renderTopBorder(width: number, hidden: number): string {
            if (hidden) return super.renderTopBorder(width, hidden);
            const followUp = keys.getKeys("app.message.followUp")[0];
            const title = owner.workbench.view === "activity" ? "Message main conversation · agent views are read-only" : composerTitle(owner.options.snapshot());
            const hint = followUp ? `${followUp} follow-up` : "";
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
        owner.editorFocused = () => editor.focused;
        owner.requestRender = () => tui.requestRender();
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
          render: width => panelFooter(this.options.snapshot(), this.wide && this.tab === "Context" && !this.focused,
            data.getExtensionStatuses(), width),
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
    pi.on("agent_start", () => { this.refresh(); });
    pi.on("agent_end", () => { this.refresh(); });
    pi.on("turn_end", () => { this.refresh(); });
    pi.on("model_select", () => { this.refresh(); });
    pi.on("thinking_level_select", () => { this.refresh(); });
    pi.on("tool_result", () => { this.workbench.invalidateFiles(); this.refresh(); });
    pi.registerCommand("panels", {
      description: "Native session inspector: context, files, agents, jobs, helpers, plan, evidence, usage, inbox, project, refresh or toggle",
      handler: async (args, ctx) => {
        if (ctx.mode !== "tui" || !this.terminal) { ctx.ui.notify("Native panels require the interactive Corral host.", "warning"); return; }
        const value = args.trim().toLowerCase();
        if (value === "helpers" || value === "agents" || value === "jobs") { this.browseActivity(); return; }
        if (value === "project") { await this.workbench.repository(); this.refresh(); return; }
        if (value === "agents") { this.choose("Agents"); this.focused = false; this.refresh(); return; }
        if (value === "jobs") { this.focused = false; this.workbench.setView("activity"); this.refresh(); return; }
        if (value === "usage") { this.focused = false; this.workbench.setView("usage"); return; }
        if (value === "evidence") { this.focused = false; this.workbench.setView("evidence"); return; }
        if (value === "inbox") { this.focused = false; this.workbench.setView("inbox"); this.refresh(); return; }
        if (value === "refresh") { await this.workbench.inspection.refresh(true); await this.loadFiles(); if (this.workbench.view === "inbox") await this.workbench.inbox.refresh(); return; }
        if (value === "toggle") { this.sidebarEnabled = !this.sidebarEnabled; this.terminal.refreshLayout(); return; }
        const tab = PANEL_TABS.find(tab => tab.toLowerCase() === value);
        if (tab) this.choose(tab);
        this.focused = true; this.refresh();
      },
    });
  }
}
