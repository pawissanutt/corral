import { test, expect } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { LifecycleView } from "./lifecycle-view.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { BridgeEvent, EventPage } from "../bridge/events.ts";

const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
const events = (from: number, count: number, payload: Record<string, unknown> = {}): BridgeEvent[] =>
  Array.from({ length: count }, (_, i) => ({ ...payload, seq: from + i, at: "2026-01-01T00:00:00Z", kind: "bound" }));
const snapshot = (list: BridgeEvent[], page: Partial<EventPage> = {}) => ({ session: "session-one", boundSession: "session-one",
  events: { events: list, last: list.at(-1)?.seq ?? 0, next: list.at(-1)?.seq ?? 0, more: false, ...page } }) as InspectorSnapshot;
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));
/** The list row carrying the selection bar. */
const selectedRow = (lines: readonly string[]) => plain(lines).find(line => line.includes("▌")) ?? "";
const GAP = { after: 0, resumed: 11, missing: 10, reconcile: true as const };

test("the newest host events list in append order, the selection follows its event as the window slides, the payload reads as fields", () => {
  let host = snapshot(events(11, 100, { gate: { exit: 1 } }), { gap: GAP });
  const view = new LifecycleView(identity, () => true, () => {}, () => true, () => host);
  const wide = view.render(140), text = plain(wide).join("\n");
  expect(plain(wide)[0]?.startsWith("‹ Task · Events")).toBe(true);
  expect(view.back.to).toBe(visibleWidth("‹ Task"));
  expect(text).toContain("last #110");
  expect(text).toContain("gap · 10 missing, resumed #11");
  expect(text).not.toMatch(/\bg\d|generation/);
  expect(selectedRow(wide)).toContain("31");
  expect(text.indexOf(" 31 ") < text.indexOf(" 32 ")).toBe(true);
  view.handleInput("j");
  host = snapshot(events(12, 100, { gate: { exit: 1 } }));
  const narrow = view.render(52), shown = plain(narrow).join("\n");
  expect(selectedRow(narrow)).toContain("32");
  expect(shown).toContain("session-one / #32");
  expect(shown).toContain("gate.exit");
  expect(shown).not.toContain('{"');
  expect(narrow.every(row => visibleWidth(row) <= 52)).toBe(true);
  host = snapshot(events(40, 100));
  const evicted = view.render(140);
  expect(selectedRow(evicted)).toBe("");
  expect(plain(evicted).join("\n")).toContain("unavailable / evicted");
});

test("a click selects only inside the list's columns, never through the detail pane beside it", () => {
  const view = new LifecycleView(identity, () => true, () => {}, () => true, () => snapshot(events(1, 3)));
  const y = plain(view.render(140)).findIndex(line => /^\s+2\s/.test(line));
  const click = { type: "click" as const, button: "left" as const, x: 100, y, screenX: 100, screenY: y, width: 140, height: 40, shift: false, alt: false, ctrl: false };
  expect(y).toBeGreaterThan(0);
  expect(view.handleMouse(click)).toBeUndefined();
  expect(selectedRow(view.render(140))).toMatch(/^\S\s+1\s/);
  expect(view.handleMouse({ ...click, x: 4, screenX: 4 })?.handled).toBe(true);
  expect(selectedRow(view.render(140))).toMatch(/^\S\s+2\s/);
});

test("the list fills the rows it is given and keeps the selection in view", () => {
  const view = new LifecycleView(identity, () => true, () => {}, () => true, () => snapshot(events(101, 80)), { rows: () => 14 });
  for (const width of [138, 98]) {
    for (let i = 0; i < 79; i++) view.handleInput("j");
    const lines = view.render(width);
    expect(selectedRow(lines)).toContain("180");
    expect(plain(lines).filter(line => /^\S?\s+\d{3}\s/.test(line)).length).toBeLessThanOrEqual(13);
    for (let i = 0; i < 79; i++) view.handleInput("k");
    expect(selectedRow(view.render(width))).toContain("101");
  }
});

test("every caveat stays behind the legend; an unbound host is flagged and a host without a log says so", () => {
  let legend = false, bound = true, host: InspectorSnapshot | undefined = snapshot(events(11, 100), { gap: GAP });
  const view = new LifecycleView(identity, () => bound, () => {}, () => true, () => host, { legend: () => legend });
  expect(plain(view.render(140)).join(" ")).not.toContain("No queue, move or dispatch controls");
  legend = true; bound = false;
  const shown = plain(view.render(140)).join(" ");
  for (const caveat of ["No queue, move or dispatch controls", "never timestamp-sorted", "GAP: 10 host events missing; resumed #11",
    "this host process only", "host closed", "these observations may be stale"]) expect(shown).toContain(caveat);
  host = undefined;
  expect(plain(view.render(140)).join(" ")).toContain("Does not apply: this host does not expose its session event log.");
  expect(selectedRow(view.render(140))).toBe("");
});
