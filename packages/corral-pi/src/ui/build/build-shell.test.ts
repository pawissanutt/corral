import { afterAll, beforeAll, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { KeybindingsManager } from "../../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { createChatViewport } from "../../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { stripTerminalSequences, Text, visibleWidth, type Component } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame, type LayoutFrame } from "../../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import type {
  BoardTaskStatus, BuildPanelState, BuildSessionRecord, BuildSummary, BuildUi, ChildSnapshot, ChildState, ExitOption, ExitState,
} from "../../build/types.ts";
import { keyData } from "../keymap.ts";
import { NativePanels, type NativePanelsOptions } from "../panels.ts";
import { PANEL_BLUE, PANEL_ON_ACCENT, type PanelSnapshot } from "../panels-view.ts";
import type { MainView } from "../workbench.ts";
import { RecordingTerminal } from "../../../test/support/panel-terminal.ts";

beforeAll(() => sdk.initTheme("dark", false));
const work = mkdtempSync(join(tmpdir(), "corral-build-shell-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

/** Pi's stock bindings; the follow-up key differs on Windows and WSL, so its sequence comes from them. */
const BINDINGS = new KeybindingsManager();
const KEY = { f7: "\x1b[18~", f8: "\x1b[19~", f9: "\x1b[20~", f10: "\x1b[21~", esc: "\x1b", enter: "\r", altT: "\x1bt",
  followUp: keyData(BINDINGS.getKeys("app.message.followUp")[0] ?? "alt+enter") ?? "\x1b\r" };
const identity = { store: "build", project: "", role: "build", generation: 1 } as const;
const NOW = Date.now();
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line)).join("\n");
/** Drains the promise continuations and microtasks the panels queue; no wall-clock wait. */
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}

const OPTIONS: readonly ExitOption[] = [
  { choice: "keep", label: "Keep on the branch", detail: "the branch stays for you" },
  { choice: "land", label: "Land after the gate", detail: "the gate checks it, then it lands", unavailable: "Project policy requires independent review." },
  { choice: "ship", label: "Ship through the board", detail: "a board task carries it" },
  { choice: "iterate", label: "Keep iterating", detail: "back to the conversation" },
];
const SUMMARY: BuildSummary = { branch: "build/save-slots", base: "a", head: "b", commits: 2, files: 1, added: 3, removed: 1, dirty: false, slices: [], outside: [] };

function child(id: string, state: ChildState, extra: Partial<ChildSnapshot> = {}): ChildSnapshot {
  return {
    id, call: "call-1", title: `${id} slice`, brief: `Build the ${id} slice`, owns: ["src"], after: [], profile: "default", model: "openai/gpt-5",
    state, createdAt: NOW - 60_000, ...(state === "queued" ? {} : { startedAt: NOW - 50_000 }),
    edits: [], usage: { input: 10, output: 5, turns: 1, cost: 0.12 }, steers: [], pulse: [1, 2], history: "qrr", messages: [], ...extra,
  };
}
const asking = (id: string, askedAt: number) => child(id, "needs", {
  question: { question: "Which empty state should the picker show?", askedAt, options: [{ label: "Show a new game action" }, { label: "Hide the picker" }] },
});

/** A BuildUi over plain state: tests change the state, then `fire()` notifies the panels as the host does. */
function fakeBuild(worktree: string, base = "a") {
  const s = {
    children: [] as ChildSnapshot[], target: "parent", exit: { phase: "none" } as ExitState, tasks: [] as BoardTaskStatus[],
    steer: true, calls: [] as string[], listeners: new Set<() => void>(),
  };
  const subscribe = (listener: () => void) => { s.listeners.add(listener); return () => { s.listeners.delete(listener); }; };
  const live = (id: string) => s.children.some(entry => entry.id === id && ["queued", "running", "needs"].includes(entry.state));
  const ui: BuildUi = {
    fleet: {
      snapshot: () => ({ children: s.children, maxParallel: 3, startedAt: NOW - 60_000 }),
      steer: async (id, text) => { s.calls.push(`steer ${id} ${text}`); return s.steer; },
      cancel: id => { s.calls.push(`cancel ${id}`); return true; },
      setModel: (id, model) => { s.calls.push(`model ${id} ${model}`); },
      answer: (id, text) => { s.calls.push(`answer ${id} ${text}`); return true; },
      models: () => ["anthropic/claude-opus-5.5", "openai/gpt-5"],
      profiles: () => [{ name: "deep", model: "anthropic/claude-opus-5.5" }, { name: "default", model: "openai/gpt-5" }],
      subscribe,
    },
    exit: {
      options: () => OPTIONS, summary: async () => SUMMARY, state: () => s.exit,
      choose: async choice => { s.calls.push(`choose ${choice}`); return s.exit; }, subscribe,
    },
    board: {
      tasks: () => s.tasks, proposed: () => undefined, preview: () => Promise.withResolvers<never>().promise, file: async () => [], dismiss: () => {},
      refresh: async () => { s.calls.push("refresh"); }, subscribe,
    },
    target: () => s.target,
    setTarget: target => { s.target = target === "parent" || live(target) ? target : "parent"; },
  };
  const session: BuildSessionRecord = { version: 1, id: "B-1", name: "save-slots", created: "", updated: "", repoRoot: work, worktree,
    branch: "build/save-slots", base, here: false, project: "mitiplay", workspaceRoot: work };
  const state = (): BuildPanelState => ({ session, settings: { exitPolicy: "gate", maxParallel: 3, profiles: [] },
    fleet: ui.fleet.snapshot(), target: s.target, exit: s.exit, board: s.tasks });
  return { s, ui, state, fire: () => { for (const listener of [...s.listeners]) listener(); } };
}
type Build = ReturnType<typeof fakeBuild>;

function snapshot(build: Build, extra: Partial<PanelSnapshot> = {}): PanelSnapshot {
  return { bound: true, model: "openai-codex/example", effort: "high", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [],
    skills: [], ledger: null, ledgerStale: false, jobs: [], auxiliary: null, steering: 0, followUps: 0, helpers: [], cost: 0.5,
    build: build.state(), ...extra };
}

interface ShellEditor extends Component {
  focused: boolean;
  handleInput(data: string): void;
  setText(text: string): void;
  getText(): string;
  onSubmit?: (text: string) => void;
  onAction(action: string, handler: () => void): void;
}

/** NativePanels of a build session on a recording terminal, with the real composer subclass and a minimal UI context. */
function shell(columns: number, rows: number, build: Build, extra: Partial<PanelSnapshot> = {}, options: Partial<NativePanelsOptions> = {}) {
  const panels = new NativePanels({ identity, cwd: work, snapshot: () => snapshot(build, extra), build: build.ui, ...options });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  const terminal = panels.wrap(physical); terminal.start(() => {}, () => {});
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: unknown[] = [];
  const pi = {
    on: (name: string, fn: (event: unknown, ctx: unknown) => unknown) => { handlers.set(name, fn); }, registerCommand() {},
    registerMessageRenderer() {}, registerMarkdownTransformer() {}, sendMessage: (message: unknown) => { sent.push(message); },
  } as unknown as ExtensionAPI;
  // Toasts the panels show, with their placement; `hide` marks one gone.
  const toasts: { readonly lines: () => string[]; readonly options: { readonly row?: unknown; readonly col?: unknown; readonly nonCapturing?: boolean }; hidden: boolean }[] = [];
  const tui = { terminal: { rows, columns }, requestRender() {}, hasOverlay: () => false,
    showOverlay: (component: Component, options: { readonly row?: unknown; readonly col?: unknown; readonly nonCapturing?: boolean }) => {
      const toast = { lines: () => component.render(columns), options, hidden: false };
      toasts.push(toast);
      return { hide() { toast.hidden = true; } };
    } };
  const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
  const editorTheme = { borderColor: (text: string) => text, selectList: {
    selectedPrefix: (text: string) => text, selectedText: (text: string) => text, description: (text: string) => text,
    scrollInfo: (text: string) => text, noMatch: (text: string) => text,
  } };
  let dock: Component | undefined, footer: Component | undefined, editor: ShellEditor | undefined;
  const asked = { confirm: [] as string[], select: [] as string[] };
  const answers = { confirm: false, select: undefined as string | undefined };
  const ui = {
    theme: { getColorMode: () => "truecolor", fg: (_color: string, text: string) => text, bg: (_color: string, text: string) => text },
    setTheme() {}, setHeader() {}, notify() {}, setWorkingVisible() {},
    setEditorComponent: (factory: (tui: unknown, theme: unknown, keys: unknown) => ShellEditor) => {
      editor = factory(tui, editorTheme, BINDINGS);
      editor.focused = true;
    },
    setWidget: (_key: string, factory: (tui: unknown, theme: unknown) => Component) => { dock = factory(tui, theme); },
    setFooter: (factory: (tui: unknown, theme: unknown, data: unknown) => Component) => {
      footer = factory(tui, theme, { onBranchChange: () => () => {}, getExtensionStatuses: () => new Map() });
    },
    getEditorText: () => editor?.getText() ?? "", setEditorText: (text: string) => { editor?.setText(text); },
    custom: () => Promise.withResolvers<unknown>().promise,
    confirm: async (title: string) => { asked.confirm.push(title); return answers.confirm; },
    select: async (title: string, options: string[]) => { asked.select.push(title); return options.find(option => option === answers.select); },
    input: async () => undefined,
  };
  panels.install(pi, sdk);
  handlers.get("session_start")?.({}, { mode: "tui", ui });
  if (!dock || !footer || !editor) throw new Error("panels did not install their dock, footer and composer");
  const installed = { dock, footer, editor };
  const main = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
    widgetsAbove: installed.dock, editor: installed.editor, footer: installed.footer, mainView: transcript => panels.mainView(transcript) });
  return {
    panels, terminal, asked, answers, sent, toasts, ...installed,
    // Laid out twice: the rail takes the height the first layout gave it.
    frame: (): LayoutFrame => { renderLayoutFrame(main.root, terminal.columns, rows, () => {}); return renderLayoutFrame(main.root, terminal.columns, rows, () => {}); },
    stop() { terminal.stop(); panels.dispose(); },
  };
}

/** A plain left click at frame cell (x, y), dispatched from the deepest component outwards as Pi does. */
function click(frame: LayoutFrame, x: number, y: number): boolean {
  for (const box of getLayoutBoxesAt(frame, x, y)) {
    const result = box.component.handleMouse?.({ type: "click", button: "left", x: x - box.rect.x, y: y - box.rect.y, screenX: x, screenY: y,
      width: box.rect.width, height: box.rect.height, shift: false, alt: false, ctrl: false, clickCount: 1 });
    if (result) return true;
  }
  return false;
}
/** The cell where `text` starts on a frame row, or -1. */
function column(frame: LayoutFrame, row: number, text: string): number {
  const line = stripTerminalSequences(frame.lines[row] ?? ""), at = line.indexOf(text);
  return at < 0 ? -1 : visibleWidth(line.slice(0, at));
}

test("F10 opens Board in a build session, and a click on the Board tab does the same", () => {
  const build = fakeBuild(work);
  const h = shell(140, 46, build);
  try {
    expect(h.panels.handleInput(KEY.f10)).toBeTrue(); expect(h.panels.workbench.view).toBe("board");
    expect(h.panels.handleInput(KEY.f9)).toBeTrue(); expect(h.panels.workbench.view).toBe("conversation");
    const frame = h.frame(), row = frame.lines.findIndex(line => stripTerminalSequences(line).includes("F10"));
    const x = column(frame, row, "Board");
    expect(x).toBeGreaterThan(0);
    expect(click(frame, x, row)).toBeTrue(); expect(h.panels.workbench.view).toBe("board");
  } finally { h.stop(); }
});

test("the composer is a rounded box with its title in the top border, `›` before the draft and the model bar inside", () => {
  for (const [columns, rows] of [[185, 40], [92, 40], [46, 40]] as const) {
    const build = fakeBuild(work);
    const h = shell(columns, rows, build);
    try {
      h.editor.setText("draft text");
      const raw = h.frame().lines, lines = raw.map(line => stripTerminalSequences(line));
      const top = lines.findIndex(line => line.includes("╭─ Message"));
      expect(top).toBeGreaterThan(0);
      expect(lines[top]!.startsWith(" ╭─ Message")).toBeTrue(); expect(lines[top]!.endsWith("╮ ")).toBeTrue();
      expect(lines[top + 1]).toMatch(/^ │ › draft text +│ $/);
      expect(lines[top + 2]).toMatch(/^ │ example ▾/);
      expect(lines[top + 3]).toMatch(/^ ╰─+╯ $/);
      // The composer has the focus: its border is accent.
      expect(raw[top]).toContain(PANEL_BLUE + "╭");
      // Then the status bar with the BUILD pill and `○ unbound` (the word from 105 columns), a rule and the footer.
      expect(lines[top + 4]).toContain(columns >= 64 ? " BUILD " : " B ");
      expect(lines[top + 4]).toContain("○"); expect(lines[top + 4]).not.toContain("●");
      if (columns >= 105) expect(lines[top + 4]).toContain("○ unbound");
      expect(raw[top + 4]).toContain(PANEL_ON_ACCENT);
      expect(lines[top + 5]).toBe("─".repeat(columns));
      expect(lines[top + 6]).toMatch(/F1\s+(keys|more)\s*$/);
      expect(top + 7).toBe(rows);
    } finally { h.stop(); }
  }
});

test("Alt+T then Enter steers the child and clears the composer; a refusal keeps the text; /finish still reaches Pi", async () => {
  const build = fakeBuild(work);
  build.s.children = [child("picker-ui", "running")];
  const h = shell(140, 46, build), submitted: string[] = [];
  h.editor.onSubmit = text => { submitted.push(text); };
  try {
    expect(h.panels.handleInput(KEY.altT)).toBeTrue(); expect(build.s.target).toBe("picker-ui");
    h.editor.setText("make it blue"); h.editor.handleInput(KEY.enter); await settle();
    expect(build.s.calls).toEqual(["steer picker-ui make it blue"]); expect(h.editor.getText()).toBe(""); expect(submitted).toEqual([]);
    build.s.steer = false;
    h.editor.setText("again"); h.editor.handleInput(KEY.enter); await settle();
    expect(build.s.calls.at(-1)).toBe("steer picker-ui again"); expect(h.editor.getText()).toBe("again"); expect(submitted).toEqual([]);
    h.editor.setText("/finish"); h.editor.handleInput(KEY.enter); await settle();
    expect(submitted).toEqual(["/finish"]); expect(build.s.calls).toHaveLength(2);
  } finally { h.stop(); }
});

test("Alt+Enter with a child target never queues a follow-up for the parent", () => {
  const build = fakeBuild(work);
  build.s.children = [child("picker-ui", "running")];
  const h = shell(140, 46, build, { streaming: true });
  let followUps = 0;
  h.editor.onAction("app.message.followUp", () => { followUps++; });
  try {
    expect(h.panels.handleInput(KEY.altT)).toBeTrue();
    h.editor.setText("later"); h.editor.handleInput(KEY.followUp);
    expect(followUps).toBe(0); expect(h.editor.getText()).toBe("later"); expect(build.s.calls).toEqual([]);
    expect(h.panels.workbench.currentNotice?.text).toContain("parent");
  } finally { h.stop(); }
});

test("a question that arrives while the composer holds a draft leaves digits to the composer", () => {
  const build = fakeBuild(work);
  build.s.children = [child("picker-ui", "running")];
  const h = shell(80, 24, build);
  try {
    h.editor.setText("draft");
    build.s.children = [asking("picker-ui", NOW)]; build.fire();
    expect(h.panels.focus).toBe("composer");
    expect(h.panels.handleInput("1")).toBeFalse();
    expect(build.s.calls).toEqual([]);
    expect(plain(h.dock.render(80))).not.toContain("picker-ui asks");
    expect(plain(h.frame().lines)).toContain("▲ 1 needs you");
  } finally { h.stop(); }
});

test("the exit dialog docks on pending; Esc leaves the header prompt, whose click reopens it; an unavailable option never chooses", async () => {
  const build = fakeBuild(work);
  const h = shell(140, 46, build);
  try {
    build.s.exit = { phase: "pending" }; build.fire();
    expect(h.panels.focus).toBe("choice");
    expect(plain(h.dock.render(105))).toContain("How should this leave");
    // Esc leaves the exit prompt in header row 1 at once; the notice it posts is a toast at the top right of the main view.
    expect(h.panels.handleInput(KEY.esc)).toBeTrue();
    expect(h.panels.focus).toBe("composer"); expect(plain(h.dock.render(105))).not.toContain("How should this leave");
    await settle();
    const toast = h.toasts.at(-1), notice = h.panels.workbench.currentNotice;
    expect(notice).toBeDefined(); expect(toast?.hidden).toBeFalse(); expect(toast?.options.nonCapturing).toBeTrue();
    expect(toast?.options.row).toBe(h.panels.workbench.bodyTop());
    const box = toast!.lines().map(line => stripTerminalSequences(line));
    expect(box).toHaveLength(3); expect(box[0]!.startsWith("╭")).toBeTrue(); expect(box[1]).toContain(notice!.text.slice(0, 20));
    expect(plain(h.frame().lines.slice(0, 1))).not.toContain(notice!.text.slice(0, 20));
    const frame = h.frame(), x = column(frame, 0, "exit choice pending");
    expect(x).toBeGreaterThan(0);
    expect(click(frame, x, 0)).toBeTrue();
    expect(h.panels.focus).toBe("choice");
    expect(h.panels.handleInput("2")).toBeTrue(); await settle();
    expect(build.s.calls).toEqual([]); expect(h.panels.workbench.currentNotice?.text).toContain("unavailable");
    expect(h.panels.handleInput("1")).toBeTrue(); await settle();
    expect(build.s.calls).toEqual(["choose keep"]);
  } finally { h.stop(); }
});

test("x on a child row asks before cancelling, and m sets the chosen profile", async () => {
  const build = fakeBuild(work);
  build.s.children = [child("picker-ui", "running")];
  const h = shell(140, 46, build);
  try {
    expect(h.panels.handleInput(KEY.f8)).toBeTrue();
    expect(h.panels.workbench.agentsView.entry()?.row.key).toBe("child:picker-ui");
    expect(h.panels.handleInput("x")).toBeTrue(); await settle();
    expect(h.asked.confirm).toEqual(["Cancel picker-ui?"]); expect(build.s.calls).toEqual([]);
    h.answers.confirm = true;
    expect(h.panels.handleInput("x")).toBeTrue(); await settle();
    expect(build.s.calls).toEqual(["cancel picker-ui"]);
    h.answers.select = "deep · anthropic/claude-opus-5.5";
    expect(h.panels.handleInput("m")).toBeTrue(); await settle();
    expect(build.s.calls.at(-1)).toBe("model picker-ui deep");
  } finally { h.stop(); }
});

/** A Git repository with one commit (returned as `base`), then `commits` committed and `edits` left uncommitted. */
function repository(name: string, files: readonly string[], commits: readonly string[], edits: readonly string[]): { readonly path: string; readonly base: string } {
  const path = join(work, name);
  mkdirSync(path, { recursive: true });
  const git = (...args: string[]) => execFileSync("git", ["-c", "user.email=t@example.com", "-c", "user.name=t", ...args], { cwd: path, encoding: "utf8" });
  for (const file of files) writeFileSync(join(path, file), "one\n");
  git("init", "-q"); git("add", "."); git("commit", "-q", "-m", "base");
  const base = git("rev-parse", "HEAD").trim();
  for (const file of commits) writeFileSync(join(path, file), "committed\n");
  if (commits.length) { git("add", "."); git("commit", "-q", "-m", "child work"); }
  for (const file of edits) writeFileSync(join(path, file), "edited\n");
  return { path, base };
}

test("a build session's Changes reads its worktree from the start: what the session changed, with authors and no project choice", async () => {
  // The child committed beta.txt and left alpha.txt edited: both show only against the commit the session started from.
  const repo = repository("session-repo", ["alpha.txt", "beta.txt"], ["beta.txt"], ["alpha.txt"]);
  const other = repository("other-repo", ["alpha.txt"], [], ["alpha.txt"]);
  const build = fakeBuild(repo.path, repo.base);
  build.s.children = [
    child("alpha", "done", { edits: [{ path: "alpha.txt", outside: false, at: NOW }] }),
    child("beta", "done", { edits: [{ path: "beta.txt", outside: false, at: NOW }] }),
  ];
  // A projects reader is what gives assistant sessions their project choice; a build session never takes it.
  const h = shell(140, 46, build, {}, { cwd: repo.path, readProjects: async () => ({ workspace: work, projects: {}, session: null }) });
  try {
    expect(h.panels.workbench.review.scopeRequired).toBeFalse();
    await h.panels.loadFiles();
    expect(h.panels.handleInput(KEY.f7)).toBeTrue();
    await h.panels.loadFiles(); await settle();
    const frame = h.frame(), screen = plain(frame.lines);
    expect(screen).toContain("alpha.txt"); expect(screen).toContain("beta.txt");
    expect(screen).toMatch(/all 2\s+alpha 1\s+beta 1/);
    expect(screen).not.toContain("Choose a project"); expect(screen).not.toContain("Choose project");
    expect(stripTerminalSequences(frame.lines[1] ?? "")).not.toContain("Changes:");
    h.panels.sidebar.tab = "files";
    expect(plain(h.panels.renderSidebar(34, 46))).toContain("beta.txt");
    // Another repository chosen for Changes has no authors.
    await h.panels.workbench.review.setRepository(other.path);
    await h.panels.loadFiles(); await settle();
    const elsewhere = plain(h.frame().lines);
    expect(elsewhere).toContain("alpha.txt"); expect(elsewhere).not.toMatch(/all \d+\s+alpha \d/);
  } finally { h.stop(); }
});

test("every build surface fits 140x46, 80x24, 80x14 and 52x24 with a docked dialog and a queued item; the receiver stays at 52", () => {
  for (const [columns, rows] of [[140, 46], [80, 24], [80, 14], [52, 24]] as const) {
    const build = fakeBuild(work);
    build.s.children = [
      child("host-slots", "done", { endedAt: NOW - 10_000, report: "Slots exported", edits: [{ path: "crates/host/src/slots.rs", outside: true, at: NOW }] }),
      child("picker-ui", "running", { activity: "edit src/picker.ts", steers: ["keep the empty state small"] }),
      asking("app-store", NOW - 5_000),
      child("save-tests", "queued", { after: ["picker-ui"] }),
    ];
    build.s.tasks = [
      { id: "T-971", title: "Host: witness-scoped slot export", status: "Ready", phase: "running", depends: [], updatedAt: NOW - 12_000 },
      { id: "T-972", title: "Picker empty state", status: "Blocked", phase: "blocked", depends: ["T-971"], detail: "waits for review", updatedAt: NOW },
    ];
    const h = shell(columns, rows, build, { queue: [{ key: "q1", text: "Follow once ".repeat(8), policy: "follow_up", editable: true }] });
    try {
      const check = (label: string) => {
        const frame = h.frame();
        for (const line of frame.lines) {
          if (visibleWidth(line) > frame.width) throw new Error(`${label} at ${columns}x${rows} overflows: ${stripTerminalSequences(line)}`);
        }
        return plain(frame.lines);
      };
      // The waiting question docked itself at start with an empty composer; Esc leaves it waiting.
      expect(h.panels.focus).toBe("choice"); check("question");
      expect(h.panels.handleInput(KEY.esc)).toBeTrue();
      expect(h.panels.handleInput(KEY.altT)).toBeTrue(); expect(build.s.target).toBe("picker-ui");
      build.s.exit = { phase: "pending" }; build.fire();
      expect(plain(h.dock.render(h.terminal.columns))).toContain("How should this leave");
      const docked = check("exit dialog");
      if (columns === 52) expect(docked).toContain("→ picker-ui");
      const views: readonly MainView[] = ["conversation", "agents", "changes", "board"];
      for (const view of views) { h.panels.workbench.setView(view); check(view); }
      h.panels.workbench.openChild("picker-ui"); check("agent");
      h.panels.workbench.openChild("app-store"); check("agent asking");
    } finally { h.stop(); }
  }
});
