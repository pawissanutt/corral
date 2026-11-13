// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation } from "@earendil-works/pi-coding-agent";
import { isKeyRelease, matchesKey, TuiAltScreen, visibleWidth, type Component, type Terminal, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { PanelTerminal, PANEL_WIDTH, RAIL_MIN, RAIL_MAX, MIN_MAIN, type PanelSurface } from "./panel-terminal.ts";
import { ACTIVITY_VIEWS, NativeWorkbench, type MainView } from "./workbench.ts";
import { navRows, navSections, renderNav, type NavSource } from "./nav-panel.ts";
import type { HistoryRead } from "./history.ts";
import type { ReviewBases } from "./review-store.ts";
import type { JobControl } from "../host/job-controls.ts";
import type { QueueControl } from "../host/queue-control.ts";
import { panelTheme } from "./panels-theme.ts";
import { type Appearance, type Density, PACKAGED } from "./appearance.ts";
import { ActionPalette, type HostAction } from "./action-palette.ts";
import { setCanvasAppearance } from "./panel-canvas.ts";
import { composerLayout, composerTitle, statusBar } from "./panels-chrome.ts";
import { pad, plainLabel, setPanelAppearance, PANEL_TEXT, PANEL_MUTED, type PanelSnapshot } from "./panels-view.ts";

export interface NativePanelsOptions {
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
/** `/panels <target>` names for main views; history, project, refresh and toggle are actions. */
const PANEL_VIEWS: Readonly<Record<string, MainView>> = {
  chat: "conversation", files: "changes", changes: "changes", activity: "activity", jobs: "activity", helpers: "activity",
  agents: "agents", usage: "usage", evidence: "evidence", inbox: "inbox",
};

/** Who this pane is, in place of a generic heading: the task and role, the assistant, or the PM. */
function railHeading(identity: HostIdentity): string {
  const role = identity.role.replaceAll("_", " ");
  if (identity.task) return `${identity.task} · ${role}`;
  if (identity.role === "assistant") return identity.assistant ? `Assistant · ${identity.assistant}` : "Assistant";
  if (identity.role === "pm") return identity.project ? `PM · ${identity.project}` : "PM";
  return role[0]?.toUpperCase() + role.slice(1);
}
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
    this.workbench = new NativeWorkbench({ ...options, railVisible: () => this.wide,
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
  /** Navigation plus the live plan and running work; detail lives in the main views. */
  renderSidebar(width: number, height: number): readonly string[] {
    const { identity } = this.options, wb = this.workbench;
    const source: NavSource = { taskBound: wb.task.bound, proof: !!identity.task && !!identity.run,
      snapshot: this.options.snapshot(), review: wb.review, inspection: wb.inspection, inbox: wb.inbox, view: wb.view };
    const frame = renderNav(navRows(source), view => view === wb.view || view === "activity" && ACTIVITY_VIEWS.has(wb.view),
      width, height, this.drag ? `⇔ ${width} cols` : railHeading(identity), navSections(source));
    this.navHits = frame.hits;
    return frame.lines;
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
          const view = this.navHits.get(Number(mouse[3]) - 1);
          if (view) this.openNav(view);
        }
        return true; // Physical rail cells must not enter Pi's smaller main viewport.
      }
    }
    if (this.drag && matchesKey(data, "escape")) { const start = this.drag.start; this.drag = undefined; this.resizeRail(start); this.refresh(); return true; }
    if (!this.canInteract || mouse) return false;
    if (this.wide && (matchesKey(data, "alt+,") || matchesKey(data, "alt+."))) {
      this.resizeRail((this.terminal?.railWidth ?? this.sidebarWidth) + (matchesKey(data, "alt+.") ? 2 : -2)); return true;
    }
    if (this.workbench.handleInput(data)) return true;
    if (matchesKey(data, "ctrl+b")) { this.sidebarEnabled = !this.sidebarEnabled; this.terminal?.refreshLayout(); return true; }
    return false;
  }
  private openNav(view: MainView): void {
    this.workbench.setView(view); this.refresh();
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
            return PANEL_MUTED + "──" + label + "─".repeat(Math.max(0, width - visibleWidth(label) - 2));
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            const snapshot = owner.options.snapshot();
            const submit = keys.getKeys("tui.input.submit")[0] ?? "Enter";
            this.composer = composerLayout(snapshot, width, hidden, `${submit} ${snapshot?.streaming ? "steer" : "send"}${owner.workbench.view === "activity" ? " to main" : ""}`);
            return this.composer.line;
          }
          handleMouse(event: TuiMouseEvent) {
            if (event.type === "click" && event.button === "left") {
              owner.workbench.focused = false; owner.refresh();
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
      description: "Native views: files, activity, agents, usage, evidence, inbox, history, project, refresh or toggle",
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
    if (value === "refresh") { await this.workbench.inspection.refresh(true); await this.loadFiles(); if (this.workbench.view === "inbox") await this.workbench.inbox.refresh(); return; }
    if (value === "toggle") { this.sidebarEnabled = !this.sidebarEnabled; this.terminal?.refreshLayout(); return; }
    const view = value === "task" && this.workbench.task.bound ? "task" : PANEL_VIEWS[value];
    if (view) { this.openNav(view); return; }
    this.ui?.notify(`/panels ${plainLabel(value) || "(empty)"}: choose files, activity, agents, usage, evidence, inbox, history, project, refresh or toggle`, "warning");
  }
  actions(): HostAction[] {
    const scope = `${this.options.identity.role} · ${this.options.identity.project}`;
    const ready = (): string | undefined => !this.ui || !this.nativeAction ? "Interactive editor unavailable" : undefined;
    const action = (id: string, label: string, shortcut: string, run: HostAction["run"], disabled = ready): HostAction =>
      ({ id, label, shortcut, scope, run, disabled });
    const items = [
      ...([ ["files", "Changed files", "F7"], ["activity", "Activity and jobs", "F8"], ["agents", "Agent observations", ""],
        ["usage", "Measured usage", ""], ["evidence", "Evidence", "F10"], ["inbox", "Decision inbox", "F11"],
        ["project", "Choose project/worktree", "o (changes)"], ["refresh", "Refresh observations", "r (view)"],
        ["toggle", "Toggle side panel", "Ctrl+B"] ] as const).map(([id, label, key]) =>
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
    const action = await ui.custom<HostAction | undefined>((tui, theme, _keys, done) =>
      new ActionPalette(this.actions(), theme, done, () => tui.requestRender(), Math.max(1, Math.min(8, Math.floor((tui.terminal.rows - 5) / 2)))));
    if (!action || this.disposed) return;
    const reason = action.disabled();
    if (reason) { ui.notify(reason, "warning"); return; }
    try { await action.run(); } catch (error) { ui.notify(plainLabel(String(error)), "error"); }
    this.refresh();
  }
}
