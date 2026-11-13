import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { TuiMouseEvent } from "@earendil-works/pi-tui";
import type { QueueControl, QueuedItem } from "../host/queue-control.ts";
import { QueueDock, queueCapacity } from "./queue-dock.ts";
import { keyCap } from "./frame.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT } from "./panels-view.ts";

const item = (key: string, text: string, editable = true, policy: QueuedItem["policy"] = "follow_up"): QueuedItem =>
  ({ key, text, policy, editable });
/** Key and pointer operations run detached; a fixed number of microtask turns lets them settle. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 20; turn++) await Promise.resolve();
}
/** A host call that waits until the test opens it. */
function gated() {
  const { promise, resolve } = Promise.withResolvers<void>();
  return { gate: promise, release: resolve };
}
const click = (x: number, y: number): TuiMouseEvent =>
  ({ type: "click", button: "left", x, y, screenX: x, screenY: y, width: 120, height: 6, shift: false, alt: false, ctrl: false });
const SHIFT_UP = "\x1b[1;2A";

/** A host whose control really withdraws, removes and reorders, recording every call. */
function harness(initial: readonly QueuedItem[], options: { readonly control?: boolean; readonly enabled?: boolean;
  readonly gate?: Promise<void>; readonly refuse?: boolean; readonly vanish?: boolean } = {}) {
  const state = { items: [...initial], draft: "", calls: [] as string[], notices: [] as { text: string; tone: string }[], composer: 0 };
  const done = async <T>(value: T): Promise<T> => { await options.gate; return value; };
  const control: QueueControl = {
    enabled: () => options.enabled !== false,
    edit: async key => {
      state.calls.push(`edit ${key}`);
      await options.gate;
      const found = state.items.find(entry => entry.key === key);
      if (!found) return undefined;
      state.items = state.items.filter(entry => entry !== found);
      return found.text;
    },
    remove: async key => {
      state.calls.push(`remove ${key}`);
      if (options.refuse) return done(false);
      state.items = state.items.filter(entry => entry.key !== key);
      return done(true);
    },
    move: async (key, delta) => {
      state.calls.push(`move ${key} ${delta}`);
      if (options.refuse) return done(false);
      // The item was delivered while the move was on its way.
      if (options.vanish) { state.items = state.items.filter(entry => entry.key !== key); return done(false); }
      const editable = state.items.filter(entry => entry.editable), at = editable.findIndex(entry => entry.key === key);
      const other = editable[at + delta];
      if (at < 0 || !other) return done(false);
      const a = state.items.indexOf(editable[at]!), b = state.items.indexOf(other);
      [state.items[a], state.items[b]] = [state.items[b]!, state.items[a]!];
      return done(true);
    },
  };
  const dock = new QueueDock({
    items: () => state.items,
    control: () => options.control === false ? undefined : control,
    draft: () => state.draft,
    setDraft: text => { state.draft = text; },
    focusComposer: () => { state.composer++; },
    notice: (text, tone) => { state.notices.push({ text, tone }); },
    changed: () => {},
  });
  return { dock, state };
}
const screen = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
/** The cell column of `glyph` on the row that shows `text`. */
function cell(dock: QueueDock, width: number, text: string, glyph: string): { x: number; y: number } {
  const rows = screen(dock.render(width)), y = rows.findIndex(row => row.includes(text));
  const at = rows[y]!.lastIndexOf(glyph);
  expect(at).toBeGreaterThanOrEqual(rows[y]!.indexOf(text));
  return { x: visibleWidth(rows[y]!.slice(0, at)), y };
}

test("edit puts the withdrawn text before the draft and keeps what was typed while it ran", async () => {
  const { gate, release } = gated();
  const { dock, state } = harness([item("a", "first queued"), item("b", "second queued")], { gate });
  state.draft = "half-written";
  dock.focusLast();
  expect(dock.handleKey("\r")).toBeTrue();
  state.draft = "half-written reply";
  release(); await settle();
  expect(state.calls).toEqual(["edit b"]);
  expect(state.draft).toBe("second queued\n\nhalf-written reply");
  expect(dock.focused).toBeFalse();
  expect(state.composer).toBeGreaterThan(0);

  const blank = harness([item("a", "only item")]);
  blank.state.draft = "  \n";
  expect(await blank.dock.editLast()).toBeTrue();
  expect(blank.state.draft).toBe("only item");
  expect(blank.dock.focused).toBeFalse();
});

test("move keeps every item, the selection follows the moved key, and a pending call blocks others", async () => {
  const { gate, release } = gated();
  const { dock, state } = harness([item("a", "alpha"), item("b", "bravo"), item("c", "charlie")], { gate });
  dock.focusLast();
  expect(dock.handleKey(SHIFT_UP)).toBeTrue();
  expect(dock.busy).toBeTrue();
  expect(screen(dock.render(100)).find(row => row.includes("charlie"))).toContain("…");
  dock.handleKey("x"); dock.handleKey("K");
  release(); await settle();
  expect(state.calls).toEqual(["move c -1"]);
  expect(state.items.map(entry => entry.key)).toEqual(["a", "c", "b"]);
  expect(dock.selected).toBe("c");
  const rows = screen(dock.render(100));
  for (const text of ["alpha", "bravo", "charlie"]) expect(rows.some(row => row.includes(text))).toBeTrue();
  expect(rows.find(row => row.includes("▌"))).toContain("charlie");
  expect(rows.findIndex(row => row.includes("charlie"))).toBeLessThanOrEqual(rows.findIndex(row => row.includes("bravo")));
  dock.handleKey("J"); await settle();
  expect(state.items.map(entry => entry.key)).toEqual(["a", "b", "c"]);
  expect(dock.selected).toBe("c");
});

test("moves that the host always refuses are dim and make no call: across policies, past notices, at the ends", async () => {
  const items = [item("s1", "steer-one", true, "steer"), item("s2", "steer-two", true, "steer"), item("f1", "follow-one"),
    item("f2", "follow-two"), item("n", "notice", false), item("f3", "follow-three")];
  const keyed = harness(items);
  keyed.dock.focusLast();
  for (const [key, data] of [["s2", "J"], ["f1", "K"], ["f2", "J"], ["f3", "K"], ["s1", "K"], ["f3", "J"]] as const) {
    keyed.dock.selected = key;
    expect(keyed.dock.handleKey(data)).toBeTrue();
    await settle();
  }
  expect(keyed.state.calls).toEqual([]);
  expect(keyed.state.notices).toEqual([]);
  keyed.dock.selected = "s1"; keyed.dock.handleKey("J"); await settle();
  expect(keyed.state.calls).toEqual(["move s1 1"]);

  const clicked = harness(items);
  for (const [text, glyph] of [["steer-two", "↓"], ["follow-one", "↑"], ["follow-two", "↓"], ["follow-three", "↑"]] as const) {
    const { x, y } = cell(clicked.dock, 120, text, glyph);
    clicked.dock.handleMouse(click(x, y)); await settle();
  }
  expect(clicked.state.calls).toEqual([]);
  const { x, y } = cell(clicked.dock, 120, "steer-one", "↓");
  clicked.dock.handleMouse(click(x, y)); await settle();
  expect(clicked.state.calls).toEqual(["move s1 1"]);

  // The idle clause describes follow-ups only; a queue holding a steer item does not claim it.
  const header = (queue: readonly QueuedItem[]) => screen(harness(queue).dock.render(140))[0]!.trimEnd();
  const followUps = header([item("a", "alpha"), item("b", "bravo")]), mixed = header([item("a", "alpha", true, "steer"), item("b", "bravo")]);
  expect(followUps.length).toBeGreaterThan(mixed.length);
  expect(followUps.startsWith(mixed)).toBeTrue();
});

test("items that are not the owner's never render their text and never reach control", async () => {
  const items = [item("n1", "WAKE secret-payload", false, "steer"), item("a", "alpha"), item("n2", "secret-two", false)];
  const keyed = harness(items);
  for (const width of [30, 60, 120]) {
    const rows = screen(keyed.dock.render(width));
    expect(rows.join("\n")).not.toContain("secret");
    expect(rows.at(-1)).toMatch(/\b2\b/);
  }
  keyed.dock.focusLast();
  keyed.dock.selected = "n1";
  keyed.dock.handleKey("k"); keyed.dock.handleKey("j"); keyed.dock.handleKey("J");
  keyed.dock.handleKey("x"); await settle();
  expect(keyed.state.calls).toEqual(["remove a"]);

  const clicked = harness(items);
  for (const width of [40, 120]) {
    const height = clicked.dock.render(width).length;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) { clicked.dock.render(width); clicked.dock.handleMouse(click(x, y)); await settle(); }
    }
  }
  expect(clicked.state.calls.length).toBeGreaterThan(0);
  expect(clicked.state.calls.some(call => call.includes("n1") || call.includes("n2"))).toBeFalse();
});

test("a refused call leaves the rows as reported and raises a notice; disabled control offers nothing", async () => {
  const refused = harness([item("a", "alpha"), item("b", "bravo")], { refuse: true });
  refused.dock.focusLast(); refused.dock.handleKey("x"); await settle();
  expect(refused.state.calls).toEqual(["remove b"]);
  expect(refused.state.items).toHaveLength(2);
  const rows = screen(refused.dock.render(100));
  expect(rows.some(row => row.includes("bravo"))).toBeTrue();
  expect(refused.state.notices).toHaveLength(1);
  expect(refused.state.notices[0]!.tone).toBe("warning");

  // A refused move of an item that is still queued says it cannot move, not that it left.
  refused.dock.handleKey("K"); await settle();
  expect(refused.state.calls).toEqual(["remove b", "move b -1"]);
  expect(refused.state.items.map(entry => entry.key)).toEqual(["a", "b"]);
  expect(refused.state.notices).toHaveLength(2);
  expect(refused.state.notices[1]!.tone).toBe("warning");
  const vanished = harness([item("a", "alpha"), item("b", "bravo")], { vanish: true });
  vanished.dock.focusLast(); vanished.dock.handleKey("K"); await settle();
  expect(vanished.state.calls).toEqual(["move b -1"]);
  expect(vanished.state.notices).toHaveLength(1);
  expect(refused.state.notices[1]!.text).not.toBe(vanished.state.notices[0]!.text);
  expect(vanished.state.notices[0]!.text).toBe(refused.state.notices[0]!.text);
  // A refusal while control is enabled does not take the controls away.
  expect(refused.dock.controllable).toBeTrue();
  expect(rows.some(row => row.includes("×"))).toBeTrue();

  for (const options of [{ enabled: false }, { control: false }]) {
    const off = harness([item("a", "alpha")], options);
    expect(off.dock.controllable).toBeFalse();
    expect(screen(off.dock.render(100)).some(row => row.includes("×"))).toBeFalse();
    off.dock.focusLast(); off.dock.handleKey("x"); off.dock.handleKey("\r"); await settle();
    expect(await off.dock.editLast()).toBeFalse();
    expect(off.state.calls).toEqual([]);
    expect(off.state.items).toHaveLength(1);
    expect(off.state.notices.length).toBeGreaterThan(0);
    expect(off.state.notices[0]!.text).not.toBe(refused.state.notices[0]!.text);
  }
});

test("control hit ranges trigger the same calls as the keys at 40 and 120 columns", async () => {
  const items = [item("a", "alpha"), item("b", "bravo"), item("c", "charlie")];
  const actions = [
    { keys: ["\r"], glyph: (width: number) => width < 84 ? "e" : "edit" },
    { keys: [SHIFT_UP], glyph: () => "↑" },
    { keys: ["J"], glyph: () => "↓" },
    { keys: ["\x1b[3~"], glyph: () => "×" },
  ];
  for (const width of [40, 120]) {
    for (const action of actions) {
      const byKey = harness(items), byClick = harness(items);
      byKey.dock.focusLast(); byKey.dock.handleKey("k");
      expect(byKey.dock.selected).toBe("b");
      for (const key of action.keys) byKey.dock.handleKey(key);
      const { x, y } = cell(byClick.dock, width, "bravo", action.glyph(width));
      expect(byClick.dock.handleMouse(click(x, y))?.handled).toBeTrue();
      await settle();
      expect(byKey.state.calls).toHaveLength(1);
      expect(byClick.state.calls).toEqual(byKey.state.calls);
    }
    const select = harness(items);
    const { x, y } = cell(select.dock, width, "alpha", "alpha");
    select.dock.handleMouse(click(x, y));
    expect(select.dock.focused).toBeTrue();
    expect(select.dock.selected).toBe("a");
    expect(select.state.calls).toEqual([]);
  }
});

test("every dock line is exactly the width from 30 to 140 columns and long queues stay within capacity", async () => {
  expect([queueCapacity(14), queueCapacity(24), queueCapacity(46)]).toEqual([2, 3, 5]);
  const { gate, release } = gated();
  const items = [item("a", "界".repeat(80), true, "steer"), item("b", "x".repeat(300)), item("c", "tab\there\x1b[2Jnewline\nend"),
    item("d", "short"), item("n", "notice", false)];
  const { dock } = harness(items, { gate });
  dock.focusLast(); dock.handleKey("k");
  void dock.editLast();
  for (const rows of [14, 24, 46]) {
    dock.setCapacity(rows);
    for (let width = 30; width <= 140; width++) {
      const lines = dock.render(width);
      expect(lines.length).toBeLessThanOrEqual(1 + queueCapacity(rows) + 1);
      for (const line of lines) expect(visibleWidth(line)).toBe(width);
      expect(lines.join("")).not.toContain("\x1b[2J");
    }
  }
  release(); await settle();
});

test("each row names its receiver in accent, joined to the text below 84 columns; lines keep the width", () => {
  const items = [item("a", "alpha"), item("b", "bravo")];
  const host = (receiver?: () => string | undefined) => new QueueDock({
    items: () => items, control: () => undefined, draft: () => "", setDraft: () => {}, focusComposer: () => {},
    notice: () => {}, changed: () => {}, ...(receiver ? { receiver } : {}),
  });
  const named = host(() => "the parent"), plain = host(), unset = host(() => undefined);
  const wide = screen(named.render(120));
  expect(wide[0]).toContain("QUEUE · 2 waiting · each runs when its receiver is next idle");
  // `n  → receiver` in a 14-cell column, then the text.
  const row = wide.find(line => line.includes("alpha"))!;
  expect(row.indexOf("→ parent")).toBe(3);
  expect(row.indexOf("alpha")).toBe(3 + 14);
  expect(named.render(120).find(line => line.includes("alpha"))).toContain(PANEL_BLUE + "→ parent");
  expect(screen(named.render(40)).find(line => line.includes("alpha"))).toContain("→parent alpha");
  expect(screen(named.render(40))[0]!.trim()).toBe("QUEUE · 2 waiting");
  expect(screen(plain.render(120)).join("\n")).not.toContain("→");
  expect(screen(unset.render(120))).toEqual(screen(plain.render(120)));
  for (let width = 30; width <= 140; width++) {
    for (const line of named.render(width)) expect(visibleWidth(line)).toBe(width);
  }
});

test("controls are a key cap and words from 84 columns, glyphs below; impossible moves are dim, remove in error", () => {
  const { dock } = harness([item("a", "alpha"), item("b", "bravo")]);
  const wide = screen(dock.render(120)), first = wide.find(line => line.includes("alpha"))!;
  expect(first.trimEnd().endsWith(" e  edit  ↑  ↓  × remove")).toBeTrue();
  expect(first.length).toBe(120);
  const styled = dock.render(120).find(line => line.includes("alpha"))!;
  expect(styled).toContain(PANEL_MUTED + "↑");
  expect(styled).toContain(PANEL_TEXT + "↓");
  expect(styled).toContain(PANEL_ERROR + "× remove");
  expect(styled).toContain(keyCap("e", false, PANEL_BG));
  const tight = screen(dock.render(46)).find(line => line.includes("alpha"))!;
  expect(tight.trimEnd().endsWith(" e  ↑ ↓ ×")).toBeTrue();
});
