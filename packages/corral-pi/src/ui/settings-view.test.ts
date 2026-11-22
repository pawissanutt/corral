import { expect, test } from "bun:test";
import { stripTerminalSequences, Text, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import type { Role } from "../roles/types.ts";
import { NativeWorkbench } from "./workbench.ts";
import {
  SettingsPage, settingsKeys, type PersonalSettings, type SettingsControl, type SettingsSnapshot, type StartupModelOption,
} from "./settings-view.ts";

const ENTER = "\r", ESC = "\x1b", DOWN = "\x1b[B", UP = "\x1b[A", F3 = "\x1bOR", BACKSPACE = "\x7f";
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const flush = () => new Promise<void>(resolve => setTimeout(resolve, 0));

const MODELS: readonly StartupModelOption[] = [
  { selector: "anthropic/claude-opus-5-5", provider: "anthropic", id: "claude-opus-5-5", label: "Anthropic · Claude subscription" },
  { selector: "openai/gpt-6", provider: "openai", id: "gpt-6", label: "OpenAI · API key" },
];
const PERSONAL: PersonalSettings = { appearance: "dark", density: "comfortable", motion: "animate", bell: "off" };

/** A host whose writes are recorded; `fail` makes the next write reject. */
function host(snapshot: Partial<SettingsSnapshot> = {}, options: { readonly role?: Role; readonly fail?: string } = {}) {
  const startup: (string | null)[] = [], personal: Partial<PersonalSettings>[] = [], applied: Partial<PersonalSettings>[] = [];
  let fail = options.fail, closes = 0, session: PersonalSettings = { ...PERSONAL };
  const control: SettingsControl = {
    read: async () => ({ personal: PERSONAL, workspace: { editable: true, name: "corral", startupModel: "anthropic/claude-opus-5-5" }, ...snapshot }),
    listStartupModels: async () => MODELS,
    setStartupModel: async selector => {
      if (fail) { const reason = fail; fail = undefined; throw new Error(reason); }
      startup.push(selector);
    },
    setPersonal: async patch => {
      if (fail) { const reason = fail; fail = undefined; throw new Error(reason); }
      personal.push(patch);
    },
  };
  const page = new SettingsPage({
    control, role: options.role ?? "assistant", session: () => session,
    apply: patch => { applied.push(patch); session = { ...session, ...patch }; }, changed() {},
  });
  page.open(() => { closes++; });
  return { page, startup, personal, applied, closes: () => closes };
}

test("one page groups the startup model and the personal Display defaults, the detail beside the list when wide", async () => {
  const { page } = host();
  await flush();
  const lines = plain(page.render(120, 30, "Chat"));
  const text = lines.join("\n");
  expect(lines[0]).toContain("‹ Chat · Settings");
  expect(text).toContain("STARTUP · corral");
  expect(text).toContain("PERSONAL · across workspaces");
  for (const label of ["Startup model", "Appearance", "Density", "Motion", "Attention bell"]) expect(text).toContain(label);
  // Beside: the selected setting's row and its exact model and billing route share rows of the page.
  expect(lines.some(line => line.includes("Startup model") && line.includes("│"))).toBeTrue();
  expect(text).toContain("anthropic/claude-opus-5-5");
  expect(text).toContain("Anthropic · Claude subscription");
  expect(text).not.toMatch(/effort|thinking/i);
});

test("narrow, the detail sits under the list and every row fits", async () => {
  const { page } = host();
  await flush();
  for (const width of [30, 44, 60, 83]) {
    const lines = page.render(width, 0, "Chat");
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const text = plain(lines);
    const list = text.findIndex(line => line.includes("Attention bell")), detail = text.findIndex(line => line.includes("STARTUP MODEL"));
    expect(detail).toBeGreaterThan(list);
    expect(text.some(line => line.includes("│"))).toBeFalse();
  }
});

test("Enter only opens the choices; Enter on a choice saves that one field and applies it to this terminal", async () => {
  const { page, personal, applied } = host();
  await flush();
  page.handleInput(DOWN);
  page.handleInput(ENTER);
  expect(personal).toEqual([]);
  expect(plain(page.render(120, 30, "Chat")).join("\n")).toContain("CHOOSE");
  page.handleInput(DOWN);
  page.handleInput(ENTER);
  await flush();
  expect(personal).toEqual([{ appearance: "light" }]);
  expect(applied).toEqual([{ appearance: "light" }]);
  const text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("Saved");
  expect(text).not.toContain("CHOOSE");
  // The same value again writes nothing.
  page.handleInput(ENTER); page.handleInput(ENTER);
  await flush();
  expect(personal).toHaveLength(1);
});

test("the startup model offers Ask at launch and the signed-in models with their billing route", async () => {
  const { page, startup } = host();
  await flush();
  page.handleInput(ENTER);
  const text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("Ask at launch");
  expect(text).toContain("openai/gpt-6");
  expect(text).toContain("OpenAI · API key");
  page.handleInput(UP);
  page.handleInput(ENTER);
  await flush();
  expect(startup).toEqual([null]);
  expect(plain(page.render(120, 30, "Chat")).join("\n")).toContain("ask for a model at launch");
});

test("a saved model no signed-in provider offers reads as unavailable and cannot be chosen again", async () => {
  const { page, startup } = host({ workspace: { editable: true, name: "corral", startupModel: "gone/model-x" } });
  await flush();
  let text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("gone/model-x · unavailable");
  page.handleInput(ENTER);
  // The saved, unavailable model is highlighted: Enter explains instead of saving.
  page.handleInput(ENTER);
  await flush();
  expect(startup).toEqual([]);
  text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("No signed-in provider offers it now");
});

test("a worker never changes the startup model; personal settings stay available without a workspace", async () => {
  const worker = host({}, { role: "worker" });
  await flush();
  worker.page.handleInput(UP); worker.page.handleInput(ENTER); worker.page.handleInput(ENTER);
  await flush();
  expect(worker.startup).toEqual([]);
  expect(plain(worker.page.render(120, 30, "Chat")).join("\n")).toContain("worker session does not change startup settings");

  const outside = host({ workspace: { editable: false, reason: "This session is outside any workspace." } });
  await flush();
  // The selection starts on the first personal setting.
  outside.page.handleInput(ENTER); outside.page.handleInput(DOWN); outside.page.handleInput(ENTER);
  await flush();
  expect(outside.personal).toEqual([{ appearance: "light" }]);
});

test("F3 searches, typing does nothing without it, and Esc steps back: search, choices, then the page once", async () => {
  const { page, personal, closes } = host();
  await flush();
  page.handleInput("b"); page.handleInput("e");
  expect(plain(page.render(120, 30, "Chat")).join("\n")).toContain("Startup model");
  page.handleInput(F3);
  for (const ch of "bellx") page.handleInput(ch);
  page.handleInput(BACKSPACE);
  let text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("search › bell");
  expect(text).not.toContain("Appearance");
  page.handleInput(ENTER);
  text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("CHOOSE");
  page.handleInput(ESC);
  expect(plain(page.render(120, 30, "Chat")).join("\n")).not.toContain("CHOOSE");
  page.handleInput(ESC);
  expect(closes()).toBe(1);
  page.handleInput(ESC); page.handleInput(ENTER);
  expect(closes()).toBe(1);
  expect(personal).toEqual([]);
});

test("a failed save says why and changes nothing here", async () => {
  const { page, personal, applied } = host({}, { fail: "config is read-only" });
  await flush();
  page.handleInput(DOWN); page.handleInput(ENTER); page.handleInput(DOWN); page.handleInput(ENTER);
  await flush();
  expect(personal).toEqual([]);
  expect(applied).toEqual([]);
  const text = plain(page.render(120, 30, "Chat")).join("\n");
  expect(text).toContain("Not saved · config is read-only");
  expect(text).toContain("Dark");
});

test("a short view keeps the highlighted model among the choices it shows", async () => {
  const many = Array.from({ length: 30 }, (_, i) => ({ selector: `p/m${i}`, provider: "p", id: `m${i}`, label: "P · API" }));
  const control: SettingsControl = {
    read: async () => ({ personal: PERSONAL, workspace: { editable: true, name: "corral", startupModel: "p/m20" } }),
    listStartupModels: async () => many, setStartupModel: async () => {}, setPersonal: async () => {},
  };
  const page = new SettingsPage({ control, role: "assistant", session: () => PERSONAL, apply() {}, changed() {} });
  page.open(() => {});
  await flush();
  page.handleInput(ENTER);
  const lines = plain(page.render(100, 20, "Chat"));
  expect(lines.length).toBeLessThanOrEqual(20);
  expect(lines.join("\n")).toContain("p/m20");
  expect(lines.join("\n")).toContain("more");
});

test("Settings is a main view over the open one: the transcript and draft stay, and closing returns to the view under it", async () => {
  const { page } = host();
  const wb = new NativeWorkbench({ cwd: "/tmp", identity: { store: "s", project: "p", role: "assistant", assistant: "A-test" },
    snapshot: () => null, changed() {}, canInteract: () => false, settingsPage: page });
  let supplied: Component | undefined;
  const viewport = createChatViewport({ document: new Text("REAL_TRANSCRIPT", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
    editor: new Text("REAL_EDITOR_DRAFT", 0, 0), footer: new Text("REAL_FOOTER", 0, 0), mainView: transcript => { supplied = transcript; return wb.mount(transcript); } });
  try {
    await flush();
    const screen = () => stripTerminalSequences(renderLayoutFrame(viewport.root, 120, 30, () => {}).lines.join("\n"));
    expect(supplied).toBe(viewport.transcript);
    wb.setView("usage");
    wb.openSettings();
    let shown = screen();
    expect(shown).toContain("‹ Chat › Usage · Settings");
    expect(shown).toContain("PERSONAL · across workspaces");
    expect(shown).toContain("REAL_EDITOR_DRAFT");
    // A message arriving while the page shows changes the view under it, not the page.
    wb.setView("conversation");
    shown = screen();
    expect(wb.view).toBe("settings");
    expect(shown).toContain("‹ Chat · Settings");
    expect(shown).not.toContain("REAL_TRANSCRIPT");
    wb.closeSettings();
    expect(wb.view).toBe("conversation");
    expect(wb.focused).toBeFalse();
    expect(screen()).toContain("REAL_TRANSCRIPT");
  } finally { wb.dispose(); }
});

test("the key row holds Pi's focus for the page, replays a clicked key, and leaves once", async () => {
  const { page, closes } = host();
  await flush();
  let left = 0;
  const keys = settingsKeys(page, () => { left++; });
  const row = plain(keys.render(80))[0] ?? "";
  expect(row).toContain("choose");
  expect(row).toContain("search");
  keys.handleInput(ENTER);
  expect(plain(keys.render(80))[0]).toContain("save");
  const at = row.indexOf("choose");
  const click: TuiMouseEvent = { type: "click", button: "left", x: Math.max(0, at - 2), y: 0, screenX: 0, screenY: 0, width: 80, height: 1,
    shift: false, alt: false, ctrl: false };
  keys.handleMouse?.(click);
  keys.dispose?.(); keys.dispose?.();
  expect(left).toBe(1);
  expect(closes()).toBe(0);
});
