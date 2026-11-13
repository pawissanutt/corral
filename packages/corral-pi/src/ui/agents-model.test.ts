import { expect, test } from "bun:test";
import { agentsModel, helperRows, liveCount, money, sparkline } from "./agents-model.ts";
import type { PanelJob, PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";

test("the sparkline keeps the last buckets right-aligned, scaled to the busiest one with a floor of four calls", () => {
  expect(sparkline([1], 14)).toBe(" ".repeat(13) + "▂");
  expect(sparkline([0, 1, 2, 4], 6)).toBe("   ▂▄▇");
  expect(sparkline([7, 0, 14], 3)).toBe("▄ ▇");
  const busy = Array.from({ length: 40 }, (_, index) => (index % 5) + 1);
  const line = sparkline(busy, 14);
  expect(line).toHaveLength(14);
  expect(line.includes(" ")).toBeFalse();
  expect(line.includes("▇")).toBeTrue();
});

const NOW = 1_000_000;
function helper(request: string, phase: HelperInspection["phase"], extra: Partial<HelperInspection> = {}): HelperInspection {
  return { request, kind: "explore", model: "provider/model-a", generation: 1, phase, createdAt: NOW - 9000,
    ...(phase === "preparing" || phase === "queued" ? {} : { startedAt: NOW - 8000 }),
    ...(phase === "preparing" || phase === "queued" || phase === "running" ? {} : { endedAt: NOW - 1000 }),
    prompt: `Find ${request}`, messages: [], truncated: false, ...extra };
}
function job(id: string, state: "running" | "exited", exitCode: number | null = null): PanelJob {
  return { job: id, state, detail: { job: id, command: `cmd ${id}`, state, startedAt: NOW - 5000, endedAt: state === "exited" ? NOW - 1000 : undefined,
    exitCode, signal: null, groupClear: true, bytes: 1, truncated: false, tail: "", monitorAttempts: 0, watches: [] } };
}
const base: PanelSnapshot = { bound: true, model: "m", effort: "off", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null, jobs: [], helpers: [] };

test("helper rows leave cost, context and files unknown instead of reporting zero", () => {
  const snapshot: PanelSnapshot = { ...base, helpers: [helper("a", "running"), helper("b", "completed", { result: "done" })],
    agents: [{ id: "a", session: "s-a", kind: "explore", state: "running", model: "model-a", transcript: true, tokens: 0 }] };
  for (const row of helperRows(snapshot, NOW)) {
    expect(row.usage?.cost).toBeUndefined();
    expect(row.usage?.contextPercent).toBeUndefined();
    expect(row.files).toBeUndefined();
    expect(money(row.usage?.cost)).toBe("–");
  }
  expect(money(0)).toBe("$0.00");
});

test("helper states read as a glyph and a word: running, done, failed, uncertain and queued", () => {
  const snapshot: PanelSnapshot = { ...base, helpers: [
    helper("run", "running"), helper("ok", "completed", { result: "Found it\nmore" }), helper("bad", "completed", { error: "provider refused\ntrace" }),
    helper("lost", "uncertain"), helper("wait", "queued"),
  ] };
  const rows = Object.fromEntries(helperRows(snapshot, NOW).map(row => [row.id, row]));
  const status = (id: string) => { const row = rows[id]; return row ? [row.state, row.status.glyph, row.status.tone, row.status.word] : []; };
  expect(status("run")).toEqual(["running", "●", "accent", "running"]);
  expect(status("ok")).toEqual(["done", "✓", "success", "done"]);
  expect(status("bad")).toEqual(["failed", "✗", "error", "failed"]);
  expect(status("lost")).toEqual(["uncertain", "!", "warning", "uncertain"]);
  expect(status("wait")).toEqual(["queued", "○", "muted", "queued"]);
  expect(rows["ok"]?.activity).toBe("answered · Found it");
  expect(rows["bad"]?.activity).toBe("provider refused");
  expect(rows["wait"]?.activity).toBe("waiting for a helper slot");
  expect(rows["lost"]?.activity).toBe("interrupted · outcome uncertain");
});

test("transcript is offered only for an eligible observation on a bound host", () => {
  const observed = (id: string, transcript: boolean) => ({ id, session: `s-${id}`, kind: "explore", state: "running", model: null, transcript });
  const snapshot: PanelSnapshot = { ...base, helpers: [helper("yes", "running"), helper("no", "running"), helper("unseen", "running")],
    agents: [observed("yes", true), observed("no", false)] };
  const offered = (value: PanelSnapshot) => Object.fromEntries(helperRows(value, NOW).map(row => [row.id, row.capabilities.transcript]));
  expect(offered(snapshot)).toEqual({ yes: true, no: false, unseen: false });
  expect(offered({ ...snapshot, bound: false })).toEqual({ yes: false, no: false, unseen: false });
  for (const row of helperRows(snapshot, NOW)) {
    expect(row.capabilities.open).toBe(true);
    expect(row.capabilities.steer).toBeUndefined(); expect(row.capabilities.cancel).toBeUndefined(); expect(row.capabilities.model).toBeUndefined();
  }
});

test("an unbound snapshot adds the stale note to every helper row", () => {
  const snapshot: PanelSnapshot = { ...base, helpers: [helper("a", "running"), helper("b", "completed")] };
  const stale = (value: PanelSnapshot) => helperRows(value, NOW).map(row => row.notes.some(note => note.tone === "warning" && /stale/.test(note.text)));
  expect(stale({ ...snapshot, bound: false })).toEqual([true, true]);
  expect(stale(snapshot)).toEqual([false, false]);
});

test("unavailable sources are marked unavailable, never turned into empty lists", () => {
  const unknown = agentsModel({ ...base, jobs: null, helpers: null }, NOW);
  expect(unknown.sources).toEqual({ helpers: false, jobs: false });
  const empty = agentsModel(base, NOW);
  expect(empty.sources).toEqual({ helpers: true, jobs: true });
  expect(agentsModel(null, NOW).sources).toEqual({ helpers: false, jobs: false });
});

test("liveCount counts preparing, queued and running helpers plus running jobs", () => {
  const model = agentsModel({ ...base,
    helpers: [helper("p", "preparing"), helper("q", "queued"), helper("r", "running"), helper("c", "completed"), helper("u", "uncertain"), helper("n", "not_started")],
    jobs: [job("live", "running"), job("done", "exited", 0), job("fail", "exited", 2)] }, NOW);
  expect(liveCount(model)).toBe(4);
});

test("names of retained helpers of one kind stay distinct in creation order", () => {
  const snapshot: PanelSnapshot = { ...base, helpers: [helper("newest", "running", { createdAt: NOW - 100 }), helper("oldest", "completed", { createdAt: NOW - 9000 }),
    helper("other", "running", { kind: "oracle", createdAt: NOW - 500 })] };
  expect(Object.fromEntries(helperRows(snapshot, NOW).map(row => [row.id, row.name]))).toEqual({ oldest: "explore", newest: "explore 2", other: "oracle" });
});
