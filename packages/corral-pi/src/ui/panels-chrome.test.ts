import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { statusBar } from "./panels-chrome.ts";
import type { PanelSnapshot } from "./panels-view.ts";

const worker: HostIdentity = { store: "s", project: "demo", role: "worker", task: "T-1", run: "R-1", generation: 3 };
const snapshot = (bound: boolean) => ({ bound, model: "openai/gpt-6.1-sol", effort: "high", cost: 0.5,
  context: { tokens: 48000, contextWindow: 200000, percent: 24 } }) as unknown as PanelSnapshot;
const bar = (identity: HostIdentity, bound: boolean, width: number, folded?: { model: string; effort: string }) =>
  statusBar(identity, snapshot(bound), new Map(), width, false, folded);

test("a build session has no binding segment, a bound role reads bound, and the role keeps three letters when tiny", () => {
  for (const width of [140, 105, 92, 61, 46]) {
    const build = bar({ ...worker, role: "build" }, false, width).map(stripTerminalSequences).join("\n");
    expect(build).not.toContain("unbound"); expect(build).not.toContain("○");
    const bound = bar(worker, true, width).map(stripTerminalSequences).join("\n");
    expect(bound).toContain("● bound");
    if (width >= 105) expect(bound).toContain("gen 3"); else expect(bound).not.toContain("gen");
    expect(bound).toContain(width < 64 ? " WRK " : " WORKER ");
  }
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
