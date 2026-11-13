import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { getKeybindings, setKeybindings, stripTerminalSequences, Text, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { NativePanels, type NativePanelsOptions } from "./panels.ts";
import { NativeWorkbench, type MainView } from "./workbench.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { PanelJob, PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import type { QueueControl } from "../host/queue-control.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

beforeAll(() => sdk.initTheme("dark", false));
const work = mkdtempSync(join(tmpdir(), "corral-workbench-shell-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

const KEY = { f1: "\x1bOP", f8: "\x1b[19~", f9: "\x1b[20~", f10: "\x1b[21~", f5: "\x1b[15~", esc: "\x1b", enter: "\r", down: "\x1b[B", pageDown: "\x1b[6~", ctrlB: "\x02", ctrlO: "\x0f" };
const worker = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
const assistant = { store: "st-0123456789abcdef", project: "demo", role: "assistant" as const, assistant: "A-7", generation: 1 };
const NOW = Date.now();

function helper(request: string, extra: Partial<HelperInspection> = {}): HelperInspection {
  return { request, kind: "explore", model: "provider/model-a", generation: 1, phase: "completed", createdAt: NOW - 60_000, startedAt: NOW - 58_000,
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
function viewport(wb: NativeWorkbench, draft = "REAL_EDITOR_DRAFT") {
  let supplied: Component | undefined;
  const view = createChatViewport({ document: new Text(Array.from({ length: 120 }, (_, i) => `PARENT_${i}`).join("\n"), 0, 0),
    pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0), editor: new Text(draft, 0, 0), footer: new Text("", 0, 0),
    mainView: transcript => { supplied = transcript; return wb.mount(transcript); } });
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
  } as unknown as ExtensionAPI;
  const tui = { terminal: { rows }, requestRender() {} }, theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
  let draft = "", dock: Component | undefined, footer: Component | undefined;
  const customs: Component[] = [];
  const ui = {
    theme: { getColorMode: () => "truecolor" }, setTheme() {}, setEditorComponent() {}, setHeader() {}, notify() {},
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
    prompt: (event: "ui_prompt_start" | "ui_prompt_end") => handlers.get(event)?.({}, context),
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

test("t refuses an ineligible transcript with a notice and never reads; an eligible one reads by its own id", async () => {
  let current = snapshot({ helpers: [helper("first")], agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: "model-a", transcript: false }] });
  const reads: string[] = [];
  const wb = new NativeWorkbench({ cwd: work, identity: assistant, snapshot: () => current, changed() {}, canInteract: () => true,
    readTranscript: async id => { reads.push(id); return "EXISTING_TRANSCRIPT"; } });
  bindUi(wb);
  wb.mount(new Text("PARENT", 0, 0));
  try {
    wb.handleInput(KEY.f8);
    expect(wb.handleInput("t")).toBeTrue(); await settle();
    expect(reads).toEqual([]); expect(wb.currentNotice?.text).toContain("Transcript unavailable");
    current = { ...current, agents: [{ id: "first", session: "s-1", kind: "explore", state: "completed", model: "model-a", transcript: true }] };
    expect(wb.handleInput("t")).toBeTrue(); await settle();
    expect(reads).toEqual(["first"]);
    current = { ...current, bound: false };
    wb.handleInput("t"); await settle();
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

test("the footer lists the keys of what is open: the palette, the keys list, a pager, or a Pi dialog", async () => {
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
    // A prompt the panels did not open is Pi's dialog.
    h.prompt("ui_prompt_start");
    expect(footer()).toMatch(/⏎\s+choose/); expect(footer()).toMatch(/esc\s+cancel/);
    h.prompt("ui_prompt_end");
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
    expect(plain(drawer.slice(0, 1))).toContain("▾"); expect(drawer.length).toBeGreaterThan(1);
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

test("a view's letters that the selection does not offer do nothing; undefined letters go on to the composer", () => {
  const h = shell(140, 46, () => snapshot({ helpers: [helper("first")] }), { identity: assistant });
  try {
    h.setDraft("my draft");
    expect(h.panels.handleInput(KEY.f8)).toBeTrue();
    expect(h.panels.workbench.agentsView.entry()?.row.key).toBe("helper:first");
    for (const letter of ["s", "x", "m"]) {
      expect(h.panels.handleInput(letter)).toBeTrue();
      expect(h.panels.focus).toBe("main"); expect(h.panels.workbench.view).toBe("agents");
    }
    expect(h.draft()).toBe("my draft");
    expect(h.panels.handleInput("z")).toBeFalse(); expect(h.panels.focus).toBe("composer");
  } finally { h.stop(); }
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

test("every surface renders within its frame at 140x46, 80x24, 80x14 and 52x24", () => {
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
  const views: readonly MainView[] = ["conversation", "task", "agents", "changes", "evidence", "artifacts", "inbox", "usage", "graph", "timeline"];
  for (const [columns, rows] of [[140, 46], [80, 24], [80, 14], [52, 24]] as const) {
    const h = shell(columns, rows, () => state);
    try {
      const main = createChatViewport({ document: new Text("PARENT_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
        widgetsAbove: h.dock, editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: h.footer, mainView: transcript => h.panels.mainView(transcript) });
      const check = (label: string) => {
        const width = h.terminal.columns;
        const lines = renderLayoutFrame(main.root, width, rows, () => {}).lines;
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
