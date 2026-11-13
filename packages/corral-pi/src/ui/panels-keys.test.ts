import { afterAll, beforeAll, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { Component } from "@earendil-works/pi-tui";
import { NativePanels, type NativePanelsOptions } from "./panels.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import type { PendingQuestion } from "../host/pending-questions.ts";
import { HERDR_CONFIG_MAX_BYTES, readHerdrConfig, sidebarToggleKey } from "./herdr-keys.ts";
import { kittyAlt } from "./panel-terminal.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

beforeAll(() => sdk.initTheme("dark", false));
const work = mkdtempSync(join(tmpdir(), "corral-panels-keys-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

const KEY = { f6: "\x1b[17~", f8: "\x1b[19~", esc: "\x1b", ctrlB: "\x02", altB: "\x1bb" };
const worker = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 3 };
function snapshot(extra: Partial<PanelSnapshot> = {}): PanelSnapshot {
  return { bound: true, model: "openai-codex/example", effort: "high", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [],
    skills: [], ledger: null, ledgerStale: false, jobs: [], auxiliary: null, steering: 0, followUps: 0, helpers: [], ...extra };
}
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}

/** NativePanels on a recording terminal with a minimal UI context that records the terminal titles it sets. */
function shell(columns: number, rows: number, state: () => PanelSnapshot, options: Partial<NativePanelsOptions> = {}) {
  const panels = new NativePanels({ identity: worker, cwd: work, snapshot: state, ...options });
  const physical = new RecordingTerminal(); physical.resize(columns, rows);
  const terminal = panels.wrap(physical); terminal.start(() => {}, () => {});
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const pi = { on: (name: string, fn: (event: unknown, ctx: unknown) => unknown) => { handlers.set(name, fn); },
    registerCommand() {}, registerMarkdownTransformer() {} } as unknown as ExtensionAPI;
  const titles: string[] = [];
  const tui = { terminal: { rows }, requestRender() {} }, theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
  const ui = {
    theme: { getColorMode: () => "truecolor" }, setTheme() {}, setEditorComponent() {}, setHeader() {}, notify() {}, setWorkingVisible() {},
    setHiddenThinkingLabel() {}, setTitle: (title: string) => { titles.push(title); },
    setWidget: (_key: string, factory: (tui: unknown, theme: unknown) => Component) => { factory(tui, theme); },
    setFooter: (factory: (tui: unknown, theme: unknown, data: unknown) => Component) => {
      factory(tui, theme, { onBranchChange: () => () => {}, getExtensionStatuses: () => new Map() });
    },
    getEditorText: () => "", setEditorText() {}, custom: () => Promise.withResolvers<unknown>().promise,
    confirm: async () => true, input: async () => undefined, select: async () => undefined,
  };
  const context = { mode: "tui", ui };
  panels.install(pi, sdk);
  handlers.get("session_start")?.({}, context);
  return { panels, physical, titles, prompt: () => handlers.get("ui_prompt_start")?.({}, context), stop() { terminal.stop(); panels.dispose(); } };
}

test("Esc out of a view is guarded: a second Esc right after it never reaches Pi's interrupt; a lone Esc in Chat does", () => {
  let now = 1_000_000;
  const h = shell(140, 46, () => snapshot({ streaming: true }), { scheduler: { now: () => now, every: () => () => {} } });
  try {
    expect(h.panels.handleInput(KEY.f8)).toBeTrue(); expect(h.panels.workbench.view).toBe("agents");
    expect(h.panels.handleInput(KEY.esc)).toBeTrue(); expect(h.panels.workbench.view).toBe("conversation");
    // Back in Chat while the turn runs: the notice says the next Esc stops it, and an Esc within the guard is ignored.
    expect(h.panels.workbench.currentNotice?.text).toContain("again stops the turn");
    expect(h.panels.handleInput(KEY.esc)).toBeTrue();
    now += 700;
    expect(h.panels.handleInput(KEY.esc)).toBeFalse();
  } finally { h.stop(); }
  const fresh = shell(140, 46, () => snapshot({ streaming: true }));
  try { expect(fresh.panels.handleInput(KEY.esc)).toBeFalse(); } finally { fresh.stop(); }
});

test("the terminal title is set once per attention change, and the bell rings only when it is on", async () => {
  let streaming = false, waiting: PendingQuestion[] = [];
  const question = { id: "Q-1", status: "pending", expires: Number.MAX_SAFE_INTEGER,
    questions: [{ question: "Ship it?", options: ["Yes", "No"] }] } as unknown as PendingQuestion;
  const h = shell(140, 46, () => snapshot({ streaming }));
  const bells = () => h.physical.writes.filter(write => write === "\x07").length;
  try {
    h.panels.attachQuestions({ waiting: () => waiting, open() {}, subscribe: () => () => {} });
    await settle();
    expect(h.titles).toHaveLength(1);
    h.panels.refresh(); await settle(); h.panels.refresh(); await settle();
    expect(h.titles).toHaveLength(1);
    streaming = true; h.panels.refresh(); await settle();
    expect(h.titles).toHaveLength(2); expect(h.titles[1]).toContain("working");
    waiting = [question]; h.panels.refresh(); await settle();
    expect(h.titles.at(-1)).toContain("needs you"); expect(bells()).toBe(0);
    waiting = []; h.panels.refresh(); await settle();
    h.panels.bell = "on";
    waiting = [question]; h.panels.refresh(); await settle(); h.panels.refresh(); await settle();
    expect(bells()).toBe(1); expect(h.titles).toHaveLength(5);
  } finally { h.stop(); }
});

test("only the chosen sidebar key toggles: Alt+B leaves Ctrl+B to the editor; an F6 drawer closes when a Pi dialog opens", () => {
  const herdr = shell(140, 46, () => snapshot(), { sidebarKey: "alt+b" });
  try {
    expect(herdr.panels.handleInput(KEY.ctrlB)).toBeFalse(); expect(herdr.panels.sidebarEnabled).toBeTrue();
    expect(herdr.panels.handleInput(KEY.altB)).toBeTrue(); expect(herdr.panels.sidebarEnabled).toBeFalse();
  } finally { herdr.stop(); }
  const narrow = shell(80, 24, () => snapshot());
  try {
    expect(narrow.panels.handleInput(KEY.f6)).toBeTrue(); expect(narrow.panels.sidebar.drawer).toBeTrue();
    narrow.prompt();
    expect(narrow.panels.sidebar.drawer).toBeFalse(); expect(narrow.panels.focus).toBe("composer");
  } finally { narrow.stop(); }
});

test("the sidebar key is Alt+B inside herdr while its prefix is Ctrl+B or unset, Ctrl+B otherwise", () => {
  const env = { HERDR_ENV: "1", HOME: "/home/owner" };
  expect(sidebarToggleKey(env, () => "[keys]\nprefix = \"ctrl+b\"\n")).toBe("alt+b");
  expect(sidebarToggleKey(env, () => "[ui]\ntheme = 'dark'\n")).toBe("alt+b");
  expect(sidebarToggleKey(env, () => { throw new Error("ENOENT"); })).toBe("alt+b");
  expect(sidebarToggleKey(env, () => "[keys]\nprefix = \"ctrl+a\"\n")).toBe("ctrl+b");
  expect(sidebarToggleKey({ HOME: "/home/owner" }, () => "[keys]\nprefix = \"ctrl+b\"\n")).toBe("ctrl+b");
});

test("a bracketed paste while a view has the keys goes to the composer unchanged and takes the keys there", () => {
  const h = shell(140, 46, () => snapshot());
  try {
    expect(h.panels.handleInput(KEY.f8)).toBeTrue(); expect(h.panels.focus).toBe("main");
    expect(h.panels.handleInput("\x1b[200~make it blue\x1b[201~")).toBeFalse();
    expect(h.panels.focus).toBe("composer");
  } finally { h.stop(); }
});

test("herdr's config is read only as a bounded regular file; anything else keeps the safe default", () => {
  const dir = join(work, "herdr");
  mkdirSync(dir, { recursive: true });
  const small = join(dir, "config.toml"), large = join(dir, "large.toml");
  writeFileSync(small, "[keys]\nprefix = \"ctrl+a\"\n");
  writeFileSync(large, "#".repeat(HERDR_CONFIG_MAX_BYTES + 1));
  expect(readHerdrConfig(small)).toContain("ctrl+a");
  expect(readHerdrConfig(large)).toBeUndefined();
  expect(readHerdrConfig(dir)).toBeUndefined();
  expect(readHerdrConfig(join(dir, "missing.toml"))).toBeUndefined();
  if (process.platform !== "win32") {
    // A FIFO at the config path would block a plain read forever; it opens without blocking and is refused.
    const fifo = join(dir, "fifo.toml");
    execFileSync("mkfifo", [fifo]);
    expect(readHerdrConfig(fifo)).toBeUndefined();
  }
  expect(sidebarToggleKey({ HERDR_ENV: "1", XDG_CONFIG_HOME: work })).toBe("ctrl+b");
});

test("a replayed Alt chord takes the Kitty form while that protocol is active; other keys stay as they are", () => {
  expect(kittyAlt("\x1bb")).toBe("\x1b[98;3u");
  expect(kittyAlt("\x1b1")).toBe("\x1b[49;3u");
  expect(kittyAlt("\x1bB")).toBe("\x1b[98;4u");
  // The follow-up hint `Alt+⏎ queue` replays `ESC CR`.
  expect(kittyAlt("\x1b\r")).toBe("\x1b[13;3u");
  for (const data of ["\x02", "\x1b[19~", "\x1b[1;3B", "a"]) expect(kittyAlt(data)).toBe(data);
});
