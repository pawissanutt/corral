import { expect, test } from "bun:test";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { getKeybindings, setKeybindings, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { TUI } from "@earendil-works/pi-tui";
import type { Api, Model, ModelsRefreshResult } from "@earendil-works/pi-ai";
import type { ModelSelectorRequest, OptionSelectorRequest, ThinkingSelectorRequest } from "@earendil-works/pi-coding-agent";
import { effortPicker, modelPicker, optionPicker } from "./model-picker.ts";
import type { OwnerDialogHost } from "./owner-dialog.ts";

const ENTER = "\r", ESC = "\x1b", DOWN = "\x1b[B", UP = "\x1b[A", PAGE_DOWN = "\x1b[6~", TAB = "\t", CTRL_S = "\x13", CTRL_U = "\x15";
const tui = { requestRender() {} } as unknown as TUI;
const host = (rows: number): OwnerDialogHost => ({ rows: () => rows, chromeRows: () => Math.floor(rows / 3), changed() {} });
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
const model = (provider: string, id: string, name = id) => ({ provider, id, name }) as unknown as Model<Api>;

function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}

/** Runs with Pi's stock bindings installed: the save keys are Pi's, not the TUI's. */
async function withPiKeys(run: () => void | Promise<void>): Promise<void> {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager());
  try { await run(); } finally { setKeybindings(previous); }
}

function modelRequest(models: Model<Api>[], extra: Partial<ModelSelectorRequest> = {}) {
  const selected: [string, boolean][] = [];
  const pending = Promise.withResolvers<ModelsRefreshResult>();
  let cancels = 0, signal: AbortSignal | undefined, available = models;
  const request: ModelSelectorRequest = {
    tui, current: models[1], scoped: [], models: () => available,
    refresh: received => { signal = received; return pending.promise; },
    select: (picked, persist) => { selected.push([picked.id, persist]); },
    cancel: () => { cancels++; },
    ...extra,
  };
  return {
    request, selected, cancels: () => cancels, signal: () => signal,
    refreshed: (next: Model<Api>[], result: ModelsRefreshResult = { aborted: false, errors: new Map() }) => {
      available = next;
      pending.resolve(result);
    },
  };
}

test("the model picker opens on the current model and commits once through Pi's select: Enter for the session, Pi's save key as default", () =>
  withPiKeys(() => {
    const models = [model("openai", "gpt-5"), model("anthropic", "claude-sonnet-4"), model("google", "gemini-3")];
    const once = modelRequest(models);
    const picker = modelPicker(once.request, host(24));
    expect(plain(picker.render(100)).some(line => line.includes("▌claude-sonnet-4"))).toBeTrue();
    picker.handleInput(ENTER); picker.handleInput(ENTER); picker.handleInput(CTRL_S);
    expect(once.selected).toEqual([["claude-sonnet-4", false]]);
    expect(once.signal()?.aborted).toBeTrue();

    // Pi's order: current, then by provider. The save key commits with persist; nothing after it reaches Pi.
    const saved = modelRequest(models);
    const other = modelPicker(saved.request, host(24));
    other.handleInput(DOWN); other.handleInput(CTRL_S); other.handleInput(ENTER); other.handleInput(ESC);
    expect(saved.selected).toEqual([["gemini-3", true]]);
    expect(saved.cancels()).toBe(0);
  }));

test("a refresh keeps the selection on its model; a closed picker's refresh is aborted and changes nothing", () =>
  withPiKeys(async () => {
    const a = model("anthropic", "claude-sonnet-4"), b = model("openai", "gpt-5"), c = model("xai", "grok-5");
    const open = modelRequest([b, a], { current: a });
    const picker = modelPicker(open.request, host(24));
    picker.render(90);
    picker.handleInput(DOWN);
    open.refreshed([c, b, a], { aborted: false, errors: new Map([["xai", new Error("offline")]]) });
    await settle();
    picker.render(90);
    picker.handleInput(ENTER);
    expect(open.selected).toEqual([["gpt-5", false]]);

    const closed = modelRequest([b, a]);
    const gone = modelPicker(closed.request, host(24));
    gone.dispose?.();
    expect(closed.signal()?.aborted).toBeTrue();
    closed.refreshed([c]);
    await settle();
    gone.handleInput(ENTER);
    expect(closed.selected).toEqual([]);
    expect(closed.cancels()).toBe(0);
  }));

test("Tab switches between all and scoped models only when Pi passed a scope; Pi's initial search opens filtered", () =>
  withPiKeys(() => {
    const a = model("anthropic", "claude-sonnet-4"), b = model("openai", "gpt-5"), c = model("xai", "grok-5");
    const scoped = modelRequest([a, b, c], { current: a, scoped: [{ model: c }] });
    const picker = modelPicker(scoped.request, host(24));
    let shown = plain(picker.render(90)).join("\n");
    expect(shown).toContain("scoped models");
    expect(shown).not.toContain("gpt-5");
    picker.handleInput(TAB);
    shown = plain(picker.render(90)).join("\n");
    expect(shown).toContain("all models");
    expect(shown).toContain("gpt-5");

    const searched = modelRequest([a, b, c], { current: a, initialSearch: "grok" });
    const filtered = modelPicker(searched.request, host(24));
    expect(plain(filtered.render(90)).join("\n")).not.toContain("gpt-5");
    filtered.handleInput(ENTER);
    expect(searched.selected).toEqual([["grok-5", false]]);
  }));

test("the effort picker shows Pi's levels and descriptions, opens on the current level and makes a default only by Pi's key", () =>
  withPiKeys(() => {
    const picked: [string, boolean][] = [];
    const request: ThinkingSelectorRequest = {
      current: "medium", defaultLevel: "high", levels: ["off", "low", "medium", "high"],
      describe: level => `${level} reasoning described by Pi`,
      select: (level, persist) => { picked.push([level, persist]); }, cancel() {},
    };
    const picker = effortPicker(request, host(24));
    const shown = plain(picker.render(100)).join("\n");
    for (const level of request.levels) expect(shown).toContain(`${level} reasoning described by Pi`);
    expect(shown).toContain("▌medium");
    picker.handleInput(DOWN); picker.handleInput(CTRL_S); picker.handleInput(ENTER);
    expect(picked).toEqual([["high", true]]);
  }));

test("an option list previews only when the owner navigates, never on open or a filter change, and Esc is Pi's own back, once", () => {
  const previews: string[] = [], chosen: string[] = [];
  let cancels = 0;
  const request: OptionSelectorRequest = {
    title: "Theme", description: "Select a theme", current: "light", searchable: true,
    items: [{ value: "dark", label: "  dark" }, { value: "light", label: "✓ light" }, { value: "solarized", label: "  solarized" }],
    select: value => { chosen.push(value); }, cancel: () => { cancels++; }, preview: value => { previews.push(value); },
  };
  const picker = optionPicker(request, host(24));
  expect(plain(picker.render(80)).some(line => line.includes("▌✓ light"))).toBeTrue();
  expect(previews).toEqual([]);
  // Typing a filter and clearing it (Ctrl+U) moves the selection to the first match without previewing it, as Pi's list does.
  picker.handleInput("d"); picker.handleInput("a"); picker.handleInput(CTRL_U);
  expect(previews).toEqual([]);
  picker.handleInput(DOWN); picker.handleInput(UP);
  picker.handleInput(ESC); picker.handleInput(ESC); picker.handleInput(ENTER);
  expect(previews).toEqual(["light", "dark"]);
  expect(cancels).toBe(1);
  expect(chosen).toEqual([]);
});

test("at 44 columns and 12 rows every row fits and the selected model's long name and reference are readable before any commit", () =>
  withPiKeys(() => {
    const long = model("openrouter", `vendor/HEADMARK-${"x".repeat(60)}`, "A very long display name ending in TAILMARK");
    const open = modelRequest([model("anthropic", "claude-sonnet-4"), long], { current: undefined });
    const picker = modelPicker(open.request, host(12));
    picker.handleInput(DOWN);
    const seen: string[] = [];
    for (let page = 0; page < 16; page++) {
      const lines = picker.render(44);
      for (const line of lines) expect(visibleWidth(line)).toBe(44);
      seen.push(...plain(lines));
      picker.handleInput(PAGE_DOWN);
    }
    const text = seen.join(" ");
    expect(text).toContain("TAILMARK");
    expect(text).toContain("HEADMARK");
    expect(open.selected).toEqual([]);
  }));

test("favorites list first, also while filtering; Alt+S marks one without choosing it, keyed by provider and id", () =>
  withPiKeys(() => {
    const ALT_S = "\x1bs";
    const kept = new Set<string>();
    const favorites = {
      has: (m: { provider: string; id: string }) => kept.has(`${m.provider}|${m.id}`),
      set: (m: { provider: string; id: string }, on: boolean) => { if (on) kept.add(`${m.provider}|${m.id}`); else kept.delete(`${m.provider}|${m.id}`); },
    };
    const models = [model("anthropic", "claude-sonnet-4"), model("anthropic-api", "claude-sonnet-4"), model("openai", "gpt-5")];
    const once = modelRequest(models, { current: models[0] });
    const picker = modelPicker(once.request, host(24), favorites);
    picker.render(100);
    picker.handleInput(DOWN);
    picker.handleInput(ALT_S);
    expect([...kept]).toEqual(["anthropic-api|claude-sonnet-4"]);
    expect(once.selected).toEqual([]);
    for (const key of "claude") picker.handleInput(key);
    const rows = plain(picker.render(100)).filter(line => line.includes("claude-sonnet-4") && !line.includes("·"));
    expect(rows[0]).toContain("★");
    expect(rows[0]).toContain("anthropic-api");
    expect(rows[1]).not.toContain("★");
    picker.handleInput(ENTER);
    expect(once.selected).toEqual([["claude-sonnet-4", false]]);
  }));
