import { test, expect } from "bun:test";
import { stripTerminalSequences } from "@earendil-works/pi-tui";
import { navRows, renderNav, type NavSource } from "./nav-panel.ts";
import type { ReviewStore } from "./review-store.ts";
import type { InspectionStore } from "./inspection.ts";
import type { DecisionInbox } from "./decision-inbox.ts";

const source = (stats: Map<string, { added: number | null; removed: number | null }>): NavSource => ({
  taskBound: false, proof: false, snapshot: null, view: "conversation",
  review: { stats, files: [...stats.keys()].map(path => ({ path, status: "??" })), checkedAt: 1, scopeRequired: false } as unknown as ReviewStore,
  inspection: {} as InspectionStore, inbox: {} as DecisionInbox,
});
const changes = (s: NavSource) => stripTerminalSequences(navRows(s).find(row => row.view === "changes")?.badge ?? "");

test("files without line counts are never shown as +0−0", () => {
  expect(changes(source(new Map([["new.ts", { added: null, removed: null }]])))).toBe("Δ1");
  expect(changes(source(new Map([["a.ts", { added: 3, removed: 1 }]])))).toBe("+3−1");
});

test("the panel never scrolls: sub-lines drop before rows when height is short", () => {
  const rows = [{ view: "task" as const, key: "F5", label: "Task", badge: "", sub: "step" },
    { view: "conversation" as const, key: "F9", label: "Chat", badge: "" },
    { view: "inbox" as const, key: "F11", label: "Inbox", badge: "⚑ 2", footer: true },
    { view: "usage" as const, key: "", label: "Usage", badge: "$0.42", footer: true }];
  const tall = renderNav(rows, view => view === "conversation", 32, 12).lines.map(stripTerminalSequences).join("\n");
  expect(tall).toContain("step"); expect(tall).toContain("Usage");
  const short = renderNav(rows, () => false, 32, 5);
  expect(short.lines).toHaveLength(5);
  const text = short.lines.map(stripTerminalSequences).join("\n");
  expect(text).not.toContain("step"); expect(text).toContain("More");
});
