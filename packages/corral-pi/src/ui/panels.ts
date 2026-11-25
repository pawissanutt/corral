import type { PlanDocument } from "./plan-page.ts";
import type { TranscriptRead } from "./agent-history.ts";
// First-party native panels over the existing InteractiveMode, never a second agent/editor loop.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext, InteractiveChatPresentation, InteractiveModeOptions, SelectorComponent } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, isKeyRelease, matchesKey, TuiAltScreen, visibleWidth, type Component, type OverlayHandle, type Terminal, type TUI, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { LIVE_CHILD_STATES, type BuildUi, type FleetControl } from "../build/types.ts";
import { PanelTerminal, PANEL_BREAKPOINT, PANEL_WIDTH, RAIL_MIN, RAIL_MAX, MIN_MAIN, type PanelSurface } from "./panel-terminal.ts";
import { NativeWorkbench, boardUnavailable, plainClick, RULE_ROWS, type MainView } from "./workbench.ts";
import type { HistoryRead } from "./history.ts";
import type { ReviewBases } from "./review-store.ts";
import type { JobControl } from "../host/job-controls.ts";
import type { QueueControl } from "../host/queue-control.ts";
import type { MarkSink } from "../host/transcript-marks.ts";
import type { RemoteDialogs } from "../host/remote/dialogs.ts";
import { hiddenThinkingLabel, panelTheme, thinkingLead, userLabel } from "./panels-theme.ts";
import { type Appearance, type Bell, type Density, type Motion, PACKAGED } from "./appearance.ts";
import { Activity, ActivityClock, elapsedLabel, PassCache, SYSTEM_SCHEDULER, type Scheduler, type Ticker } from "./activity.ts";
import { ActionPalette, type ActionGroup, type HostAction } from "./action-palette.ts";
import { setCanvasAppearance } from "./panel-canvas.ts";
import { agentColor, setAgentHueAppearance } from "./agent-hues.ts";
import { statusBar } from "./panels-chrome.ts";
import { titleText, type TitleAttention } from "./chrome.ts";
import { composerBar, composerBottom, composerRow, composerTitle, composerTop, shellDisposition, type ComposerHit } from "./composer-chrome.ts";
import { chromeTier, COMPACT_COLUMNS, SHORT_ROWS, type Notice } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, keyLabel, piKey, queueFocusKey, targetKey, type FooterHit, type Hint, type HintState, type KeyContext } from "./keymap.ts";
import { KeysList } from "./keys-list.ts";
import { rule, setMotion } from "./frame.ts";
import { QueueDock } from "./queue-dock.ts";
import { Sidebar, type SideHit, type SideTarget } from "./sidebar.ts";
import { runRewind, type RewindCall, type TranscriptBranch } from "./rewind-picker.ts";
import { openPager, type PagerLine } from "./pager.ts";
import { DialogAdoption, ownerDialogPresentation, type OwnerDialogHost } from "./owner-dialog.ts";
import { statusPresenter } from "./notice-route.ts";
import { quietNote } from "./mark-rows.ts";
import { effortPicker, modelPicker, optionPicker } from "./model-picker.ts";
import type { FavoriteModels } from "../host/model-favorites.ts";
import { settingsCard } from "./settings-card.ts";
import { SettingsPage, settingsKeys, type PersonalSettings, type SettingsControl } from "./settings-view.ts";
import { ownerBashPresenter } from "./owner-bash.ts";
import { planLedger } from "./plan-detail.ts";
import type { PlanSurface } from "./todo.ts";
import { plainLabel, setPanelAppearance, type PanelSnapshot } from "./panels-view.ts";
import { sessionCost } from "./usage-model.ts";
import { BuildController, childQuestionCard } from "./build/build-controller.ts";
import { BUILD_BOARD_ENTRY, BUILD_EXIT_ENTRY, BUILD_FINISH_ENTRY, buildEntryRenderer, type BuildEntry } from "./build/build-entries.ts";
import { choiceRows, PASTE_START } from "./build/choice-dock.ts";
import { ComposerPaste, IMAGE_MARKER, StagedImages, type PasteBase } from "./composer-attach.ts";
import { childIdOf, type BuildCardSource } from "./build/contract.ts";
import { steerable } from "./build/fleet-rows.ts";
import { QuestionDock, questionsKey, type QuestionSource } from "./owner-questions.ts";
import { sidebarKeyLabel, type SidebarKey } from "./herdr-keys.ts";
import { EscGuard } from "./esc-guard.ts";
import { AttentionSignal } from "./attention.ts";
import { noticeGlyph, toastFrame } from "./toast.ts";

export interface NativePanelsOptions {
  /** Whether the selected model takes image input; without it images are never staged. */
  readonly imageInput?: () => { readonly accepts: boolean; readonly model: string };
  /** Whether the session is compacting: Pi then holds a submitted draft as text only, so a draft with images waits. */
  readonly compacting?: () => boolean;
  readonly planDocument?: () => PlanDocument | undefined;
  readonly openPlanDocument?: () => void;
  readonly build?: BuildUi;
  /** The child runtime of a session that is not a build session; the Agents page steers, cancels, answers and re-models its children through it. Read when used: the host may build it after the panels. */
  readonly fleetControl?: FleetControl;
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly artifacts?: () => Promise<unknown>;
  /** `corral artifact show` into a private file, for the Artifacts view's byte-exact text and image preview. */
  readonly artifactFile?: (reference: string) => Promise<string>;
  /** The host's in-process rewind (workers with turn checkpoints); F2 offers the rewind picker while it is given. */
  readonly rewind?: RewindCall;
  readonly exportSession?: () => Promise<string>;
  readonly readTranscript?: TranscriptRead;
  readonly jobControl?: JobControl;
  readonly queueControl?: QueueControl;
  readonly readTask?: () => Promise<unknown>;
  readonly snapshot: () => PanelSnapshot | null;
  readonly readHistory?: HistoryRead;
  readonly projectDir?: string;
  readonly readProjects?: (signal: AbortSignal) => Promise<unknown>;
  readonly canRenderTool?: (name: string) => boolean;
  readonly reviewBases?: ReviewBases;
  /** The host's delivery of composer text to the current child target; false when the child or the session cannot take it. */
  readonly deliverBuildInput?: (text: string) => Promise<boolean>;
  /** Time and interval source of the working indicator's clock and the Esc guard; tests inject one. */
  readonly scheduler?: Scheduler;
  /** The sidebar toggle the host chose at startup (`sidebarToggleKey()`: Alt+B when herdr's prefix is Ctrl+B); Ctrl+B by default. */
  readonly sidebarKey?: SidebarKey;
  /** Where the build controller appends transcript marks; absent outside build sessions or before the host wires it. */
  readonly marks?: MarkSink;
  /**
   * The session's dialog registry: owner dialogs register there and the build dock decides through it, so the app can
   * answer the same card and whichever side answers first closes the other's. Absent: dialogs answer only here.
   */
  readonly dialogs?: RemoteDialogs;
  /** The workspace's favorite models, which the model picker lists first and marks; absent: no favorites. */
  readonly modelFavorites?: FavoriteModels;
  /** The owner's saved settings: `/settings` and F2 Settings… open the Settings page on them; absent: Pi's settings card. */
  readonly settings?: SettingsControl;
  /** The saved personal Display defaults this terminal opens with; F2 Display changes them for this session only. */
  readonly personal?: PersonalSettings;
}
/** Views a tool row's clickable label can open by name; `plan` is the sidebar's Plan tab. */
export type PanelView = "chat" | "agents" | "changes" | "task" | "proof" | "artifacts" | "usage" | "board" | "plan";
/** `/panels <target>` names for main views; history, project, refresh and toggle are actions. */
const PANEL_VIEWS: Readonly<Record<string, MainView>> = {
  chat: "conversation", files: "changes", changes: "changes", agents: "agents", activity: "agents", jobs: "agents", helpers: "agents",
  usage: "usage", evidence: "evidence", proof: "evidence", task: "task", timeline: "timeline", events: "timeline", board: "plan", plan: "plan",
};
/** Views that read the bound task; sessions without one do not offer them. */
const TASK_VIEWS: Readonly<Partial<Record<MainView, true>>> = { task: true, evidence: true };
const PANEL_TARGETS = "chat, agents, changes, plan, task, proof, usage, events, history, project, refresh or toggle";

/** Where keys go when no Pi dialog or overlay is open: Pi's editor, the main view, the sidebar, the queue, the owner questions or a docked build dialog. */
export type PanelFocus = "composer" | "main" | "sidebar" | "queue" | "questions" | "choice";
/** Where the owner is, compared around an Esc to tell whether it navigated. */
interface Place { readonly key: string; readonly dialog: string | undefined; readonly chat: boolean }
/** One F2 row in its group; `disabled` defaults to the editor being ready, `current` marks a Display setting's value. */
type ActionMaker = (group: ActionGroup, id: string, label: string, shortcut: string, run: HostAction["run"],
  disabled?: () => string | undefined, current?: boolean) => HostAction;

/** The session's build controls, with a steer to the current target delivered through the host, which refuses it once the session closes or unbinds. */
function routedBuild(build: BuildUi, deliver: ((text: string) => Promise<boolean>) | undefined): BuildUi {
  if (!deliver) return build;
  const fleet = build.fleet;
  const routed: FleetControl = {
    snapshot: () => fleet.snapshot(),
    steer: (id, text) => id === build.target() ? deliver(text) : fleet.steer(id, text),
    cancel: id => fleet.cancel(id), setModel: (id, model) => fleet.setModel(id, model), answer: (id, text) => fleet.answer(id, text),
    models: () => fleet.models(), profiles: () => fleet.profiles(), subscribe: listener => fleet.subscribe(listener),
    message: (from, to, text, options) => fleet.message(from, to, text, options), roster: () => fleet.roster(), wait: (ids, signal) => fleet.wait(ids, signal),
  };
  return { fleet: routed, exit: build.exit, record: () => build.record(), ...(build.board ? { board: build.board } : {}),
    target: () => build.target(), setTarget: target => build.setTarget(target) };
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
  /** Rows of the dock widget above the composer (or the dialog card in its place) in the last frame. */
  private dockRows = 0;
  /** The drawer was opened by F6 and closes when the sidebar gives up the focus. */
  private drawerByKey = false;
  /** A dialog or pager the panels opened (Pi raises a prompt for it too): its footer shows its own keys. */
  private overlay: "palette" | "keys" | "pager" | undefined;
  appearance: Appearance = "dark";
  density: Density = "comfortable";
  motion: Motion = "animate";
  bell: Bell = "off";
  /** The key that collapses or expands the sidebar; every cap draws it, and only it toggles. */
  readonly sidebarToggle: SidebarKey;
  /** The owner-question card as a dock item, once the host attached its questions. */
  readonly questionDock: QuestionDock;
  private questions: QuestionSource | undefined;
  private unsubscribeQuestions: (() => void) | undefined;
  /** After an Esc that navigated, a further Esc this soon is ignored instead of stopping the turn. */
  private readonly escGuard: EscGuard;
  /** The terminal title and the bell, from the session's attention state; set once the session starts. */
  private attention: AttentionSignal | undefined;
  /** A reopen notice raised by dismissing a docked dialog, cleared once that dialog's subject has an outcome. */
  private dismissal: { readonly notice: Notice; readonly settled: () => boolean } | undefined;
  /** The shared redraw clock: rows that keep changing after their tool returned (a running job, a detached cell) hold it. */
  readonly ticker: Ticker;
  /** What the main agent is doing, for the composer's working indicator and the header's state word. */
  private readonly activity: Activity;
  private readonly clock: ActivityClock;
  /** The host snapshot, built once per render pass however many surfaces read it. */
  private readonly snapshots: PassCache<PanelSnapshot | null>;
  private releaseLive: (() => void) | undefined;
  private nativeAction: ((id: "app.model.select" | "app.thinking.cycle") => void) | undefined;
  private nativeKeys: ((id: "app.model.select" | "app.thinking.cycle") => string) | undefined;
  /** The keys of Pi's clipboard paste (app.clipboard.pasteImage), read from the composer's keybindings once it exists. */
  private nativePasteKeys: () => readonly string[] = () => ["ctrl+v"];
  /** The composer's current draft, once it exists. */
  private draftText: () => string = () => "";
  /** Images pasted into the draft, sent with the message that marks them. */
  readonly stagedImages = new StagedImages();
  private pasteRouter: ComposerPaste | undefined;
  private get composerPaste(): ComposerPaste {
    return this.pasteRouter ??= new ComposerPaste({
      images: this.stagedImages, imageInput: () => this.options.imageInput?.() ?? { accepts: false, model: "This host" },
      notice: (text, tone) => this.workbench.notice(text, tone), pasteKeys: () => this.nativePasteKeys(),
    });
  }
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
  /** The Settings page over the main view, when the host offers saved settings. */
  private readonly settingsPage: SettingsPage | undefined;
  private readonly unsubscribeBuild: readonly (() => void)[] = [];
  private pi: ExtensionAPI | undefined;
  /** The session's transcript, read for the words that started each turn in the rewind picker. */
  private sessionManager: TranscriptBranch | undefined;
  /** Entries the controller posted before Pi was installed. */
  private readonly posts: BuildEntry<unknown>[] = [];
  /** The next owner dialog answers a card the app already shows (a fleet child's question) instead of registering one. */
  private readonly adoption = new DialogAdoption();
  /** `/todo`'s hold on the sidebar's Plan tab; see `planSurface`. */
  private readonly planView: PlanSurface = {
    show: () => this.showSidebarPlan(),
    scroll: delta => { this.showSidebarPlan(); this.sidebar.page(delta, this.workbench.sideSource()); this.refresh(); },
    notice: (text, tone) => this.workbench.notice(text, tone),
  };
  constructor(private readonly options: NativePanelsOptions) {
    const scheduler = options.scheduler ?? SYSTEM_SCHEDULER;
    // The saved personal defaults are this terminal's starting Display; session_start applies them with the theme.
    const personal = options.personal;
    if (personal) { this.appearance = personal.appearance; this.density = personal.density; this.motion = personal.motion; this.bell = personal.bell; }
    this.settingsPage = options.settings ? new SettingsPage({
      control: options.settings, role: options.identity.role, session: () => this.display(),
      apply: patch => this.applyDisplay(patch), changed: () => this.refresh(),
    }) : undefined;
    this.snapshots = new PassCache(() => options.snapshot());
    this.activity = new Activity(() => scheduler.now());
    this.clock = new ActivityClock(scheduler, () => this.activity.busy(), () => this.refresh(), () => this.motion === "still");
    this.ticker = this.clock;
    this.sidebarToggle = options.sidebarKey ?? "ctrl+b";
    this.sidebar = new Sidebar(() => this.refresh());
    this.sidebar.toggleKey = sidebarKeyLabel(this.sidebarToggle);
    this.escGuard = new EscGuard(() => scheduler.now());
    this.questionDock = new QuestionDock({
      source: () => this.questions, focus: () => this.focusQuestions(), focusComposer: () => this.focusComposer(), changed: () => this.refresh(),
    });
    const build = options.build ? routedBuild(options.build, options.deliverBuildInput) : undefined;
    const panels = this;
    this.build = build ? new BuildController({
      ui: build, state: () => this.snapshot()?.build, streaming: () => this.snapshot()?.streaming ?? false,
      overlay: () => !this.canInteract, draft: () => this.ui?.getEditorText() ?? "", setDraft: text => this.ui?.setEditorText(text),
      focusComposer: () => this.focusComposer(), focusDock: () => this.focusChoice(),
      notice: (text, tone) => this.workbench.notice(text, tone), post: entry => this.post(entry),
      showPlan: () => { this.workbench.setView("plan"); }, openChild: id => this.workbench.openChild(id),
      color: agentColor, now: () => Date.now(), changed: () => this.refresh(), ...(options.marks ? { marks: options.marks } : {}),
      get dialogs() { return panels.attachedDialogs(); },
    }) : undefined;
    this.queue = new QueueDock({
      items: () => this.snapshot()?.queue ?? [], control: () => this.options.queueControl,
      draft: () => this.ui?.getEditorText() ?? "", setDraft: text => this.ui?.setEditorText(text),
      focusComposer: () => this.focusComposer(), notice: (text, tone) => this.workbench.notice(text, tone),
      changed: () => this.refresh(),
      // A build session's queue names the parent as every item's receiver: children never take queued input.
      ...(build ? { receiver: () => "parent" } : {}),
    });
    this.workbench = new NativeWorkbench({ ...options, openLedger: step => { void this.showLedger(step); }, ...(build && this.build ? { build, buildController: this.build } : {}), fleet: () => this.options.fleetControl,
      ...(this.settingsPage ? { settingsPage: this.settingsPage } : {}),
      childCards: {
        watch: (id, askedAt, settled) => this.attachedDialogs()?.watch(childQuestionCard(id, askedAt), settled),
        adopt: <T>(bind: (close: () => void) => void, open: () => T): T => this.adoption.with(bind, open),
      },
      snapshot: () => this.snapshot(),
      terminal: () => this.terminal ? { columns: this.terminal.physical.columns, rows: this.terminal.rows } : undefined,
      onFocusMain: () => { this.sidebar.focused = false; this.queue.focused = false; this.blurDocks(); this.closeKeyDrawer(); },
      canInteract: () => this.canInteract && !this.activeSelection?.(), changed: () => this.refresh(),
      modal: () => !this.disposed && (this.prompts > 0 || this.hasOverlay?.() === true),
      rail: {
        width: () => this.wide && this.terminal ? this.terminal.railWidth : undefined,
        render: (width, height) => this.renderSidebar(width, height),
        placed: (top, height) => { this.railTop = top; this.railRows = height; },
      },
      mark: () => {
        // A missed start or end event is squared with the session's own flag once per frame.
        this.activity.reconcile(this.snapshot()?.streaming ?? false);
        this.clock.sync();
        return this.activity.headerMark(this.clock.now());
      },
      live: live => this.liveRows(live) });
    if (build) {
      const sync = () => { this.build?.sync(); this.refresh(); };
      this.unsubscribeBuild = [build.fleet.subscribe(sync), build.exit.subscribe(sync), ...(build.board ? [build.board.subscribe(sync)] : [])];
    }
  }
  mainView(transcript: Component, presentation?: () => InteractiveChatPresentation): Component { return this.workbench.mount(transcript, presentation); }
  /**
   * The SDK presentation hooks of the rich host, assembled here and nowhere else: the owner dialog card takes the
   * composer's place for `ui.select`, `ui.confirm` and `ui.input`, and Pi's model, effort and settings selectors draw
   * as the same card; Pi's status, warning and error lines become notices (and quiet rows for errors); an owner
   * `!command` draws as a tool row. Plain and headless hosts pass none and keep Pi's own.
   */
  presentation(): Pick<InteractiveModeOptions, "dialogPresentation" | "statusPresentation" | "selectorPresentation" | "bashPresentation"> {
    const host: OwnerDialogHost = {
      rows: () => this.terminal?.rows ?? 24, chromeRows: rows => this.cardChrome(rows), changed: () => this.refresh(), adoption: this.adoption,
      ...(this.options.dialogs ? { dialogs: this.options.dialogs, notice: (text: string) => this.workbench.notice(text, "text") } : {}),
    };
    return {
      dialogPresentation: ownerDialogPresentation(host),
      // Read per notice: before the panels have a terminal, or once disposed, Pi keeps its own line.
      statusPresentation: statusPresenter({
        sink: () => this.terminal && !this.disposed ? this.workbench : undefined,
        inChat: () => this.workbench.view === "conversation",
        row: quietNote,
      }),
      selectorPresentation: {
        model: request => modelPicker(request, host, this.options.modelFavorites),
        thinking: request => effortPicker(request, host),
        // `/settings` opens the Settings page on the host's saved settings, not Pi's session settings; Pi's nested
        // lists (a `title`) and hosts without saved settings keep Pi's settings in the card.
        settings: request => request.title === undefined && this.settingsPage ? this.showSettings(() => request.cancel()) : settingsCard(request, host),
        options: request => optionPicker(request, host),
      },
      bashPresentation: ownerBashPresenter({ cwd: this.options.cwd, ticker: () => this.disposed ? undefined : this.ticker }),
    };
  }
  /**
   * The Settings page over the main view, and the component that holds Pi's focus in the composer's place for it
   * (Pi keeps the draft in its editor and restores it as that component closes). Esc at the page's top or its back
   * line calls `finish`, which closes the component; however Pi closes it, the view under the page shows again.
   * Undefined (Pi's own settings) before the panels have a terminal, and once disposed.
   */
  private showSettings(finish: () => void): SelectorComponent | undefined {
    const page = this.settingsPage;
    if (!page || !this.terminal || this.disposed) return undefined;
    let open = true;
    const left = () => {
      if (!open) return;
      open = false;
      page.closed();
      this.workbench.closeSettings();
      this.refresh();
    };
    this.workbench.openSettings();
    page.open(() => {
      // The focused SDK selector sees Esc after the panel input router has returned.
      // Treat leaving Settings like the other owned views before Pi restores its editor.
      this.escGuard.arm();
      left();
      finish();
    });
    this.refresh();
    return settingsKeys(page, left);
  }
  /** F2 Settings…: the same page, its key row in the composer's place through `ui.custom`. */
  private async openSettings(): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    await ui.custom<void>((_tui, _theme, _keys, done) => {
      const keys = this.showSettings(() => done());
      if (!keys) done();
      return keys ?? { render: () => [], invalidate() {} };
    });
  }
  /** This terminal's Display values now. */
  private display(): PersonalSettings {
    return { appearance: this.appearance, density: this.density, motion: this.motion, bell: this.bell };
  }
  /** This terminal's Display: F2 changes it for this session; a saved personal setting changes it too. */
  private applyDisplay(patch: Partial<PersonalSettings>): void {
    if (this.disposed) return;
    const { appearance, density, motion, bell } = patch;
    if (appearance !== undefined) {
      this.appearance = appearance; setPanelAppearance(appearance); setCanvasAppearance(appearance); setAgentHueAppearance(appearance);
      if (this.ui && this.themeCtor) this.ui.setTheme(panelTheme(this.themeCtor, this.ui.theme.getColorMode(), appearance));
      this.requestRender?.(true);
    }
    if (density !== undefined) { this.density = density; this.updateDensity?.(); }
    if (motion !== undefined) { this.motion = motion; setMotion(motion); this.clock.sync(); }
    if (bell !== undefined) this.bell = bell;
    this.refresh();
  }
  /** The Plan presentation `/todo` drives; undefined until the panels have a terminal, and once disposed. */
  planSurface(): PlanSurface | undefined { return this.terminal && !this.disposed ? this.planView : undefined; }
  /**
   * `/todo`: the sidebar's Plan tab takes the keys, in the rail or, where none shows (narrow, or the rail hidden), in
   * the drawer it opens as F6 does, so Enter on the plan or a step opens the whole ledger. While a dialog or popup
   * holds the keys only the tab changes.
   */
  private showSidebarPlan(): void {
    if (this.disposed) return;
    this.sidebar.tab = "plan";
    if (this.canInteract) this.focusSidebar();
    this.refresh();
  }
  /** The whole plan ledger in the shared pager, opened at a step's entry (the top for the plan itself), in every role. */
  private async showLedger(step: string | undefined): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    const ledger = planLedger(this.workbench.planSource());
    this.overlay = "pager"; this.refresh();
    try { await openPager(ui, { title: ledger.title, lines: ledger.lines, start: ledger.startOf(step), wrap: true }); }
    finally { this.overlay = undefined; this.refresh(); }
  }
  /** The host snapshot for this render pass. */
  private snapshot(): PanelSnapshot | null { return this.snapshots.get(); }
  /** While a helper, child or job runs, the shared clock keeps their spinners and elapsed times moving. */
  private liveRows(live: boolean): void {
    if (live) this.releaseLive ??= this.clock.hold("panels:live-rows");
    else { this.releaseLive?.(); this.releaseLive = undefined; }
  }
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
    if (this.questionDock.focused && this.questionDock.waiting) return "questions";
    return this.queue.focused ? "queue" : this.sidebar.focused ? "sidebar" : this.workbench.focused ? "main" : "composer";
  }
  /** The owner questions the host asks on the panels' behalf; their card becomes a dock item. */
  attachQuestions(source: QuestionSource): void {
    this.unsubscribeQuestions?.();
    this.questions = source;
    source.useDialogs?.(() => this.attachedDialogs());
    this.unsubscribeQuestions = source.subscribe(() => this.refresh());
    this.refresh();
  }
  /**
   * The session's dialog registry while the panels draw on a terminal. Only then is there a second surface to race the
   * app: a headless session (panels without a terminal) never watches or answers its cards, so its replies stay its own.
   */
  private attachedDialogs(): RemoteDialogs | undefined { return this.terminal ? this.options.dialogs : undefined; }
  layoutChanged(): void { this.refresh(); }
  refresh(): void {
    // State changed: later reads in this job rebuild the snapshot.
    this.snapshots.clear();
    if (this.queued || this.disposed) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      if (this.disposed) return;
      this.settleDismissal();
      this.syncToast();
      this.attention?.update(this.attentionState());
      this.requestRender?.();
    });
  }
  /**
   * What the session needs from the owner, for the terminal title: a docked or pending build choice, a child's
   * question, a waiting owner question or a dialog the agent opened; else working while a turn runs or, as the
   * header says, while a build child is still live; else idle.
   */
  private attentionState(): TitleAttention {
    const build = this.build, ui = this.options.build, snapshot = this.snapshot();
    const needs = this.activity.phase() === "owner" || !!build?.dialog() || (build?.questions().length ?? 0) > 0
      || ui?.exit.state().phase === "pending" || !!ui?.board?.proposed()
      || (this.questions?.waiting().some(question => question.status === "pending") ?? false);
    if (needs) return "needs-you";
    const children = snapshot?.build?.fleet.children ?? snapshot?.fleet?.children ?? [];
    return this.activity.active || snapshot?.streaming || children.some(child => LIVE_CHILD_STATES.includes(child.state)) ? "working" : "idle";
  }
  /** A reopen notice whose subject has since had its outcome (the exit chosen, the question answered) is stale. */
  private settleDismissal(): void {
    const dismissal = this.dismissal;
    if (!dismissal) return;
    if (this.workbench.currentNotice !== dismissal.notice) { this.dismissal = undefined; return; }
    if (!dismissal.settled()) return;
    this.dismissal = undefined;
    this.workbench.clearNotice(dismissal.notice);
  }
  /**
   * A notice the header has no room for as a toast at the top right of the main view, sized from its wrapped
   * text. It never takes the focus; a new notice replaces it and it leaves when the notice expires. The header's
   * own frame decides each render whether it drew the notice, and the toast shows only when it did not.
   */
  private syncToast(): void {
    const notice = this.workbench.currentNotice, tui = this.tui, terminal = this.terminal;
    if (this.toast?.notice === notice) return;
    this.toast?.handle.hide(); this.toast = undefined;
    if (!notice || !tui || !terminal) return;
    const main = terminal.mainColumns, frame = toastFrame(notice, main);
    const handle = tui.showOverlay({ render: () => [...frame.lines], invalidate() {} }, {
      row: this.workbench.bodyTop(), col: Math.max(0, main - 1 - frame.width), width: frame.width, nonCapturing: true,
      visible: () => !this.probing && !this.workbench.noticeInHeader,
    });
    this.toast = { handle, notice };
  }
  dispose(): void {
    this.disposed = true;
    this.releaseLive = undefined; this.clock.dispose();
    this.toast?.handle.hide(); this.toast = undefined; this.tui = undefined;
    for (const unsubscribe of this.unsubscribeBuild) unsubscribe();
    this.unsubscribeQuestions?.(); this.unsubscribeQuestions = undefined; this.questions = undefined; this.attention = undefined;
    this.build?.dispose();
    this.workbench.dispose();
    this.ui = undefined; this.pi = undefined; this.sessionManager = undefined;
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
  /** Back to Pi's editor: the main view, the sidebar, the queue and the dock items give up the focus. */
  focusComposer(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false; this.blurDocks();
    this.closeKeyDrawer(); this.refresh();
  }
  /** The docked build dialog and the owner-question card give up the keys; both stay docked. */
  private blurDocks(): void {
    if (this.build) this.build.dock.focused = false;
    this.questionDock.focused = false;
  }
  /** A docked build dialog takes the keys; it already marked itself focused. */
  private focusChoice(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false; this.questionDock.focused = false;
    this.closeKeyDrawer(); this.refresh();
  }
  /** The owner-question card takes the keys. */
  private focusQuestions(): void {
    this.sidebar.focused = false; this.queue.focused = false; this.workbench.focused = false;
    if (this.build) this.build.dock.focused = false;
    this.questionDock.focused = true;
    this.closeKeyDrawer(); this.refresh();
  }
  /** F6: focus the sidebar (opening the drawer when there is no rail), or the next tab once it has the focus. */
  private sidebarKey(): void {
    if (this.sidebar.focused) this.sidebar.nextTab();
    else this.focusSidebar();
    this.refresh();
  }
  /** The sidebar takes the keys; without a rail its drawer opens, and closes again when the sidebar gives them up. */
  private focusSidebar(): void {
    this.queue.focused = false; this.workbench.focused = false; this.blurDocks(); this.sidebar.focused = true;
    if (!this.wide && !this.sidebar.drawer) { this.sidebar.drawer = true; this.drawerByKey = true; }
  }
  /** The queue takes the focus with the last editable item selected; false when nothing is editable. */
  private focusQueue(): boolean {
    if (!this.queue.focusLast()) return false;
    this.sidebar.focused = false; this.workbench.focused = false; this.blurDocks(); this.closeKeyDrawer(); this.refresh();
    return true;
  }
  /**
   * Alt+↓: the next dock item takes the keys, nearest the composer first (the queue while it holds input of
   * yours, the owner questions, a docked build dialog); after the last one the composer has them again. False
   * when the dock holds nothing that takes keys.
   */
  private nextDockItem(): boolean {
    const items: PanelFocus[] = [
      ...(this.editableQueue() ? ["queue" as const] : []),
      ...(this.questionDock.waiting ? ["questions" as const] : []),
      ...(this.build?.dialog() ? ["choice" as const] : []),
    ];
    if (!items.length) return false;
    const at = items.indexOf(this.focus), next = at < 0 ? items[0] : items[at + 1];
    if (next === "queue") return this.focusQueue();
    if (next === "questions") this.focusQuestions();
    else if (next === "choice" && this.build) { this.build.dock.focused = true; this.focusChoice(); }
    else this.focusComposer();
    return true;
  }
  /** The sidebar key: where the rail fits it hides or returns; elsewhere the collapsed row opens or closes its drawer. */
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
    // A build child waiting for an answer docks its question; every other agent opens its conversation, a job Agents.
    const id = this.build && target.kind === "agent" ? childIdOf(target.key) : undefined;
    const asking = id !== undefined && this.snapshot()?.build?.fleet.children.some(child => child.id === id && child.state === "needs");
    if (id !== undefined && asking) this.build?.open("question", id);
    else if (target.kind === "agent") this.workbench.openConversation(target.key);
    else if (target.kind === "job") this.workbench.openAgents(target.key);
    else if (target.kind === "file") void this.workbench.openFile(target.path);
    else if (target.kind === "scope") void this.workbench.repository();
    else if (target.kind === "plan") void this.showLedger(target.step);
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
   * One widget above the editor, in the mock's dock order: a docked build dialog, then the owner questions, then
   * the queue, then (only without a rail and without a docked dialog) the collapsed sidebar row or its drawer, each
   * with its own rule. The question card and the queue count in the dialog's row budget. While a dialog holds the
   * keys the owner-question card steps aside (the dialog may be that very question; the questions stay pending), and
   * in a short terminal the sidebar row and drawer give the card their rows; all return as it closes.
   */
  private dock(): Component {
    let choiceRowsShown = 0, questionRows = 0, queueRows = 0;
    let sideHits: ReadonlyMap<number, readonly SideHit[]> = new Map();
    return {
      render: width => {
        const rows = this.terminal?.rows ?? 24, short = rows < SHORT_ROWS, dialog = this.footerContext() === "dialog";
        this.queue.setCapacity(rows);
        const queue = this.queue.render(width);
        // Short terminals give each question one row and list two at most.
        this.questionDock.setLimit(short ? 4 : 15, short);
        const questions = dialog ? [] : this.questionDock.render(width);
        let choice: string[] = [];
        if (this.build?.dialog()) {
          this.build.dock.setLimit(choiceRows(rows, this.chromeRows(rows, questions.length + queue.length)), short);
          choice = this.build.dock.render(width);
        }
        let side: readonly string[] = [];
        sideHits = new Map();
        if (!this.wide && !choice.length && !(short && dialog)) {
          const source = this.workbench.sideSource();
          const frame = this.sidebar.drawer ? this.sidebar.drawerFrame(source, width, rows)
            : this.sidebar.row(source, width, chromeTier(this.terminal?.physical.columns ?? width) === "tiny", rows);
          side = frame.lines; sideHits = frame.hits;
        }
        choiceRowsShown = choice.length; questionRows = questions.length; queueRows = queue.length;
        const lines = [...choice, ...questions, ...queue, ...side];
        this.dockRows = lines.length;
        return lines;
      },
      handleMouse: event => {
        if (event.y < choiceRowsShown) return this.canInteract ? this.build?.dock.handleMouse(event) : { handled: true };
        const inQuestions = event.y - choiceRowsShown;
        if (inQuestions < questionRows) return this.canInteract ? this.questionDock.handleMouse({ ...event, y: inQuestions }) : { handled: true };
        const y = inQuestions - questionRows;
        if (y >= queueRows) {
          const row = y - queueRows;
          if (this.canInteract && plainClick(event)) this.sideClick(sideHits.get(row)?.find(hit => event.x >= hit.from && event.x < hit.to), false);
          return { handled: true };
        }
        if (!this.canInteract) return undefined;
        const result = this.queue.handleMouse({ ...event, y });
        if (this.queue.focused) { this.sidebar.focused = false; this.workbench.focused = false; this.blurDocks(); this.closeKeyDrawer(); this.refresh(); }
        return result;
      },
      invalidate() {},
    };
  }
  /**
   * Rows the rest of the screen keeps while a dialog is docked: header (one row when short), tabs and their
   * rule, the composer box (its bar folds away when short), the status bar, the footer and its rule, and the
   * other dock items (the owner questions and the queue).
   */
  private chromeRows(rows: number, dockRows: number): number {
    const header = rows < SHORT_ROWS ? 1 : 2;
    const rules = rows >= RULE_ROWS ? 2 : 0;
    return header + 1 + (rows < SHORT_ROWS ? 3 : 4) + this.statusRows + 1 + rules + dockRows;
  }
  /**
   * Rows the rest of the screen keeps while a dialog card stands in the composer's place: the header, the tab row and
   * its rule as last drawn, what the dock above the card still shows (a docked build dialog, the queue, a sidebar row),
   * the status bar, and the footer row with its rule unless a short terminal reclaimed them.
   */
  private cardChrome(rows: number): number {
    const context = this.footerContext();
    const hints = contextHints(this.workbench.hintState(context, this.queueState(), this.questionDock.waiting));
    const footer = this.footerKept(rows, context, hints) ? 1 + (rows >= RULE_ROWS ? 1 : 0) : 0;
    return this.workbench.bodyTop() + this.dockRows + this.statusRows + footer;
  }
  /**
   * Rows a dialog card (its key row included) may take in the composer's place: the terminal's rows less what
   * `cardChrome` keeps (the old composer not counted), the same figures the owner dialog card's host reports. Any rows
   * the main view keeps are the card's own budget to leave.
   */
  dialogRows(): number {
    const rows = this.terminal?.rows ?? 24;
    return Math.max(0, rows - this.cardChrome(rows));
  }
  /**
   * A short terminal draws no empty footer row (a dialog card draws its own keys), nor the rule above it. The footer
   * and the card's row budget both ask this, so they agree on the rows.
   */
  private footerKept(rows: number, context: KeyContext, hints: readonly Hint[]): boolean {
    return rows >= SHORT_ROWS || footerBase(context) || hints.length > 0;
  }
  private queueState(): HintState["queue"] {
    const items = this.snapshot()?.queue ?? [];
    return { items: items.length, editable: items.filter(item => item.editable).length, control: this.queue.controllable };
  }
  /**
   * Whose keys the footer lists. What the panels opened comes first, since Pi reports it as a prompt as well;
   * then a Pi dialog, then another overlay; then anything Pi put in the composer's place, which holds Pi's focus
   * there (a dialog card, the answer card, a selector), announced by a prompt event or not; then the focused surface.
   */
  private footerContext(): KeyContext {
    if (this.overlay) return this.overlay;
    if (this.workbench.pagerOpen) return "pager";
    if (this.prompts > 0) return "dialog";
    if (this.hasOverlay?.()) return "pager";
    if (this.editorFocused?.() === false) return "dialog";
    const focus = this.focus;
    return focus === "main" ? this.workbench.mainContext() : focus;
  }
  /**
   * Runs a key as if typed, after the current input or click has been handled, in the encoding the active
   * keyboard protocol accepts (the F1 rows and footer hints carry legacy bytes).
   */
  private replay(data: string): void {
    queueMicrotask(() => { if (!this.disposed) this.terminal?.replayKey(data); });
  }
  /**
   * Every key and pointer event reaches the panels first. Esc right after an Esc that navigated is ignored, so the
   * habit of pressing Esc to get out never also stops the turn; Esc reaches Pi's interrupt only from Chat with the
   * composer focused. Any other key ends the guard.
   */
  handleInput(data: string): boolean {
    if (isKeyRelease(data)) return false;
    if (!matchesKey(data, "escape")) {
      if (!data.startsWith("\x1b[<")) this.escGuard.clear();
      return this.route(data);
    }
    if (this.canInteract && this.escGuard.blocking) return true;
    // Esc that closes the owner's own palette, keys list or pager navigates too.
    const own = !this.canInteract && (this.overlay !== undefined || this.workbench.pagerOpen);
    const before = this.place(), handled = this.route(data), after = this.place();
    if (own || after.key !== before.key) this.escNavigated(before, after);
    return handled;
  }
  /** Where the owner is: the view, its context, the focus, a docked dialog and the drawer. */
  private place(): Place {
    const dialog = this.build?.dialog()?.key, focus = this.focus;
    return {
      key: [this.workbench.view, this.workbench.mainContext(), focus, dialog ?? "", this.sidebar.drawer].join("|"),
      dialog, chat: this.workbench.view === "conversation" && focus === "composer",
    };
  }
  /**
   * An Esc navigated: the guard starts. A dismissed build dialog says how to reopen it; landing in Chat while a
   * turn runs says that Esc now stops it.
   */
  private escNavigated(before: Place, after: Place): void {
    this.escGuard.arm();
    if (before.dialog && !after.dialog) { this.reopenNotice(before.dialog); return; }
    if (before.chat || !after.chat || !(this.activity.active || this.snapshot()?.streaming)) return;
    const stop = piKey("app.interrupt");
    this.workbench.notice(`Back in Chat · ${stop ? keyLabel(stop) : "esc"} again stops the turn`, "text");
  }
  /** After Esc on a docked build dialog: what stays waiting and how to reopen it, until its subject has an outcome. */
  private reopenNotice(key: string): void {
    const ui = this.options.build;
    if (!ui) return;
    const [kind, id = ""] = key.split(":");
    const [text, settled]: [string, () => boolean] = kind === "exit"
      ? ["No exit chosen. Nothing was landed · F2 Finish reopens it", () => ui.exit.state().phase !== "pending"]
      : kind === "question"
        ? [`${id} keeps waiting · F2 Answer ${id}'s question reopens it`, () => !this.build?.questions().some(child => child.id === id)]
        : ["Nothing filed. The plan still waits · F10 Plan, then p, reopens it", () => !ui.board?.proposed()];
    this.workbench.notice(text, "text");
    const notice = this.workbench.currentNotice;
    if (notice) this.dismissal = { notice, settled };
  }
  private route(data: string): boolean {
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
    // Pasted text is typing wherever the keys were: the composer takes it unchanged, so a later Enter sends the draft
    // instead of picking a docked option, editing a queued item or acting in a view.
    if (data.startsWith(PASTE_START) && this.focus !== "composer") { this.focusComposer(); return false; }
    // A docked build dialog keeps ↑↓, Enter, digits and Esc; page keys scroll the open view. Typing leaves it docked
    // and reaches the composer; F1, F2, view keys and Alt chords continue below with the dialog still docked.
    if (this.focus === "choice") {
      if (this.build?.dock.handleKey(data)) return true;
      if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) return this.workbench.scrollPage(data);
      if (this.focus !== "choice") { this.refresh(); return false; }
    }
    if (this.focus === "questions") {
      if (this.questionDock.handleKey(data)) return true;
      if (this.focus !== "questions") return false;
    }
    if (matchesKey(data, "f1")) { void this.showKeys(); return true; }
    if (matchesKey(data, "f6")) { this.sidebarKey(); return true; }
    if (matchesKey(data, this.sidebarToggle)) { this.toggleSidebar(); return true; }
    if (this.wide && (matchesKey(data, "alt+,") || matchesKey(data, "alt+."))) {
      this.resizeRail((this.terminal?.railWidth ?? this.sidebarWidth) + (matchesKey(data, "alt+.") ? 2 : -2)); return true;
    }
    const receiverKey = this.build ? targetKey() : undefined;
    if (receiverKey && matchesKey(data, receiverKey)) { this.build?.cycleTarget(); return true; }
    const focusKey = queueFocusKey();
    if (focusKey && matchesKey(data, focusKey) && this.nextDockItem()) return true;
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
    // Pi keeps one Markdown transformer per extension (a second registration replaces the first), so the owner's `you`
    // label and the thinking lead share one; each leaves every other message type unchanged.
    pi.registerMarkdownTransformer((markdown, context) => thinkingLead(userLabel(markdown, context), context),
      { messageTypes: ["user", "assistant-thinking"] });
    if (this.build) {
      const entries = buildEntryRenderer(() => this.build);
      for (const type of [BUILD_EXIT_ENTRY, BUILD_BOARD_ENTRY, BUILD_FINISH_ENTRY]) pi.registerMessageRenderer(type, entries);
    }
    // The working indicator follows the run: each change restarts or stops the shared clock and redraws once.
    const observe = (change: () => void) => { change(); this.clock.sync(); this.refresh(); };
    pi.on("tool_execution_start", event => {
      // The latest `build_finish` call's card shows live exit progress; older cards read as superseded.
      if (this.build && event.toolName === "build_finish" && event.parentToolCallId === undefined) this.build.noteFinish(event.toolCallId);
      observe(() => this.activity.toolStart(event.toolCallId, event.toolName, event.parentToolCallId));
    });
    pi.on("tool_execution_end", event => { observe(() => this.activity.toolEnd(event.toolCallId)); });
    // Panel overlays (the palette, the keys list, a pager) are the owner's own prompts, not the agent waiting on the owner.
    pi.on("ui_prompt_start", () => {
      this.prompts++;
      const own = this.overlay !== undefined || this.workbench.pagerOpen;
      // A Pi dialog needs the rows an F6-opened drawer holds; the owner's own popups keep it.
      if (!own && this.drawerByKey) { this.closeKeyDrawer(); this.sidebar.focused = false; }
      observe(() => this.activity.promptStart(own));
    });
    pi.on("ui_prompt_end", () => { this.prompts = Math.max(0, this.prompts - 1); observe(() => this.activity.promptEnd()); });
    pi.on("before_provider_request", () => { observe(() => this.activity.request()); });
    // Stream events only mark that data arrived; Pi redraws the transcript for them itself.
    pi.on("provider_stream_event", () => { this.activity.data(); });
    pi.on("message_update", event => {
      const update = event.assistantMessageEvent;
      const block = "partial" in update && "contentIndex" in update ? update.partial.content[update.contentIndex] : undefined;
      this.activity.update(update.type, block?.type === "toolCall" ? block.name : undefined);
      this.clock.sync();
    });
    pi.on("message_end", event => {
      const message = event.message;
      if (message.role === "assistant") observe(() => this.activity.messageEnd(message.stopReason));
    });
    pi.on("session_before_compact", () => { observe(() => this.activity.compactStart()); });
    pi.on("session_compact", () => { observe(() => this.activity.compactEnd()); });
    pi.on("session_compact_failed", () => { observe(() => this.activity.compactEnd()); });
    pi.on("agent_settled", event => {
      const ended = this.activity.settled(event.aborted);
      this.clock.sync(); this.refresh();
      // Away from Chat a long turn's end also raises a toast; in Chat the indicator says it.
      if (ended && this.workbench.view !== "conversation" && ended.elapsed > 10_000) {
        this.workbench.notice(`Turn ${ended.aborted ? "stopped" : "finished"} · ${elapsedLabel(ended.elapsed)}`, ended.aborted ? "warning" : "success");
      }
    });
    pi.on("session_start", (_event, ctx) => {
      if (ctx.mode !== "tui" || !this.terminal) return;
      this.ui = ctx.ui; this.sessionManager = ctx.sessionManager;
      this.attention = new AttentionSignal({ title: text => ctx.ui.setTitle(text), bell: () => this.terminal?.bell() },
        titleText(this.options.identity), () => this.bell === "on");
      this.workbench.bind(ctx.ui);
      this.prompts = 0;
      setPanelAppearance(this.appearance); setCanvasAppearance(this.appearance); setAgentHueAppearance(this.appearance); setMotion(this.motion);
      ctx.ui.setTheme(panelTheme(sdk.Theme, ctx.ui.theme.getColorMode(), this.appearance));
      ctx.ui.setHiddenThinkingLabel(hiddenThinkingLabel());
      // The composer's working indicator (activity.ts) replaces Pi's loader row above the composer.
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
              // What Enter would send: pi-tui's `[paste #n …]` placeholders expanded, as Pi's own submit expands them, so a
              // folded paste that starts with `/` is a command for Pi and the child gets what was pasted.
              const text = this.getExpandedText();
              if (keys.getKeys("tui.input.submit").some(key => matchesKey(data, key)) && text.trim() && !text.trimStart().startsWith("/")) {
                if (owner.stagedImages.count > 0 && new RegExp(IMAGE_MARKER.source).test(this.getText())) {
                  owner.workbench.notice("Images go to the parent only; choose the parent as the receiver to send them", "warning"); return;
                }
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
            // Pi holds a draft submitted during compaction as text only and sends it later; its images would be lost.
            if (owner.options.compacting?.() && owner.stagedImages.count > 0 && new RegExp(IMAGE_MARKER.source).test(this.getText())
                && keys.getKeys("tui.input.submit").concat(keys.getKeys("app.message.followUp")).some(key => matchesKey(data, key))) {
              owner.workbench.notice("Images can be sent once compaction finishes; the draft is kept", "warning"); return;
            }
            // Pastes: an image or image file becomes `[Image #n]`, long text folds (composer-attach.ts).
            if (owner.composerPaste.handle(data, this.pasteBase())) return;
            super.handleInput(data);
          }
          insertTextAtCursor(text: string): void {
            if (!owner.composerPaste.inserted(text, this.pasteBase())) super.insertTextAtCursor(text);
          }
          /** The stock editor's own operations, past these overrides. */
          private pasteBase(): PasteBase {
            return {
              input: data => super.handleInput(data), insert: text => super.insertTextAtCursor(text),
              clipboard: () => this.onPasteImage?.(), text: () => this.getText(), cursor: () => this.getCursor(), lines: () => this.getLines(),
            };
          }
          protected renderTopBorder(width: number, hidden: number): string {
            this.hiddenAbove = hidden;
            // The receiver stays at every width (it decides where Enter goes); the input disposition clips first, then the indicator.
            // A draft Pi runs as an owner `!command` names the shell instead, unless a child takes Enter (it gets the text).
            const columns = owner.terminal?.physical.columns ?? width, target = owner.build?.composerTarget(), stop = piKey("app.interrupt");
            // Judged on the expanded draft, as Pi's submit sees it: a folded paste that starts with `!` runs in the shell.
            const shell = owner.build?.target() ? undefined : shellDisposition(this.getExpandedText());
            const status = owner.activity.indicator(columns, owner.clock.now(), stop ? keyLabel(stop) : undefined);
            return composerTop(composerTitle(columns < COMPACT_COLUMNS ? null : owner.snapshot(), target, shell), width, this.accent,
              hidden ? `↑ ${hidden} more` : "", status, visibleWidth(composerTitle(null, target, shell)));
          }
          protected renderBottomBorder(width: number, hidden: number): string {
            if ((owner.terminal?.rows ?? SHORT_ROWS) < SHORT_ROWS) {
              this.composer = undefined;
              this.bottom = composerBottom(width, this.accent, hidden ? `↓ ${hidden} more` : "");
            } else {
              this.composer = composerBar(owner.snapshot(), width, hidden, this.accent);
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
        owner.nativePasteKeys = () => keys.getKeys("app.clipboard.pasteImage");
        owner.draftText = () => editor.getText();
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
          // The status bar, a rule (not below RULE_ROWS rows), then one row of the focused surface's keys ending with F1;
          // a short terminal drops an empty key row and its rule (`footerKept`).
          render: width => {
            const snapshot = this.snapshot(), cost = this.build ? sessionCost(snapshot) : undefined;
            const rows = this.terminal?.rows ?? RULE_ROWS;
            // The composer's model bar folds away when short; the status bar then carries model and effort.
            const folded = rows < SHORT_ROWS ? { model: snapshot?.model ?? null, ...(snapshot ? { effort: snapshot.effort } : {}) } : undefined;
            const status = statusBar(this.options.identity, cost && snapshot ? { ...snapshot, cost: cost.total } : snapshot,
              data.getExtensionStatuses(), width, cost?.partial ?? false, folded);
            const context = this.footerContext();
            const hints = contextHints(this.workbench.hintState(context, this.queueState(), this.questionDock.waiting));
            this.statusRows = status.length;
            if (!this.footerKept(rows, context, hints)) { footerAt = -1; hits = []; return status; }
            const row = footerRow(hints, chromeTier(this.terminal?.physical.columns ?? width), width, footerBase(context));
            const ruled = rows >= RULE_ROWS;
            footerAt = status.length + (ruled ? 1 : 0); hits = row.hits;
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
    pi.on("input", event => {
      if (this.workbench.view !== "conversation") { this.workbench.setView("conversation"); this.workbench.focused = false; this.refresh(); }
      // The owner's staged images go with the message they are marked in (a prompt, a steer or a follow-up).
      if (event.source !== "interactive" || this.stagedImages.count === 0) return { action: "continue" as const };
      const taken = this.stagedImages.take(event.text, this.draftText());
      if (taken.dropped) {
        const one = taken.dropped === 1;
        this.workbench.notice(`${taken.dropped} ${one ? "image was" : "images were"} not sent: ${one ? "its marker was" : "their markers were"} removed`, "warning");
      }
      return taken.images.length
        ? { action: "transform" as const, text: taken.text, images: [...(event.images ?? []), ...taken.images] }
        : { action: "continue" as const };
    });
    pi.on("agent_start", () => { observe(() => this.activity.agentStart()); });
    pi.on("agent_end", () => { observe(() => this.activity.agentEnd()); });
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
      description: "Search permitted host actions and session appearance, density and motion (F2)",
      handler: async (_args, ctx) => {
        if (ctx.mode !== "tui" || !this.terminal) { ctx.ui.notify("Actions require native panels; plain UI is unchanged.", "warning"); return; }
        await this.palette();
      },
    });
  }
  private editableQueue(): boolean {
    return (this.snapshot()?.queue ?? []).some(item => item.editable);
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
  /** Pi's own model selector, as its key opens it. False when the interactive editor is not running. */
  selectModel(): boolean {
    if (!this.ui || !this.nativeAction || this.disposed) return false;
    this.nativeAction("app.model.select");
    return true;
  }
  /** `/board`: the Plan page, when the interactive panels are running. */
  showPlan(): void {
    if (!this.ui || this.disposed) return;
    this.workbench.setView("plan");
  }
  /**
   * Opens a view by name, for a tool row's clickable label. `select` picks inside it: an Agents row key (`job:<id>`,
   * `helper:<request>`; a bare id reads as a job) or a Changes file path. `plan` opens the Plan page, or the
   * sidebar's Plan tab in a task-bound session. A view this session lacks says why in a
   * notice and changes nothing.
   */
  openView(view: PanelView, select?: string): void {
    if (this.disposed) return;
    const taskBound = !!this.options.identity.task;
    const missing = (view === "task" || view === "proof" || view === "artifacts") && !taskBound ? "this session has no bound task"
      : view === "board" && taskBound ? "this session has a bound task" : undefined;
    if (missing) { this.workbench.notice(`Cannot open ${view}: ${missing}`, "warning"); return; }
    switch (view) {
      case "chat": this.workbench.chat(); break;
      case "agents": this.workbench.openAgents(select === undefined || select.includes(":") ? select : `job:${select}`); break;
      case "changes": if (select) void this.workbench.openFile(select); else this.workbench.setView("changes"); break;
      case "proof": this.workbench.setView("evidence"); break;
      case "artifacts": this.workbench.setView("artifacts"); break;
      case "task": case "usage": this.workbench.setView(view); break;
      case "board": this.workbench.setView("plan"); break;
      case "plan":
        if (!taskBound) { this.workbench.setView("plan"); break; }
        this.sidebar.tab = "plan";
        if ((this.terminal?.physical.columns ?? 0) < PANEL_BREAKPOINT) { this.sidebar.drawer = true; this.drawerByKey = false; }
        else if (!this.sidebarEnabled) { this.sidebarEnabled = true; this.terminal?.refreshLayout(); }
        break;
    }
    this.refresh();
  }
  /** Conversation entries join without starting a turn; while the parent streams Pi adds them after its turn. */
  private post(entry: BuildEntry<unknown>): void {
    if (this.pi && this.ui) this.pi.sendMessage(entry, { triggerTurn: false });
    else this.posts.push(entry);
  }
  private async panelCommand(args: string): Promise<void> {
    const value = args.trim().toLowerCase();
    if (value === "history") { await this.workbench.openHistory(); return; }
    if (value === "project") { await this.workbench.repository(); this.refresh(); return; }
    if (value === "refresh") { await this.workbench.task.refresh(true); await this.loadFiles(); return; }
    if (value === "toggle") { this.toggleSidebar(); return; }
    const view = PANEL_VIEWS[value];
    if (view === "plan" && this.options.identity.task) { this.workbench.notice(`/panels ${value}: this session has a bound task`, "warning"); return; }
    if (view && (!TASK_VIEWS[view] || this.options.identity.task)) {
      if (view === "agents") this.workbench.openAgents(); else if (view === "conversation") this.workbench.chat(); else this.workbench.setView(view);
      this.refresh(); return;
    }
    if (view) { this.workbench.notice(`/panels ${value}: this session has no bound task`, "warning"); return; }
    this.workbench.notice(`/panels ${plainLabel(value) || "(empty)"}: choose ${PANEL_TARGETS}`, "warning");
  }
  /**
   * F2's actions in UID §13's group order (Views, Inspect, Message, Model, Workspace, Display, Help); each keeps the
   * reason it cannot run right now, shortened to the cause. A Display row marks its current value.
   */
  actions(): HostAction[] {
    const identity = this.options.identity, taskBound = !!identity.task, rewind = identity.role === "worker" ? this.options.rewind : undefined;
    const scope = `${identity.role} · ${identity.project}`;
    const ready = (): string | undefined => !this.ui || !this.nativeAction ? "interactive editor unavailable" : undefined;
    const queued = (): string | undefined => ready() ?? (this.editableQueue() ? undefined : "no queued input");
    const action: ActionMaker = (group, id, label, shortcut, run, disabled = ready, current) =>
      ({ id, label, shortcut, scope, group, run, disabled, ...(current === undefined ? {} : { current }) });
    const panel = (group: ActionGroup, id: string, label: string, shortcut = "") => action(group, `/panels ${id}`, label, shortcut, () => this.panelCommand(id));
    const dequeue = piKey("app.message.dequeue"), focus = queueFocusKey();
    const build = this.buildActions(action, ready);
    const items: HostAction[] = [
      panel("views", "chat", "Chat", "F9"),
      panel("views", "agents", "Agents", "F8"),
      panel("views", "changes", "Changes", "F7"),
      ...(taskBound ? [panel("views", "task", "Task", "F5"), panel("views", "proof", "Proof", "F10")] : [panel("views", "plan", "Plan", "F10")]),
      panel("views", "usage", "Usage"),
      action("inspect", "agents.details", "Agent details", "", async () => { await this.workbench.openAgentDetails(); }, () => {
        if (ready()) return ready();
        const row = this.workbench.selectedAgent();
        return !row ? "no agent selected" : row.meta.length ? undefined : "no details for this agent";
      }),
      panel("inspect", "history", "Browse history"),
      panel("inspect", "events", "Host events"),
      action("inspect", "notices", "Recent notices", "", () => this.showNotices(),
        () => ready() ?? (this.workbench.recentNotices().length ? undefined : "no notices yet")),
      action("message", "queue.edit", "Edit the last queued input", dequeue ? keyLabel(dequeue) : "", () => { void this.queue.editLast(); }, queued),
      action("message", "queue.focus", "Focus the queue", focus ? keyLabel(focus) : "", () => { this.focusQueue(); }, queued),
      ...(build?.message ?? []),
      ...(rewind ? [action("message", "rewind", "Rewind to an earlier turn…", "", async () => {
        if (this.ui) await runRewind(this.ui, rewind, this.sessionManager, (text, tone, detail) => this.workbench.notice(text, tone, detail));
      })] : []),
      action("model", "app.model.select", "Select model", this.nativeKeys?.("app.model.select") ?? "", () => this.nativeAction?.("app.model.select")),
      action("model", "app.thinking.cycle", "Cycle effort", this.nativeKeys?.("app.thinking.cycle") ?? "", () => this.nativeAction?.("app.thinking.cycle"),
        () => !this.snapshot()?.effortAvailable ? "this model has no effort levels" : ready()),
      panel("workspace", "project", "Show files from another project…"),
      panel("workspace", "refresh", taskBound ? "Reread task, proof and files" : "Reread files"),
      action("workspace", "sidebar.focus", "Sidebar: focus, then the next tab", "F6", () => this.sidebarKey()),
      panel("workspace", "toggle", "Sidebar: collapse or expand", sidebarKeyLabel(this.sidebarToggle)),
      ...(build?.refresh ? [build.refresh] : []),
    ];
    // Saved settings open the Settings page; the Display rows below change only this terminal, for this session.
    if (this.settingsPage) items.push(action("workspace", "settings", "Saved settings…", "", () => this.openSettings()));
    for (const appearance of Object.keys(PACKAGED) as Appearance[]) items.push(action("display", `appearance.${appearance}`,
      `Appearance: ${appearance}`, "", () => this.applyDisplay({ appearance }), ready, appearance === this.appearance));
    for (const density of ["comfortable", "compact"] as const) items.push(action("display", `density.${density}`,
      `Density: ${density}`, "", () => this.applyDisplay({ density }), ready, density === this.density));
    for (const motion of ["animate", "still"] as const) items.push(action("display", `motion.${motion}`,
      `Motion: ${motion}`, "", () => this.applyDisplay({ motion }), ready, motion === this.motion));
    for (const bell of ["off", "on"] as const) items.push(action("display", `bell.${bell}`,
      `Bell: ${bell}`, "", () => this.applyDisplay({ bell }), ready, bell === this.bell));
    items.push(action("help", "keys", "Show keys", "F1", () => this.showKeys()));
    return items;
  }
  /**
   * F2 in a build session: the message receiver, open questions, Finish and board planning (message),
   * and the board status reread (workspace), each with its unavailable reason; undefined in every other role. A bare
   * session has no board, so board planning and the reread are not offered.
   */
  private buildActions(action: ActionMaker, ready: () => string | undefined):
    { readonly message: readonly HostAction[]; readonly refresh?: HostAction } | undefined {
    const build = this.build, ui = this.options.build;
    if (!build || !ui) return undefined;
    const receiver = targetKey(), unavailable = () => ready() ?? boardUnavailable(this.snapshot()?.build), board = ui.board;
    const message = [
      action("message", "build.target", "Change who receives the message", receiver ? keyLabel(receiver) : "", () => build.cycleTarget(),
        () => ready() ?? (ui.fleet.snapshot().children.some(steerable) ? undefined : "no agent is running")),
      ...build.questions().map(child => action("message", `build.answer.${child.id}`, `Answer ${plainLabel(child.id)}'s question`, "",
        () => { build.open("question", child.id); })),
      ...(build.state()?.session.trustedCode ? [] : [
        action("message", "build.finish", "Finish: choose how this leaves the session", "", () => { build.openFinish(); }, () => ready() ?? build.finishUnavailable()),
      ]),
    ];
    if (!board) return { message };
    return {
      message: [...message,
        action("message", "build.plan.review", "Review the proposed board plan", "", () => { build.open("plan"); },
          () => ready() ?? (board.proposed() ? undefined : "no plan is waiting")),
        action("message", "build.plan.draft", "Plan board tasks from this conversation", "", () => build.planDraft(), unavailable),
      ],
      refresh: action("workspace", "build.board.refresh", "Reread board status", "", () => board.refresh(), unavailable),
    };
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
      { overlay: true, overlayOptions: () => this.workbench.popupPlace() });
    } finally { this.overlay = undefined; this.refresh(); }
    if (!action || this.disposed) return;
    const reason = action.disabled();
    if (reason) { this.workbench.notice(reason, "warning"); return; }
    try { await action.run(); } catch (error) { this.workbench.notice(plainLabel(String(error)), "error"); }
    this.refresh();
  }
  /**
   * F2 Recent notices: the session's notices, oldest first, each with its time and tone glyph and, indented under it,
   * the exact raw detail it carried; every row wraps, nothing is clipped. Opens at the newest.
   */
  private async showNotices(): Promise<void> {
    const ui = this.ui;
    if (!ui) return;
    const notices = this.workbench.recentNotices();
    const lines: (string | PagerLine)[] = notices.flatMap(item => [
      `${new Date(item.at).toTimeString().slice(0, 8)}  ${noticeGlyph(item.tone) || "○"} ${plainLabel(item.text)}`,
      // Each raw detail line indented two cells under its notice; its wrapped continuations hang at the same indent.
      ...(item.detail?.split(/\r?\n/).map(line => ({ text: `  ${line}`, hanging: 2 })) ?? []),
    ]);
    this.overlay = "pager"; this.refresh();
    try { await openPager(ui, { title: `Recent notices · ${notices.length}`, lines, start: lines.length, wrap: true }); }
    finally { this.overlay = undefined; this.refresh(); }
  }
  /** F1: the keys this mode offers; Enter or a click runs a key as if typed. */
  private async showKeys(): Promise<void> {
    const ui = this.ui;
    if (!ui || !this.canInteract) return;
    const identity = this.options.identity, answerKey = this.questions ? questionsKey() : undefined;
    const groups = keyGroups({ taskBound: !!identity.task,
      ...(this.workbench.view === "plan" ? { planActions: this.workbench.planPage?.actions() ?? [] } : {}),
      ...(this.workbench.view === "job" ? { jobActions: this.workbench.jobPage.actions() } : {}),
      ...(this.workbench.view === "agent" ? { agentBackLabel: this.workbench.agentBackLabel() } : {}),
      jobs: this.snapshot()?.jobs != null, queueControl: this.queue.controllable, lifecycle: !!identity.task,
      sidebarKey: this.sidebarToggle, ...(answerKey ? { questionsKey: answerKey } : {}), ...(this.build ? { build: true, board: !!this.options.build?.board, trustedCode: this.build.state()?.session.trustedCode === true } : {}) });
    this.overlay = "keys"; this.refresh();
    let data: string | undefined;
    try {
      data = await ui.custom<string | undefined>((tui, _theme, _keys, done) =>
        new KeysList(groups, done, () => tui.requestRender(), Math.max(3, Math.min(22, tui.terminal.rows - 8))),
      { overlay: true, overlayOptions: () => this.workbench.popupPlace() });
    } finally { this.overlay = undefined; this.refresh(); }
    if (data && !this.disposed) this.replay(data);
  }
}
