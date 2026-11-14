import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, SettingItem, TUI } from "@earendil-works/pi-tui";
import type { SettingsSelectorRequest } from "@earendil-works/pi-coding-agent";
import type { OwnerDialogHost } from "./owner-dialog.ts";
import { settingsCard } from "./settings-card.ts";

const ENTER = "\r", ESC = "\x1b", DOWN = "\x1b[B", PAGE_DOWN = "\x1b[6~";
const tui = { requestRender() {} } as unknown as TUI;
const host = (rows: number): OwnerDialogHost => ({ rows: () => rows, chromeRows: () => Math.floor(rows / 3), changed() {} });
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));

/** Pi's settings as Pi passes them: live values, Pi's change dispatch recording the value on the item. */
function settings(items: SettingItem[], extra: Partial<SettingsSelectorRequest> = {}) {
  const changes: [string, string][] = [];
  let cancels = 0;
  const request: SettingsSelectorRequest = {
    tui, items,
    change: (id, value) => {
      changes.push([id, value]);
      const item = items.find(candidate => candidate.id === id);
      if (item) item.currentValue = value;
    },
    cancel: () => { cancels++; },
    ...extra,
  };
  return { request, changes, cancels: () => cancels };
}

test("Enter cycles a value through Pi's change and the card stays open; Esc is Pi's close, once", () => {
  const pi = settings([
    { id: "autocompact", label: "Auto-compact", description: "Automatically compact context", currentValue: "true", values: ["true", "false"] },
    { id: "transport", label: "Transport", currentValue: "auto", values: ["sse", "websocket", "auto"] },
  ]);
  const card = settingsCard(pi.request, host(24));
  let shown = plain(card.render(90)).join("\n");
  expect(shown).toContain("Pi settings");
  expect(shown).toContain("Automatically compact context");
  card.handleInput(ENTER); card.handleInput(ENTER);
  card.handleInput(DOWN); card.handleInput(" ");
  expect(pi.changes).toEqual([["autocompact", "false"], ["autocompact", "true"], ["transport", "sse"]]);
  shown = plain(card.render(90)).join("\n");
  expect(shown).toContain("sse");
  card.handleInput(ESC); card.handleInput(ESC); card.handleInput(ENTER);
  expect(pi.cancels()).toBe(1);
  expect(pi.changes).toHaveLength(3);
});

test("a submenu is hosted in the card's place with the keys and focus until its done, whose value goes to Pi's change", () => {
  const typed: string[] = [];
  let finish: ((value?: string) => void) | undefined;
  const submenu: Component & { focused: boolean } = {
    focused: false,
    render: width => [`THEME MENU ${width}`],
    handleInput: data => { typed.push(data); },
    invalidate() {},
  };
  const pi = settings([
    { id: "hide-thinking", label: "Hide thinking", currentValue: "false", values: ["true", "false"] },
    { id: "theme", label: "Theme", currentValue: "dark", submenu: (_value, done) => { finish = done; return submenu; } },
  ]);
  const card = settingsCard(pi.request, host(24));
  card.focused = true;
  card.handleInput(DOWN); card.handleInput(ENTER);
  expect(plain(card.render(70))).toEqual(["THEME MENU 70"]);
  expect(submenu.focused).toBeTrue();
  card.handleInput(DOWN); card.handleInput(ESC);
  expect(typed).toEqual([DOWN, ESC]);
  expect(pi.cancels()).toBe(0);
  finish?.("light");
  expect(pi.changes).toEqual([["theme", "light"]]);
  const shown = plain(card.render(70)).join("\n");
  expect(shown).toContain("▌Theme");
  expect(shown).toContain("light ›");
  // A second done from the closed submenu changes nothing.
  finish?.("dracula");
  expect(pi.changes).toEqual([["theme", "light"]]);
});

test("a nested list is drawn with Pi's title and description, and its Esc is Pi's back", () => {
  const pi = settings([
    { id: "light-theme", label: "Light theme", currentValue: "light", submenu: () => ({ render: () => [], invalidate() {} }) },
    { id: "apply", label: "Apply", description: "Save and go back", currentValue: "save and go back", values: ["save and go back"] },
  ], { title: "Automatic Theme", description: "Choose themes for terminal light and dark appearance.\nLight/dark detection requires terminal support." });
  const card = settingsCard(pi.request, host(28));
  const shown = plain(card.render(100)).join("\n");
  expect(shown).toContain("Automatic Theme");
  expect(shown).toContain("Light/dark detection requires terminal support.");
  expect(shown).toContain("back");
  expect(shown).not.toContain("Pi settings");
  card.handleInput(DOWN); card.handleInput(ENTER);
  expect(pi.changes).toEqual([["apply", "save and go back"]]);
  card.handleInput(ESC);
  expect(pi.cancels()).toBe(1);
});

test("at 44 columns and 12 rows every row fits and a long description and clipped value are readable in full", () => {
  const description = `HEADMARK ${"explains the setting in many words ".repeat(6)}TAILMARK`;
  const pi = settings([
    { id: "a", label: "Fullscreen copy on select with a long label", description, currentValue: "a-very-long-value-name", values: ["a-very-long-value-name", "b"] },
    { id: "b", label: "Quiet startup", currentValue: "header", values: ["true", "header", "false"] },
  ]);
  const card = settingsCard(pi.request, host(12));
  const seen: string[] = [];
  for (let page = 0; page < 10; page++) {
    const lines = card.render(44);
    for (const line of lines) expect(visibleWidth(line)).toBe(44);
    seen.push(...plain(lines));
    card.handleInput(PAGE_DOWN);
  }
  const text = seen.join(" ");
  for (const mark of ["HEADMARK", "TAILMARK", "a-very-long-value-name"]) expect(text).toContain(mark);
  expect(pi.changes).toEqual([]);
});
