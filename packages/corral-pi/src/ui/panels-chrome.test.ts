import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { statusBar } from "./panels-chrome.ts";
import type { PanelSnapshot } from "./panels-view.ts";

const worker: HostIdentity = { store: "s", project: "demo", role: "worker", task: "T-1", run: "R-1" };
const snapshot = (bound: boolean) => ({ bound, model: "openai/gpt-6.1-sol", effort: "high", cost: 0.5,
  context: { tokens: 48000, contextWindow: 200000, percent: 24 } }) as unknown as PanelSnapshot;
const bar = (identity: HostIdentity, bound: boolean, width: number, folded?: { model: string; effort: string }) =>
  statusBar(identity, snapshot(bound), new Map(), width, false, folded);

test("no role draws a binding segment, and the role keeps three letters when tiny", () => {
  for (const width of [140, 105, 92, 61, 46]) {
    for (const identity of [worker, { ...worker, role: "build" as const }]) for (const bound of [true, false]) {
      const text = bar(identity, bound, width).map(stripTerminalSequences).join("\n");
      expect(text).not.toMatch(/bound|[●○]/);
    }
    expect(bar(worker, true, width).map(stripTerminalSequences).join("\n")).toContain(width < 64 ? " WRK " : " WORKER ");
  }
});

test("a long child identity does not clip the host's alerts when the meter moves to its own row", () => {
  const rows = statusBar(worker, snapshot(true), new Map([["refusal", "Parent input was refused: check the host's reason before trying again."]]),
    64, false, undefined, { id: "a".repeat(32) });
  const text = rows.map(stripTerminalSequences).join("\n");
  expect(text).toContain("Context unavailable");
  expect(text.replace(/\s+/g, " ")).toContain("Parent input was refused: check the host's reason before trying again.");
  expect(rows.every(row => visibleWidth(row) === 64)).toBeTrue();
});

test("while the composer bar is folded the status bar names the model and effort, and holds its width at 46 columns", () => {
  const folded = { model: "openai/gpt-6.1-sol", effort: "high" };
  expect(stripTerminalSequences(bar(worker, true, 140, folded)[0]!)).toContain("gpt-6.1-sol · effort high");
  for (const width of [140, 92, 64, 61, 46]) {
    const rows = bar(worker, true, width, folded);
    expect(rows).toHaveLength(1);
    for (const row of rows) expect(visibleWidth(row)).toBe(width);
    // The meter stays readable: it never shortens past `ctx` to make room for the model.
    expect(stripTerminalSequences(rows[0]!)).toContain("ctx ");
  }
});
