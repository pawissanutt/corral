import { test, expect } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { fitSections, navRows, renderNav, type NavSection, type NavSource } from "./nav-panel.ts";
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

const plan: NavSection = { title: "PLAN 3/6", items: [
  ...["a", "b", "c"].map(id => ({ text: `done ${id}`, state: "closed" as const })),
  { text: "now", state: "current" as const }, { text: "next 1" }, { text: "next 2" }] };
const running: NavSection = { title: "RUNNING 2", items: [{ text: "job 1" }, { text: "job 2" }] };
const texts = (list: readonly NavSection[]) => list.flatMap(section => section.items.map(item => stripTerminalSequences(item.text)));

test("sections shrink in order: finished steps fold, then windows trim around the current step, then later sections drop", () => {
  expect(texts(fitSections([plan, running], 20))).toContain("done a");
  const folded = texts(fitSections([plan, running], 11));
  expect(folded).toContain("✓ 3 done"); expect(folded).toContain("now"); expect(folded).not.toContain("done a");
  const trimmed = fitSections([plan, running], 7);
  expect(trimmed).toHaveLength(2);
  expect(texts(trimmed)).toContain("now");
  expect(fitSections([plan, running], 4).map(section => section.title)).toEqual(["PLAN 3/6"]);
});

test("the rail never scrolls and keeps every row width-bounded", () => {
  const rows = [{ view: "conversation" as const, key: "F9", label: "Chat", badge: "" },
    { view: "inbox" as const, key: "F11", label: "Inbox", badge: "⚑ 2", footer: true }];
  for (const height of [5, 9, 30]) {
    const frame = renderNav(rows, view => view === "conversation", 32, height, "GO TO", [plan, running]);
    expect(frame.lines).toHaveLength(height);
    for (const line of frame.lines) expect(visibleWidth(line)).toBe(32);
  }
  const tall = renderNav(rows, () => false, 32, 30, "GO TO", [plan, running]);
  const text = tall.lines.map(stripTerminalSequences).join("\n");
  expect(text).toContain("RUNNING 2"); expect(text).toContain("Inbox");
});
