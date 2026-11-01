// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { isKeyRelease, matchesKey, Text, type Component, type Terminal, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { PanelTerminal, type PanelSurface } from "./panel-terminal.ts";
import { readPanelDiff, readPanelFiles } from "./panels-git.ts";
import { panelTheme } from "./panels-theme.ts";
import { composerBar, overviewLines, panelFooter, panelHeader, splitRow } from "./panels-chrome.ts";
import {
  box, pad, panelLines, plainLabel, diffLine, PANEL_TABS, PANEL_BG, PANEL_BLUE, PANEL_TEXT, PANEL_MUTED, RESET,
  type FilesState, type PanelSnapshot, type PanelTab, type PanelLine,
} from "./panels-view.ts";

export interface NativePanelsOptions {
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly snapshot: () => PanelSnapshot | null;
}
interface TabHit { readonly from: number; readonly to: number; readonly tab: PanelTab }

export class NativePanels implements PanelSurface {
  sidebarEnabled = true;
  tab: PanelTab = "Context";
  focused = false;
  expanded = false;
  offset = 0;
  private terminal: PanelTerminal | undefined;
  private ui: ExtensionUIContext | undefined;
  private requestRender: (() => void) | undefined;
  private editorFocused: (() => boolean) | undefined;
  private hasOverlay: (() => boolean) | undefined;
  private prompts = 0;
  private inspecting = false;
  private pointerOrigin: "main" | "sidebar" | undefined;
  private queued = false;
  private disposed = false;
  private readonly abort = new AbortController();
  private fileIndex = 0;
  private files: FilesState = { kind: "unread" };
  private contentRows: readonly PanelLine[] = [];
  private tabHits: TabHit[] = [];

  constructor(private readonly options: NativePanelsOptions) {}
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
    this.abort.abort();
    this.ui = undefined;
    this.requestRender = undefined;
    this.editorFocused = undefined;
    this.hasOverlay = undefined;
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
    if (tab === "Files" && this.files.kind === "unread") void this.loadFiles();
    this.refresh();
  }
  async loadFiles(): Promise<void> {
    if (this.files.kind === "loading") return;
    const previous = this.files.kind === "ready" ? this.files.files[this.fileIndex]?.path : undefined;
    this.files = { kind: "loading" }; this.refresh();
    try {
      const files = await readPanelFiles(this.options.cwd, this.abort.signal);
      if (this.disposed) return;
      this.files = { kind: "ready", files };
      const index = files.findIndex(file => file.path === previous);
      this.fileIndex = index >= 0 ? index : 0;
    } catch (error) {
      if (this.disposed) return;
      this.files = { kind: "error", reason: plainLabel(error instanceof Error ? error.message : String(error)).slice(0, 240) };
    }
    this.refresh();
  }
  private content(capacity: number): string[] {
    const lines = this.wide && this.tab === "Context" && !this.focused
      ? overviewLines(this.options.snapshot(), this.files)
      : this.lines().filter(line => this.wide || !line.heading);
    if (this.tab === "Files" && this.files.kind === "ready" && this.files.files.length) {
      const files = this.files.files;
      const start = Math.max(0, Math.min(this.fileIndex, files.length - capacity));
      this.contentRows = files.slice(start, start + capacity).map(file => ({ text: file.path, action: { kind: "diff", path: file.path } }));
      return files.slice(start, start + capacity).map((file, i) =>
        (start + i === this.fileIndex ? PANEL_BLUE + "› " : PANEL_MUTED + "  ") +
        plainLabel(file.status + " " + file.path) + PANEL_TEXT);
    }
    const start = Math.max(0, Math.min(this.offset, lines.length - capacity));
    this.contentRows = lines.slice(start, start + capacity);
    return this.contentRows.map(row =>
      (row.heading ? PANEL_BLUE : PANEL_MUTED) + row.text + PANEL_TEXT);
  }
  renderSidebar(width: number, height: number): readonly string[] {
    const lines = [this.tabs(), "", ...this.content(Math.max(0, height - 5))];
    while (lines.length < height - 3) lines.push("");
    lines.push(PANEL_MUTED + (this.focused ? "←/→ tabs · ↑/↓ · Enter · Esc" : "F6 inspect · Ctrl+B inline") + PANEL_TEXT);
    return box("Session", lines, width, height, this.focused);
  }
  renderInline(width: number, rows: number): string[] {
    if (this.wide) return [];
    const capacity = this.focused || this.expanded ? Math.min(this.expanded ? 4 : 2, Math.max(1, Math.floor(rows / 8))) : 0;
    const navigation = splitRow(PANEL_TEXT + " " + this.tabs(), PANEL_MUTED + (this.focused ? "↑↓ Enter · Esc" : "F6 inspect"), width);
    return [PANEL_BG + navigation + RESET,
      ...this.content(capacity).map(line => PANEL_BG + pad("  " + line, width) + RESET)];
  }
  private move(delta: number): void {
    if (this.tab === "Files" && this.files.kind === "ready") {
      this.fileIndex = Math.max(0, Math.min(this.files.files.length - 1, this.fileIndex + delta));
    } else this.offset = Math.max(0, Math.min(Math.max(0, this.lines().length - 1), this.offset + delta));
    this.refresh();
  }
  async inspect(): Promise<void> {
    if (!this.ui || !this.canInteract || this.inspecting) return;
    this.inspecting = true;
    try {
    if (this.tab === "Files") {
      if (this.files.kind === "unread" || this.files.kind === "error") { await this.loadFiles(); return; }
      if (this.files.kind === "ready") {
        const file = this.files.files[this.fileIndex];
        if (file) {
          const ui = this.ui;
          try {
            const diff = await readPanelDiff(this.options.cwd, file, this.abort.signal);
            if (this.canInteract && this.tab === "Files" && this.files.kind === "ready"
              && this.files.files[this.fileIndex]?.path === file.path) {
              await this.detail(file.path + " · HEAD → working tree", diff.split("\n"), "diff");
            }
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
    if (y === (inline ? 0 : 1)) {
      const tab = this.tabHits.find(hit => x >= hit.from && x < hit.to)?.tab;
      if (tab) { if (inline) this.focused = true; this.choose(tab); }
      return;
    }
    const row = this.contentRows[y - (inline ? 1 : 3)];
    if (!row) return;
    const action = row.action;
    if (!action) { if (this.tab !== "Files") void this.inspect(); return; }
    switch (action.kind) {
      case "diff": {
        if (this.files.kind !== "ready") return;
        const index = this.files.files.findIndex(file => file.path === action.path);
        if (index < 0) return; // A refresh may have removed the displayed path.
        this.choose("Files"); this.fileIndex = index; this.refresh(); void this.inspect(); return;
      }
      case "files": this.focused = true; this.choose("Files"); return;
      default: {
        const unreachable: never = action;
        throw new Error(`Unknown native panel action: ${String(unreachable)}`);
      }
    }
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
    if (matchesKey(data, "ctrl+b")) {
      if ((this.terminal?.physical.columns ?? 0) >= 105) this.sidebarEnabled = !this.sidebarEnabled;
      else this.expanded = !this.expanded;
      this.terminal?.refreshLayout(); return true;
    }
    if (matchesKey(data, "f6")) { this.focused = !this.focused; this.refresh(); return true; }
    if (!this.focused) return false;
    if (matchesKey(data, "escape")) { this.focused = false; this.refresh(); return true; }
    if (matchesKey(data, "left") || matchesKey(data, "right")) {
      const index = (PANEL_TABS.indexOf(this.tab) + (matchesKey(data, "right") ? 1 : 3)) % PANEL_TABS.length;
      const tab = PANEL_TABS[index]; if (tab) this.choose(tab); return true;
    }
    if (matchesKey(data, "up") || matchesKey(data, "down")) { this.move(matchesKey(data, "down") ? 1 : -1); return true; }
    if (matchesKey(data, "enter")) { void this.inspect(); return true; }
    // Typing/abort shortcuts return to the actual Pi editor; never silently lose pasted input.
    this.focused = false; this.refresh(); return false;
  }
  install(pi: ExtensionAPI, sdk: typeof Sdk): void {
    pi.on("ui_prompt_start", () => { this.prompts++; this.focused = false; this.refresh(); });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); this.refresh(); });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui;
      this.prompts = 0;
      this.focused = false;
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode()));
      const owner = this;
      ctx.ui.setEditorComponent((tui, theme, keys) => {
        const editor = new class extends sdk.CustomEditor {
          setPaddingX(padding: number): void { super.setPaddingX(Math.max(2, padding)); }
          protected renderTopBorder(width: number, hidden: number): string {
            if (hidden) return super.renderTopBorder(width, hidden);
            return PANEL_MUTED + "┌─ Message " + "─".repeat(Math.max(0, width - 12)) + "┐";
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            return composerBar(owner.options.snapshot(), width, hidden);
          }
          handleMouse(event: TuiMouseEvent) {
            if (event.type === "click" && event.button === "left") { owner.focused = false; owner.refresh(); }
            return super.handleMouse(event);
          }
          render(width: number): string[] {
            this.borderColor = text => ctx.ui.theme.fg("borderAccent", text);
            return super.render(width).map(line => {
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
        return editor;
      });
      ctx.ui.setFooter((tui, _theme, data) => {
        const unsubscribe = data.onBranchChange(() => tui.requestRender());
        ctx.ui.setHeader(() => ({
          render: width => panelHeader(this.options.identity, this.options.cwd, data.getGitBranch(), this.options.snapshot(), width),
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
        handleMouse: (event: TuiMouseEvent) => {
          if (!this.canInteract) return undefined;
          if (event.type === "wheel") { this.move(event.wheelDelta ?? 0); return { handled: true }; }
          if (event.type === "click" && event.button === "left") {
            this.click(event.x - 1, event.y, true); return { handled: true };
          }
          return undefined;
        },
        invalidate() {},
      }), { placement: "aboveEditor" });
      this.refresh();
    });
    pi.on("agent_start", () => { this.refresh(); });
    pi.on("agent_end", () => { this.refresh(); });
    pi.on("turn_end", () => { this.refresh(); });
    pi.on("model_select", () => { this.refresh(); });
    pi.on("tool_result", () => { this.refresh(); });
    pi.registerCommand("panels", {
      description: "Native session inspector: context, files, agents, plan, refresh or toggle",
      handler: async (args, ctx) => {
        if (ctx.mode !== "tui" || !this.terminal) { ctx.ui.notify("Native panels require the interactive Corral host.", "warning"); return; }
        const value = args.trim().toLowerCase();
        if (value === "refresh") { await this.loadFiles(); return; }
        if (value === "toggle") { this.sidebarEnabled = !this.sidebarEnabled; this.terminal.refreshLayout(); return; }
        const tab = PANEL_TABS.find(tab => tab.toLowerCase() === value);
        if (tab) this.choose(tab);
        this.focused = true; this.refresh();
      },
    });
  }
}
