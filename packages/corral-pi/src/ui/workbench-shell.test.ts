import type { TranscriptPage } from "../host/transcript-preview.ts";
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { getKeybindings, setKeybindings, sliceByColumn, stripTerminalSequences, Text, TuiAltScreen, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { NativePanels, type NativePanelsOptions } from "./panels.ts";
import { NativeWorkbench, type MainView } from "./workbench.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { PanelJob, PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import type { QueueControl } from "../host/queue-control.ts";
import type { BuildUi, ChildSnapshot } from "../build/types.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

beforeAll(() => sdk.initTheme("dark", false));
const work = mkdtempSync(join(tmpdir(), "corral-workbench-shell-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

const KEY = { f1: "\x1bOP", f7: "\x1b[18~", f8: "\x1b[19~", f9: "\x1b[20~", f10: "\x1b[21~", f11: "\x1b[23~", f5: "\x1b[15~", esc: "\x1b", enter: "\r",
  down: "\x1b[B", pageUp: "\x1b[5~", pageDown: "\x1b[6~", end: "\x1b[F", ctrlB: "\x02", ctrlO: "\x0f", alt1: "\x1b1", alt2: "\x1b2" };
const worker = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
const assistant = { store: "st-0123456789abcdef", project: "demo", role: "assistant" as const, assistant: "A-7" };
const NOW = Date.now();

function helper(request: string, extra: Partial<HelperInspection> = {}): HelperInspection {
  return { request, kind: "explore", model: "provider/model-a", phase: "completed", createdAt: NOW - 60_000, startedAt: NOW - 58_000,
    endedAt: NOW - 30_000, prompt: `Find ${request} callers`, result: `ANSWER_${request}`, truncated: false,
    messages: [{ id: "a", kind: "assistant", text: `ANSWER_${request}`, stopReason: "stop" }], ...extra };
}
function job(id: string, state: "running" | "exited", exitCode: number | null = null): PanelJob {
  return { job: id, state, detail: { job: id, command: `bun test ${id}`, state, startedAt: NOW - 20_000,
    endedAt: state === "running" ? undefined : NOW - 1000, exitCode, signal: null, groupClear: true, bytes: 10, truncated: false,
    tail: `OUTPUT_${id}\n`, lines: 1, monitorAttempts: 0, watches: [] } };
}
function snapshot(extra: Partial<PanelSnapshot> = {}): PanelSnapshot {
  return { bound: true, model: "openai-codex/example", effort: "high", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [],
    skills: [], ledger: null, ledgerStale: false, jobs: [], auxiliary: null, steering: 0, followUps: 0, helpers: [], ...extra };
}
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line)).join("\n");
/** Drains the promise continuations and microtasks the panels queue (dialogs settling, replayed keys); no wall-clock wait. */
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}
function viewport(wb: NativeWorkbench, draft = "REAL_EDITOR_DRAFT", presentation?: Parameters<NativeWorkbench["mount"]>[1]) {
  let supplied: Component | undefined;
  const view = createChatViewport({ document: new Text(Array.from({ length: 120 }, (_, i) => `PARENT_${i}`).join("\n"), 0, 0),
    pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0), editor: new Text(draft, 0, 0), footer: new Text("", 0, 0),
    mainView: transcript => { supplied = transcript; return wb.mount(transcript, presentation); } });
  return { ...view, supplied: () => supplied };
}
function bindUi(wb: NativeWorkbench, overrides: Partial<ExtensionUIContext> = {}): void {
  wb.bind({ notify() {}, custom: async () => undefined, confirm: async () => true, input: async () => undefined, select: async () => undefined,
    getEditorText: () => "", setEditorText() {}, ...overrides } as unknown as ExtensionUIContext);
}

/**
 * NativePanels installed against a recording terminal and a minimal UI context that captures the dock, the footer
 * and the commands. Like Pi, `custom` raises `ui_prompt_start` when a dialog opens and `ui_prompt_end` when it closes.
 */
function shell(columns: number, rows: number, state: () => PanelSnapshot | null, options: Partial<NativePanelsOptions> = {}) {
  const panels = new NativePanels({ identity: worker, cwd: work, snapshot: state, ...options });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  const terminal = panels.wrap(physical); terminal.start(() => {}, () => {});
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const commands = new Map<string, (args: string, ctx: unknown) => Promise<void>>();
  const pi = { on: (name: string, fn: (event: unknown, ctx: unknown) => unknown) => { handlers.set(name, fn); },
    registerCommand: (name: string, command: { handler: (args: string, ctx: unknown) => Promise<void> }) => { commands.set(name, command.handler); },
    registerMarkdownTransformer() {},
  } as unknown as ExtensionAPI;
  const tui = { terminal: { rows }, requestRender() {} }, theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
  let draft = "", dock: Component | undefined, footer: Component | undefined;
  const customs: Component[] = [];
  const ui = {
    theme: { getColorMode: () => "truecolor" }, setTheme() {}, setEditorComponent() {}, setHeader() {}, setTitle() {}, notify() {}, setWorkingVisible() {}, setHiddenThinkingLabel() {},
    setWidget: (_key: string, factory: (tui: unknown, theme: unknown) => Component) => { dock = factory(tui, theme); },
    setFooter: (factory: (tui: unknown, theme: unknown, data: unknown) => Component) => {
      footer = factory(tui, theme, { onBranchChange: () => () => {}, getExtensionStatuses: () => new Map() });
    },
    getEditorText: () => draft, setEditorText: (text: string) => { draft = text; },
    custom: (factory: (...args: unknown[]) => Component) => {
      const { promise, resolve } = Promise.withResolvers<unknown>();
      handlers.get("ui_prompt_start")?.({}, context);
      customs.push(factory(tui, theme, undefined, (value: unknown) => { handlers.get("ui_prompt_end")?.({}, context); resolve(value); }));
      return promise;
    },
    confirm: async () => true, input: async () => undefined, select: async () => undefined,
  };
  const context: { readonly mode: string; readonly ui: unknown } = { mode: "tui", ui };
  panels.install(pi, sdk);
  handlers.get("session_start")?.({}, context);
  if (!dock || !footer) throw new Error("panels did not install their dock and footer");
  const installed = { dock, footer };
  return {
    panels, physical, terminal, customs, ...installed,
    command: (name: string) => commands.get(name)?.("", context),
    draft: () => draft, setDraft: (text: string) => { draft = text; },
    stop() { terminal.stop(); panels.dispose(); },
  };
}

test("F8 opens Agents and Enter opens the agent's conversation; the parent draft and transcript instance stay", () => {
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => snapshot({ helpers: [helper("first")] }), changed() {}, canInteract: () => true });
  const view = viewport(wb);
  const screen = () => plain(renderLayoutFrame(view.root, 100, 40, () => {}).lines);
  try {
    expect(wb.handleInput(KEY.f8)).toBeTrue(); expect(wb.view).toBe("agents");
    let text = screen();
    expect(text).toContain("explore"); expect(text).toContain("REAL_EDITOR_DRAFT"); expect(text).not.toContain("PARENT_0");
    expect(wb.handleInput(KEY.enter)).toBeTrue(); expect(wb.view).toBe("agent");
    text = screen();
    expect(text).toContain("‹ Agents"); expect(text).toContain("Find first callers"); expect(text).toContain("ANSWER_first");
    expect(text).toContain("REAL_EDITOR_DRAFT");
    expect(wb.handleInput(KEY.esc)).toBeTrue(); expect(wb.view).toBe("agents");
    expect(wb.handleInput(KEY.f9)).toBeTrue(); expect(wb.view).toBe("conversation");
    expect(screen()).toContain("PARENT_"); expect(view.supplied()).toBe(view.transcript);
  } finally { wb.dispose(); }
});

test("End in an agent conversation jumps that conversation to its latest row", () => {
  const long = helper("first", { messages: [{ id: "a", kind: "assistant", text: Array.from({ length: 90 }, (_, i) => `first_LINE_${i}`).join("\n\n"), stopReason: "stop" }] });
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => snapshot({ helpers: [long] }), changed() {}, canInteract: () => true });
  const view = viewport(wb, "");
  const frame = () => plain(renderLayoutFrame(view.root, 100, 30, () => {}).lines);
  try {
    wb.openAgent("helper:first"); frame();
    for (let page = 0; page < 6; page++) wb.handleInput(KEY.pageUp);
    expect(frame()).not.toContain("first_LINE_89");
    expect(wb.handleInput(KEY.end)).toBeTrue(); expect(wb.view).toBe("agent");
    expect(frame()).toContain("first_LINE_89");
  } finally { wb.dispose(); }
});

test("while another view is open the parent transcript is never rendered", () => {
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => snapshot(), changed() {}, canInteract: () => true });
  const document = new Text("PARENT_ONLY", 0, 0), render = document.render.bind(document);
  let renders = 0;
  document.render = width => { renders++; return render(width); };
  const view = createChatViewport({ document, pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0), editor: new Text("", 0, 0),
    footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
  try {
    renderLayoutFrame(view.root, 100, 30, () => {});
    wb.setView("changes"); renders = 0;
    for (let frame = 0; frame < 5; frame++) renderLayoutFrame(view.root, 100, 30, () => {});
    expect(renders).toBe(0);
  } finally { wb.dispose(); }
});

test("reading positions survive a return to an agent and to the parent conversation", () => {
  const long = (request: string) => helper(request, { messages: [{ id: "a", kind: "assistant", text: Array.from({ length: 90 }, (_, i) => `${request}_LINE_${i}`).join("\n\n"), stopReason: "stop" }] });
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => snapshot({ helpers: [long("first"), long("second")] }), changed() {}, canInteract: () => true });
  const view = viewport(wb);
  const frame = () => plain(renderLayoutFrame(view.root, 100, 30, () => {}).lines);
  try {
    frame();
    const parentTop = view.transcript.scrollTop;
    wb.openAgent("helper:first"); frame();
    wb.handleInput(KEY.pageDown); wb.handleInput(KEY.pageDown);
    const reading = frame();
    expect(reading).not.toContain("Find first callers"); // the prompt scrolled out of view
    wb.handleInput(KEY.f9); frame();
    expect(view.transcript.scrollTop).toBe(parentTop);
    wb.openAgent("helper:second"); expect(frame()).toContain("Find second callers");
    wb.openAgent("helper:first");
    expect(frame()).toBe(reading);
  } finally { wb.dispose(); }
});

test("t on an agent without a transcript is typing; an eligible one reads by its own id", async () => {
  let current = snapshot({ helpers: [helper("first")], agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: "model-a", transcript: false }] });
  const reads: string[] = [];
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => current, changed() {}, canInteract: () => true,
    readTranscript: async id => { reads.push(id); return { stamp: "s", rows: [{ id: "1", kind: "raw", text: "EXISTING_TRANSCRIPT" }] }; } });
  bindUi(wb);
  wb.mount(new Text("PARENT", 0, 0));
  try {
    wb.handleInput(KEY.f8);
    expect(wb.handleInput("t")).toBeFalse(); expect(wb.focused).toBeFalse(); await settle();
    expect(reads).toEqual([]);
    current = { ...current, agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: "model-a", transcript: true }] };
    wb.handleInput(KEY.f8);
    expect(wb.handleInput("t")).toBeTrue(); await settle();
    expect(reads).toEqual(["first"]);
    current = { ...current, bound: false };
    wb.handleInput(KEY.f8); wb.handleInput("t"); await settle();
    expect(reads).toEqual(["first"]);
  } finally { wb.dispose(); }
});

test("job control keys keep the captured target through the Stop confirmation", async () => {
  const calls: { job: string; action: string }[] = [];
  const state = snapshot({ jobs: [job("j-1", "running"), job("j-2", "running")], inspector: { session: "s-1" } as unknown as InspectorSnapshot });
  const wb = new NativeWorkbench({ cwd: work, identity: worker, snapshot: () => state, changed() {}, canInteract: () => true,
    jobControl: async (target, action) => { calls.push({ job: target.job, action }); return { state: "running", text: "current output" }; } });
  let confirmations = 0;
  bindUi(wb, { confirm: async () => { confirmations++; wb.openAgents("job:j-2"); return true; } });
  wb.mount(new Text("PARENT", 0, 0));
  try {
    wb.openAgents("job:j-1");
    wb.handleInput("b"); await settle();
    wb.handleInput("p"); await settle();
    wb.handleInput("x"); await settle();
    expect(confirmations).toBe(1);
    expect(calls).toEqual([{ job: "j-1", action: "background" }, { job: "j-1", action: "peek" }, { job: "j-1", action: "stop" }]);
    expect(wb.agentsView.selected).toBe("job:j-2");
  } finally { wb.dispose(); }
});

test("agents keys walk agents then jobs, f pauses a job's follow and Ctrl+O folds the conversation's tool cards", () => {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager({ "app.tools.expand": "ctrl+o" }));
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => snapshot({ helpers: [helper("first")], jobs: [job("build", "running")] }),
    changed() {}, canInteract: () => true });
  const view = viewport(wb);
  const screen = () => plain(renderLayoutFrame(view.root, 120, 40, () => {}).lines);
  try {
    wb.handleInput(KEY.f8);
    expect(wb.agentsView.entry()?.row.key).toBe("helper:first");
    wb.handleInput("j"); expect(wb.agentsView.entry()?.row.key).toBe("job:build");
    expect(screen()).toContain("pause follow");
    wb.handleInput("f"); expect(screen()).toContain("resume follow");
    wb.handleInput("k"); wb.handleInput(KEY.enter); expect(wb.view).toBe("agent");
    expect(wb.handleInput(KEY.ctrlO)).toBeTrue(); expect(wb.view).toBe("agent");
  } finally { setKeybindings(previous); wb.dispose(); }
});

test("a footer click replays its key, and the F1 list runs F8", async () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    expect(h.panels.handleInput(KEY.f1)).toBeTrue(); await settle();
    const keys = h.customs.at(-1);
    if (!keys) throw new Error("F1 opened no keys list");
    expect(plain(keys.render(80))).toContain("F8");
    keys.handleInput?.(KEY.down); keys.handleInput?.(KEY.enter); await settle();
    expect(h.panels.workbench.view).toBe("agents");
    const lines = h.footer.render(140), row = stripTerminalSequences(lines.at(-1) ?? "");
    expect(row).toContain("esc"); expect(row.trimEnd()).toMatch(/F1\s+keys$/);
    const x = visibleWidth(row.slice(0, row.indexOf("esc")));
    const click: TuiMouseEvent = { type: "click", button: "left", x, y: lines.length - 1, screenX: x, screenY: 45, width: 140, height: lines.length,
      shift: false, alt: false, ctrl: false, clickCount: 1 };
    expect(h.footer.handleMouse?.(click)?.handled).toBeTrue(); await settle();
    expect(h.panels.workbench.view).toBe("conversation");
  } finally { h.stop(); }
});

test("the footer lists the keys of what is open: the palette, the keys list or a pager", async () => {
  const h = shell(140, 46, () => snapshot({ jobs: [job("build", "running")] }), { identity: assistant });
  const footer = () => stripTerminalSequences(h.footer.render(140).at(-1) ?? "");
  const close = async () => { h.customs.at(-1)?.handleInput?.(KEY.esc); await settle(); };
  try {
    // F2 and /actions open the same palette.
    void h.command("actions"); await settle();
    expect(footer()).toMatch(/↑↓\s+select/); expect(footer()).toMatch(/⏎\s+run/); expect(footer()).toMatch(/esc\s+close/);
    expect(footer()).not.toMatch(/choose|cancel|F1/);
    await close();
    expect(h.panels.handleInput(KEY.f1)).toBeTrue(); await settle();
    expect(footer()).toMatch(/⏎\s+run/); expect(footer()).toMatch(/esc\s+close/); expect(footer()).not.toMatch(/select|choose|F1/);
    await close();
    h.panels.workbench.openAgents("job:build");
    expect(h.panels.handleInput(KEY.enter)).toBeTrue(); await settle();
    expect(footer()).toMatch(/PgUp\/PgDn\s+page/); expect(footer()).toMatch(/esc\s+close/); expect(footer()).not.toMatch(/choose|F1/);
    await close();
    expect(footer()).toMatch(/F1\s+keys/);
  } finally { h.stop(); }
});

test("Ctrl+B at 140 columns hides the rail for the collapsed row; at 80 it opens the drawer", () => {
  const wide = shell(140, 46, () => snapshot({ helpers: [helper("first")] }));
  try {
    expect(wide.panels.wide).toBeTrue(); expect(wide.dock.render(140)).toEqual([]);
    expect(wide.panels.handleInput(KEY.ctrlB)).toBeTrue();
    expect(wide.panels.wide).toBeFalse();
    const row = plain(wide.dock.render(140));
    expect(row).toContain("Context"); expect(row).toContain("Files"); expect(row).toContain("Plan"); expect(row).toContain("▴");
    expect(wide.panels.handleInput(KEY.ctrlB)).toBeTrue(); expect(wide.panels.wide).toBeTrue();
  } finally { wide.stop(); }
  const narrow = shell(80, 24, () => snapshot({ helpers: [helper("first")] }));
  try {
    expect(plain(narrow.dock.render(80))).toContain("▴");
    expect(narrow.panels.handleInput(KEY.ctrlB)).toBeTrue();
    expect(narrow.panels.sidebar.drawer).toBeTrue();
    const drawer = narrow.dock.render(80);
    // The drawer's own rule comes first, then its tab row.
    expect(plain(drawer.slice(0, 1))).toBe("─".repeat(80));
    expect(plain(drawer.slice(1, 2))).toContain("▾"); expect(drawer.length).toBeGreaterThan(2);
  } finally { narrow.stop(); }
});

test("Alt+Up with an editable queued item puts its text before the draft", async () => {
  const pulled: string[] = [];
  const control: QueueControl = { enabled: () => true, edit: async key => { pulled.push(key); return "Follow once"; },
    remove: async () => true, move: async () => true };
  const h = shell(140, 46, () => snapshot({ queue: [{ key: "q1", text: "Follow once", policy: "follow_up", editable: true },
    { key: "n1", text: "STRUCTURED_WAKE", policy: "follow_up", editable: false }] }), { queueControl: control });
  try {
    h.setDraft("my draft");
    const dock = plain(h.dock.render(140));
    expect(dock).toContain("Follow once"); expect(dock).not.toContain("STRUCTURED_WAKE"); expect(dock).toContain("read-only");
    expect(h.panels.editQueued()).toBeTrue(); await settle();
    expect(pulled).toEqual(["q1"]); expect(h.draft()).toBe("Follow once\n\nmy draft");
  } finally { h.stop(); }
});

test("a letter a view defines but does not offer now goes to the composer like any other typing", () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    for (const letter of ["s", "x", "m"]) {
      expect(h.panels.handleInput(KEY.f8)).toBeTrue();
      expect(h.panels.workbench.agentsView.entry()?.row.key).toBe("helper:first");
      expect(h.panels.focus).toBe("main");
      expect(h.panels.handleInput(letter)).toBeFalse();
      expect(h.panels.focus).toBe("composer"); expect(h.panels.workbench.view).toBe("agents");
    }
  } finally { h.stop(); }
});

test("`/` with an empty draft starts a command in the composer from Changes and Agents; Enter without an action stays", () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    for (const key of [KEY.f7, KEY.f8]) {
      expect(h.panels.handleInput(key)).toBeTrue(); expect(h.panels.focus).toBe("main");
      const view = h.panels.workbench.view;
      expect(h.panels.handleInput("/")).toBeFalse();
      expect(h.panels.focus).toBe("composer"); expect(h.panels.workbench.view).toBe(view);
    }
    // Changes has no Enter action: Enter keeps the keys where they are; Tab gives them to the composer.
    h.panels.handleInput(KEY.f7);
    expect(h.panels.handleInput(KEY.enter)).toBeTrue(); expect(h.panels.focus).toBe("main");
    expect(h.panels.handleInput("\t")).toBeTrue(); expect(h.panels.focus).toBe("composer");
  } finally { h.stop(); }
});

test("Alt+1 to Alt+6 open the views in tab order, and a focused view leads its footer with its name", () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    expect(h.panels.handleInput(KEY.alt2)).toBeTrue(); expect(h.panels.workbench.view).toBe("agents");
    expect(stripTerminalSequences(h.footer.render(140).at(-1) ?? "")).toContain("keys → Agents");
    expect(h.panels.handleInput(KEY.alt1)).toBeTrue(); expect(h.panels.workbench.view).toBe("conversation");
    expect(stripTerminalSequences(h.footer.render(140).at(-1) ?? "")).not.toContain("keys →");
  } finally { h.stop(); }
});

test("a worker starts on Chat with the keys in the composer, and its footer has no view lead", () => {
  const h = shell(140, 46, () => snapshot(), { readTask: async () => ({ id: "T-001", title: "Fixture task" }) });
  try {
    expect(h.panels.workbench.view).toBe("conversation"); expect(h.panels.focus).toBe("composer");
    expect(stripTerminalSequences(h.footer.render(140).at(-1) ?? "")).not.toContain("keys →");
    expect(h.panels.handleInput(KEY.f5)).toBeTrue(); expect(h.panels.workbench.view).toBe("task"); expect(h.panels.focus).toBe("main");
    expect(stripTerminalSequences(h.footer.render(140).at(-1) ?? "")).toContain("keys → Task");
  } finally { h.stop(); }
});

test("opening a child's conversation makes it the receiver with a notice; leaving it for another view gives the parent back", () => {
  let receiver = "parent";
  const children = [{ id: "alpha", kind: "slice", state: "running" }] as unknown as ChildSnapshot[];
  const build = { fleet: { snapshot: () => ({ children, mail: [], maxParallel: 3, startedAt: NOW }) }, board: { tasks: () => [], proposed: () => undefined },
    target: () => receiver, setTarget: (id: string) => { receiver = id; } } as unknown as BuildUi;
  const wb = new NativeWorkbench({ cwd: work, identity: { store: "build", project: "", role: "build" }, snapshot: () => snapshot(),
    changed() {}, canInteract: () => true, build });
  wb.mount(new Text("PARENT", 0, 0));
  try {
    wb.openChild("alpha");
    expect(wb.view).toBe("agent"); expect(receiver).toBe("alpha"); expect(wb.currentNotice?.text).toBe("Messages now go to alpha");
    wb.setView("changes"); expect(receiver).toBe("parent");
    wb.openChild("alpha"); expect(receiver).toBe("alpha");
    wb.openAgents(); expect(receiver).toBe("parent");
  } finally { wb.dispose(); }
});

test("an expanded image's footer lists its own keys, never in the image title", () => {
  const h = shell(140, 46, () => snapshot(), { identity: assistant });
  const data = readFileSync(new URL("../../test/fixtures/images/zoom.png", import.meta.url)).toString("base64");
  try {
    expect(h.panels.workbench.images.activate({ data, mimeType: "image/png", dimensions: { widthPx: 960, heightPx: 480 } })).toBeTrue();
    expect(h.panels.workbench.mainContext()).toBe("image");
    const row = stripTerminalSequences(h.footer.render(140).at(-1) ?? "");
    expect(row).toMatch(/esc\s+close/); expect(row.trimEnd()).toMatch(/F1\s+keys$/);
    h.panels.workbench.images.setViewportHeight(4);
    expect(plain(h.panels.workbench.images.render(80).slice(0, 1))).not.toMatch(/\besc\b|click/i);
    expect(h.panels.handleInput(KEY.esc)).toBeTrue(); expect(h.panels.workbench.images.isOpen).toBeFalse();
  } finally { h.stop(); }
});

test("F10 and F5 do nothing for an assistant", () => {
  const h = shell(140, 46, () => snapshot(), { identity: assistant });
  try {
    expect(h.panels.handleInput(KEY.f10)).toBeFalse(); expect(h.panels.workbench.view).toBe("conversation");
    expect(h.panels.handleInput(KEY.f5)).toBeFalse(); expect(h.panels.workbench.view).toBe("conversation");
  } finally { h.stop(); }
});

test("every surface renders within its frame at the contract's sizes", () => {
  const state = snapshot({ streaming: true, cost: 1.25,
    helpers: [helper("first"), helper("second", { phase: "running", endedAt: undefined, result: undefined,
      messages: [{ id: "t", kind: "tool", tool: "read", phase: "running", arguments: JSON.stringify({ path: "src/a.ts" }), text: "" }] }),
      helper("third", { phase: "queued", startedAt: undefined, endedAt: undefined, result: undefined, messages: [] })],
    jobs: [job("build", "running"), job("lint", "exited", 1)],
    queue: [{ key: "q1", text: "Steer once ".repeat(12), policy: "steer", editable: true }, { key: "q2", text: "Follow once", policy: "follow_up", editable: true },
      { key: "n1", text: "wake", policy: "follow_up", editable: false }],
    ledger: { revision: 2, spec_revision: "spec", phases: [{ id: "p", label: "Build", steps: [
      { id: "a", label: "Write the plan", status: { state: "done" }, criteria: [] },
      { id: "b", label: "Wire the workbench", status: { state: "in_progress" }, criteria: [] },
      { id: "c", label: "Verify the frames", status: { state: "pending" }, criteria: [] }] }] } });
  const views: readonly MainView[] = ["conversation", "task", "agents", "changes", "evidence", "artifacts", "usage", "timeline"];
  for (const [columns, rows] of [[185, 40], [140, 46], [105, 30], [92, 40], [80, 24], [80, 14], [61, 40], [52, 24], [46, 40]] as const) {
    const h = shell(columns, rows, () => state);
    try {
      const main = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
        widgetsAbove: h.dock, editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: h.footer, mainView: transcript => h.panels.mainView(transcript) });
      const check = (label: string) => {
        const width = h.terminal.columns;
        expect(width).toBe(columns);
        // Twice: the rail takes the height of the first layout in the second.
        renderLayoutFrame(main.root, width, rows, () => {});
        const lines = renderLayoutFrame(main.root, width, rows, () => {}).lines;
        expect(lines).toHaveLength(rows);
        for (const line of lines) if (visibleWidth(line) > width) throw new Error(`${label} at ${columns}x${rows} overflows: ${stripTerminalSequences(line)}`);
        if (h.panels.wide) for (const line of h.panels.renderSidebar(h.terminal.railWidth, rows)) expect(visibleWidth(line)).toBe(h.terminal.railWidth);
      };
      for (const view of views) { h.panels.workbench.setView(view); check(view); }
      h.panels.workbench.openAgent("helper:second"); check("agent");
      h.panels.workbench.setView("conversation");
      expect(h.panels.handleInput(KEY.ctrlB)).toBeTrue(); check("sidebar toggled");
      h.panels.handleInput("\x1b[17~"); check("sidebar focus");
    } finally { h.stop(); }
  }
});

test("the frame: header, tabs, dock, status bar and footer span the full width; the rail only the rows between", () => {
  for (const [columns, rows] of [[185, 40], [140, 46], [105, 30]] as const) {
    const h = shell(columns, rows, () => snapshot({ helpers: [helper("first")], cost: 0.5 }));
    try {
      const main = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
        widgetsAbove: h.dock, editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: h.footer, mainView: transcript => h.panels.mainView(transcript) });
      renderLayoutFrame(main.root, columns, rows, () => {});
      const lines = renderLayoutFrame(main.root, columns, rows, () => {}).lines.map(line => stripTerminalSequences(line));
      const rail = h.terminal.mainColumns, at = (row: number) => sliceByColumn(lines[row]!, rail, 1);
      expect(rail).toBe(columns - h.terminal.railWidth - 1);
      // Header row 1's state word sits at the terminal's right edge, past the rail's column.
      expect(visibleWidth(lines[0]!.trimEnd())).toBeGreaterThan(rail + 1);
      expect(lines[2]).toContain("F9");
      expect(lines[3]).toBe("─".repeat(columns));
      const composer = lines.findIndex(line => line.includes("REAL_EDITOR_DRAFT"));
      expect(composer).toBeGreaterThan(4);
      for (let row = 4; row < composer; row++) expect(at(row)).toBe("│");
      expect(at(0)).not.toBe("│"); expect(at(3)).toBe("─");
      // Below the composer: the status bar with the role pill, a rule, the footer.
      expect(lines[rows - 3]).toContain("WORKER");
      expect(lines[rows - 2]).toBe("─".repeat(columns));
      expect(lines[rows - 1]!.trimEnd()).toMatch(/F1\s+keys$/);
    } finally { h.stop(); }
  }
  // Below 105 columns: no rail; the collapsed sidebar row above the composer. A settled helper leaves no summary,
  // so the row is set into its own rule: the full width, with the tab labels and the toggle key.
  const h = shell(92, 40, () => snapshot({ helpers: [helper("first")] }));
  try {
    expect(h.panels.wide).toBeFalse();
    const dock = plain(h.dock.render(92)).split("\n");
    expect(visibleWidth(dock[0]!)).toBe(92);
    expect(dock[0]!.startsWith("─ Context")).toBeTrue();
    for (const label of ["Files", "Plan"]) expect(dock[0]).toContain(label);
    expect(dock[0]!.trimEnd()).toMatch(/─ +(Ctrl|Alt)\+B  ▴$/);
  } finally { h.stop(); }
});

test("a one-line notice takes header row 1's state place and is kept with its time; Agent details opens the pager", async () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    h.panels.workbench.notice("Saved the plan", "success");
    expect(h.panels.workbench.currentNotice?.text).toBe("Saved the plan");
    expect(h.panels.workbench.recentNotices().at(-1)?.text).toBe("Saved the plan");
    const main = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      widgetsAbove: h.dock, editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: h.footer, mainView: transcript => h.panels.mainView(transcript) });
    const header = plain(renderLayoutFrame(main.root, 140, 46, () => {}).lines.slice(0, 1));
    expect(header).toContain("Saved the plan"); expect(h.panels.workbench.noticeInHeader).toBeTrue();
    const details = h.panels.actions().find(action => action.id === "agents.details")!;
    expect(details.label).toBe("Agent details"); expect(details.shortcut).toBe("");
    expect(h.panels.workbench.selectedAgent()).toBeUndefined();
    expect(await h.panels.workbench.openAgentDetails()).toBeFalse();
    h.panels.workbench.openAgents("helper:first");
    expect(h.panels.workbench.selectedAgent()?.meta.length).toBeGreaterThan(0);
    void h.panels.workbench.openAgentDetails(); await settle();
    expect(plain(h.customs.at(-1)?.render(100) ?? [])).toContain("model");
  } finally { h.stop(); }
});

test("Open an agent by id reads an evicted ID and a stale completion cannot replace the selected agent", async () => {
  const state = snapshot({ helpers: [helper("first", { phase: "completed" })] });
  const reads: string[] = [];
  const deferred = Promise.withResolvers<TranscriptPage>();
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => state, changed() {}, canInteract: () => true,
    readTranscript: (id, request) => { reads.push(`${id}:${request.mode}`); return deferred.promise; } });
  bindUi(wb, { input: async () => "evicted-agent" });
  const view = viewport(wb);
  const frame = () => plain(renderLayoutFrame(view.root, 100, 40, () => {}).lines);
  try {
    wb.openAgents("retained-picker");
    expect(frame()).toContain("Open an agent by id");
    expect(wb.handleInput(KEY.enter)).toBeTrue(); await settle();
    expect(reads).toEqual(["evicted-agent:conversation"]);
    expect(wb.view).toBe("agent");
    wb.openAgent("helper:first");
    deferred.resolve({ stamp: "stable", rows: [{ id: "1", kind: "user", text: "STALE_RECORDING" }] }); await settle();
    expect(frame()).not.toContain("STALE_RECORDING");
    expect(frame()).toContain("Find first callers");
    expect(frame()).toContain("REAL_EDITOR_DRAFT");
  } finally { wb.dispose(); }
});

test("raw transcript n and p request opaque host cursors without replacing the draft", async () => {
  const reads: (string | undefined)[] = [];
  const state = snapshot({ helpers: [helper("first")], agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: null, transcript: true }] });
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => state, changed() {}, canInteract: () => true,
    readTranscript: async (_id, request) => {
      expect(request.mode).toBe("raw"); reads.push(request.cursor);
      return { stamp: "same", rows: [{ id: "raw", kind: "raw", text: request.cursor === "next:opaque" ? "TAIL_TEXT" : "HEADER_TEXT" }],
        ...(request.cursor === "next:opaque" ? { older: "previous:opaque" } : { newer: "next:opaque" }) };
    } });
  const overlay = { current: undefined as Component | undefined };
  bindUi(wb, { custom: <T>(make: (tui: never, theme: never, keys: never, done: (value: T) => void) => Component | Promise<Component>) => {
    const { promise, resolve } = Promise.withResolvers<T>();
    overlay.current = make({ terminal: { rows: 30 }, requestRender() {} } as never, undefined as never, undefined as never, resolve) as Component;
    return promise;
  } });
  const view = viewport(wb);
  try {
    wb.openAgents("helper:first");
    expect(wb.handleInput("t")).toBeTrue(); await settle();
    expect(plain(overlay.current!.render(80))).toContain("HEADER_TEXT");
    overlay.current!.handleInput?.("n"); await settle();
    expect(plain(overlay.current!.render(80))).toContain("TAIL_TEXT");
    overlay.current!.handleInput?.("p"); await settle();
    expect(plain(overlay.current!.render(80))).toContain("HEADER_TEXT");
    overlay.current!.handleInput?.(KEY.esc); await settle();
    expect(reads).toEqual([undefined, "next:opaque", "previous:opaque"]);
    expect(plain(renderLayoutFrame(view.root, 100, 40, () => {}).lines)).toContain("REAL_EDITOR_DRAFT");
  } finally { wb.dispose(); }
});

test("an empty recording keeps paging and opens settled native tool preview with Ctrl+O or v", async () => {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager({ "app.tools.expand": "ctrl+o" }));
  const state = snapshot({ helpers: [helper("first", { result: "UNRECORDED_RESULT", messages: [
    { id: "tool", kind: "tool", tool: "read", phase: "completed", text: "NATIVE_TOOL_OUTPUT", arguments: '{"path":"native.txt"}' },
  ] })], agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: null, transcript: true }] });
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => state, changed() {}, canInteract: () => true,
    readTranscript: async () => ({ stamp: "same", rows: [], older: "opaque" }) });
  const tui = new TuiAltScreen(new RecordingTerminal());
  const view = viewport(wb, undefined, () => ({ tui, markdownTheme: sdk.getMarkdownTheme(), markdownTransformers: [],
    outputPad: 1, renderers: () => undefined }));
  const screen = () => plain(renderLayoutFrame(view.root, 120, 40, () => {}).lines);
  try {
    wb.openAgents("helper:first"); wb.handleInput(KEY.enter); await settle();
    expect(screen()).toContain("Show earlier messages");
    expect(screen()).toContain("Show live preview");
    expect(screen()).not.toContain("Find first callers");
    expect(wb.handleInput(KEY.ctrlO)).toBeTrue();
    expect(screen()).toContain("Find first callers");
    expect(screen()).toContain("UNRECORDED_RESULT");
    expect(screen()).toContain("NATIVE_TOOL_OUTPUT");
    expect(screen()).not.toContain("Show earlier messages");
    expect(wb.handleInput(KEY.ctrlO)).toBeTrue();
    expect(screen()).not.toContain("NATIVE_TOOL_OUTPUT");
    const frame = renderLayoutFrame(view.root, 120, 40, () => {});
    const y = frame.lines.findIndex(line => stripTerminalSequences(line).includes("read native.txt"));
    expect(y).toBeGreaterThan(0);
    for (const box of getLayoutBoxesAt(frame, 3, y)) {
      const result = box.component.handleMouse?.({ type: "click", button: "left", x: 3 - box.rect.x,
        y: y - box.rect.y, screenX: 3, screenY: y, width: box.rect.width, height: box.rect.height,
        shift: false, alt: false, ctrl: false, clickCount: 1 });
      if (result?.handled) break;
    }
    expect(screen()).toContain("NATIVE_TOOL_OUTPUT");
    expect(wb.handleInput("v")).toBeTrue();
    expect(screen()).toContain("Show earlier messages");
    expect(screen()).not.toContain("UNRECORDED_RESULT");
    expect(wb.handleInput("v")).toBeTrue();
    expect(screen()).toContain("UNRECORDED_RESULT");
  } finally { setKeybindings(previous); wb.dispose(); }
});
