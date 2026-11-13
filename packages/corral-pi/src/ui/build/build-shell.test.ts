import { afterAll, beforeAll, expect, jest, test } from "bun:test";
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
import { NOTICE_MS } from "../header.ts";
import { keyData } from "../keymap.ts";
import { NativePanels } from "../panels.ts";
import type { PanelSnapshot } from "../panels-view.ts";
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
function fakeBuild(worktree: string) {
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
    branch: "build/save-slots", base: "a", here: false, project: "mitiplay", workspaceRoot: work };
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
function shell(columns: number, rows: number, build: Build, extra: Partial<PanelSnapshot> = {}, cwd = work) {
  const panels = new NativePanels({ identity, cwd, snapshot: () => snapshot(build, extra), build: build.ui });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  const terminal = panels.wrap(physical); terminal.start(() => {}, () => {});
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: unknown[] = [];
  const pi = {
    on: (name: string, fn: (event: unknown, ctx: unknown) => unknown) => { handlers.set(name, fn); }, registerCommand() {},
    registerMessageRenderer() {}, sendMessage: (message: unknown) => { sent.push(message); },
  } as unknown as ExtensionAPI;
  const tui = { terminal: { rows, columns }, requestRender() {}, hasOverlay: () => false };
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
    setTheme() {}, setHeader() {}, notify() {},
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
    panels, terminal, asked, answers, sent, ...installed,
    frame: (): LayoutFrame => renderLayoutFrame(main.root, terminal.columns, rows, () => {}),
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
    // The "No exit chosen" notice holds header row 1 for its duration; the prompt follows.
    jest.useFakeTimers();
    try {
      expect(h.panels.handleInput(KEY.esc)).toBeTrue();
      expect(h.panels.focus).toBe("composer"); expect(plain(h.dock.render(105))).not.toContain("How should this leave");
      jest.advanceTimersByTime(NOTICE_MS + 1);
    } finally { jest.useRealTimers(); }
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

test("the Changes author filter shows only while Changes reads the build worktree", async () => {
  const repo = join(work, "repo");
  mkdirSync(join(repo, "src"), { recursive: true });
  writeFileSync(join(repo, "src", "a.ts"), "one\n");
  const git = (...args: string[]) => execFileSync("git", ["-c", "user.email=t@example.com", "-c", "user.name=t", ...args], { cwd: repo, stdio: "ignore" });
  git("init", "-q"); git("add", "."); git("commit", "-q", "-m", "init");
  writeFileSync(join(repo, "src", "a.ts"), "two\n");
  const edited = (worktree: string) => {
    const build = fakeBuild(worktree);
    build.s.children = [child("picker-ui", "done", { edits: [{ path: "src/a.ts", outside: false, at: NOW }] })];
    return build;
  };
  for (const [worktree, shown] of [[repo, true], [join(work, "elsewhere"), false]] as const) {
    const h = shell(140, 46, edited(worktree), {}, repo);
    try {
      await h.panels.loadFiles();
      expect(h.panels.handleInput(KEY.f7)).toBeTrue();
      await h.panels.loadFiles(); await settle();
      const screen = plain(h.frame().lines);
      expect(screen).toContain("src/a.ts");
      if (shown) expect(screen).toContain("picker-ui"); else expect(screen).not.toContain("picker-ui");
    } finally { h.stop(); }
  }
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
