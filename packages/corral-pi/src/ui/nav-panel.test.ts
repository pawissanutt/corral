import { test, expect } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { fitSections, navRows, navSections, renderNav, type NavSection, type NavSource } from "./nav-panel.ts";
import { PANEL_BG, PANEL_SELECTED, RESET, type PanelSnapshot } from "./panels-view.ts";
import type { ReviewStore } from "./review-store.ts";
import type { InspectionStore } from "./inspection.ts";
import type { DecisionInbox } from "./decision-inbox.ts";

const source = (stats: Map<string, { added: number | null; removed: number | null }>): NavSource => ({
  taskBound: false, proof: false, snapshot: null, view: "conversation",
  review: { stats, files: [...stats.keys()].map(path => ({ path, status: "??" })), checkedAt: 1, scopeRequired: false } as unknown as ReviewStore,
  inspection: {} as InspectionStore, inbox: {} as DecisionInbox,
});
const changes = (s: NavSource) => stripTerminalSequences(navRows(s).find(row => row.view === "changes")?.badge ?? "");

test("files without line counts are never shown as +0−0, and there is no Usage row", () => {
  expect(changes(source(new Map([["new.ts", { added: null, removed: null }]])))).toBe("Δ1");
  expect(changes(source(new Map([["a.ts", { added: 3, removed: 1 }]])))).toBe("+3−1");
  expect(navRows(source(new Map())).some(row => row.view === "usage")).toBeFalse();
});

test("finished plan steps fold into one line, and waits show ? for the owner and … for an event", () => {
  const step = (id: string, state: string) => ({ id, label: id, status: { state }, criteria: [] });
  const snapshot = { ledgerStale: false, jobs: [], helpers: [], ledger: { phases: [{ steps: [
    step("read", "done"), step("write", "done"), step("build", "in_progress"), step("ask", "pending"), step("land", "pending")] }],
  waits: [{ step: "ask", on: { kind: "owner" } }, { step: "land", on: { kind: "event" } }] } } as unknown as PanelSnapshot;
  const [plan] = navSections({ ...source(new Map()), snapshot });
  expect(plan?.title).toBe("Plan"); expect(plan?.count).toBe("2 of 5");
  expect(plan?.items.map(item => [stripTerminalSequences(item.text), stripTerminalSequences(item.right ?? "")]))
    .toEqual([["✓ 2 done", ""], ["› build", ""], ["  ask", "?"], ["  land", "…"]]);
});

const plan: NavSection = { title: "Plan", count: "3 of 6", items: [
  { text: "✓ 3 done", state: "closed" as const },
  { text: "now", state: "current" as const }, { text: "next 1" }, { text: "next 2" }] };
const running: NavSection = { title: "Running", count: "2", items: [{ text: "job 1" }, { text: "job 2" }] };
const texts = (list: readonly NavSection[]) => list.flatMap(section => section.items.map(item => stripTerminalSequences(item.text)));

test("sections shrink in order: windows trim around the current step, then later sections drop", () => {
  expect(texts(fitSections([plan, running], 20))).toEqual(["✓ 3 done", "now", "next 1", "next 2", "job 1", "job 2"]);
  const trimmed = fitSections([plan, running], 7);
  expect(trimmed).toHaveLength(2);
  expect(texts(trimmed)).toContain("now");
  expect(fitSections([plan, running], 4).map(section => section.title)).toEqual(["Plan"]);
});

test("the rail never scrolls and keeps every row width-bounded", () => {
  const rows = [{ view: "conversation" as const, key: "F9", label: "Chat", badge: "" },
    { view: "inbox" as const, key: "F11", label: "Inbox", badge: "⚑ 2", footer: true }];
  for (const height of [5, 9, 30]) {
    const frame = renderNav(rows, view => view === "conversation", 32, height, "Assistant · A-1", [plan, running]);
    expect(frame.lines).toHaveLength(height);
    for (const line of frame.lines) expect(visibleWidth(line)).toBe(32);
  }
  const tall = renderNav(rows, () => false, 32, 30, "Assistant · A-1", [plan, running]);
  const text = tall.lines.map(stripTerminalSequences).join("\n");
  expect(text).toContain("Assistant · A-1"); expect(text).toMatch(/Running\s+2/); expect(text).toContain("Inbox");
  expect(text).not.toContain("GO TO"); expect(text).not.toContain("Ctrl+B hide");
});

test("clipped rows restore the panel background after truncation's reset, so no black cells show", () => {
  const rows = [{ view: "conversation" as const, key: "F9", label: "Chat", badge: "" }];
  const long: NavSection = { title: "Plan", count: "0 of 1", items: [{ text: "x".repeat(80), state: "current" as const, right: "?" }] };
  const lines = renderNav(rows, view => view === "conversation", 32, 12, "Assistant · A-1", [long]).lines;
  const inner = lines.flatMap(line => line.slice(0, -RESET.length).split(RESET).slice(1));
  expect(inner.length).toBeGreaterThan(0);
  for (const rest of inner) expect(rest.startsWith(PANEL_BG) || rest.startsWith(PANEL_SELECTED)).toBeTrue();
});
