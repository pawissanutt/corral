import { expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { usageBackAt, usageLines, type UsageViewOptions } from "./usage-view.ts";
import { budgetState, usageModel, type UsageModel } from "./usage-model.ts";
import { InspectionStore, type Inspection } from "./inspection.ts";
import { clock } from "./panel-parts.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativeWorkbench } from "./workbench.ts";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
const pm = { store: "s", project: "demo", role: "pm" as const, generation: 1 };
const WIDTHS = [101, 78, 68, 44];
function source(): Inspection {
  return { version: 1, task: identity.task, run: identity.run, root: "/repo", base: "a".repeat(40), head: "b".repeat(40), dirty: false,
    manifest_digest: null, audited: false, manifest_failures: [], criteria: [], gate: null, review: null, findings: [], findings_omitted: 0,
    artifacts: [], artifacts_omitted: 0, worker_summary: null, task_status: "running", usage: { source: "stored collector snapshot", models_omitted: 0, rows: [
      { scope: "Metered transcript", usage: { input: 12_345_678, output: 2, cache_read: 3, cache_write: 4, reported_usd: 0.1, estimated_usd: 0.2, unpriced_tokens: 0, cost_basis: "reported" }, wall_seconds: 12, known_runs: 1, runs: 1 },
      { scope: "Subscription API-equivalent", usage: { input: 20, output: 3, cache_read: 0, cache_write: 0, reported_usd: null, estimated_usd: 0.3, unpriced_tokens: 0, cost_basis: "estimated" }, wall_seconds: null, known_runs: 1, runs: 1 },
      { scope: "Unknown price", usage: { input: 50, output: 0, cache_read: 0, cache_write: 0, reported_usd: null, estimated_usd: null, unpriced_tokens: 50, cost_basis: null }, wall_seconds: null, known_runs: 1, runs: 2 },
      { scope: "Legacy missing", usage: null, wall_seconds: null, known_runs: 0, runs: 1 },
    ] } };
}
const receipt = (extra: Record<string, unknown> = {}) => ({ limits: { max_tokens: null, max_usd_nanos: null }, tokens: 260_834, usd_nanos: 50_965_000,
  uncertain_requests: 0, pricing_gaps: 0, unbounded_requests: 0, legacy_usage_gap: false, exhausted: false, stopped: false, ...extra });
const pane = (extra: Partial<PanelSnapshot> = {}): PanelSnapshot => ({ bound: true, model: "anthropic/claude-sonnet-4", effort: "high", streaming: false,
  context: { tokens: 48_000, contextWindow: 200_000, percent: 24 }, tokens: 248_112, cost: 0.048201, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, jobs: null, auxiliary: null, steering: 0, followUps: 0, budget: receipt(), ...extra });
const options = (extra: Partial<UsageViewOptions> = {}): UsageViewOptions => ({ chart: false, legend: false, now: Date.now(), bodyRows: 0, ...extra });
const lines = (model: UsageModel, width: number, extra: Partial<UsageViewOptions> = {}) =>
  usageLines(model, width, options(extra)).map(row => stripTerminalSequences(row.text));
const worker = (store: InspectionStore, snapshot: PanelSnapshot | null = pane()) => usageModel({ identity, snapshot, inspection: store });
async function loaded(): Promise<InspectionStore> {
  const store = new InspectionStore(identity, async () => source(), () => {}); await store.refresh(); return store;
}
const bounded = (page: readonly string[], width: number) => { for (const line of page) expect(visibleWidth(line)).toBeLessThanOrEqual(width); };

test("a PM session shows its own usage and accounting; the task/run collector does not apply, is never read and is said once", async () => {
  let reads = 0;
  const wb = new NativeWorkbench({ identity: pm, cwd: "/tmp", snapshot: () => pane(), changed() {}, canInteract: () => true,
    inspection: async () => { reads++; return source(); } });
  try {
    wb.mount(new Text("transcript", 0, 0)); wb.setView("usage"); wb.handleInput("r"); wb.handleInput("c"); await Promise.resolve();
    expect(reads).toBe(0);
    expect(wb.hintState("usage", { items: 0, editable: 0, control: false }).usage).toEqual({ reread: false, chart: false });
    const model = usageModel({ identity: pm, snapshot: pane(), inspection: wb.inspection });
    for (const width of WIDTHS) {
      const page = lines(model, width), text = page.join("\n");
      bounded(page, width);
      for (const fact of ["248,112", "$0.048201", "24%", "anthropic/claude-sonnet-4", "260,834", "$0.050965"]) expect(text).toContain(fact);
      expect(text.match(/Does not apply/g)?.length).toBe(1);
      expect(text).not.toMatch(/stale|unavailable|running/i);
      // Accounted totals lead; diagnostics are subordinate and last.
      expect(text.indexOf("260,834") < text.indexOf("DIAGNOSTICS")).toBe(true);
      expect(text.indexOf("Does not apply") < text.indexOf("DIAGNOSTICS")).toBe(true);
    }
  } finally { wb.dispose(); }
});

test("budget: missing or unreadable is never zero or unlimited, legacy needs proof, zero limits are real, a stop is claimed only when recorded", async () => {
  const store = await loaded();
  const page = (budget: unknown, catalog?: number, width = 101) => lines({ ...worker(store), budget: budgetState(budget, catalog) }, width).join("\n");
  for (const [budget, catalog] of [[undefined, 6], [undefined, undefined], [{}, 7], [{ ...receipt(), tokens: -1 }, 7]] as const) {
    const text = page(budget, catalog);
    expect(text).toContain("unknown"); expect(text).not.toContain("not configured"); expect(text).not.toContain("no limit configured");
    expect(text).not.toContain("DIAGNOSTICS"); expect(text).not.toContain("260,834");
  }
  expect(page(undefined, 6)).toContain("predates"); expect(page(undefined, undefined)).not.toContain("predates"); expect(page({}, 7)).not.toContain("predates");
  const zero = page(receipt({ limits: { max_tokens: 0, max_usd_nanos: 0 }, tokens: 0, usd_nanos: 0 }));
  expect(zero).toContain("0 / 0"); expect(zero).toContain("$0.000000 / $0.000000"); expect(zero).not.toContain("not configured");
  for (const width of WIDTHS) {
    expect(page(receipt(), 7, width)).not.toMatch(/running|host stopped/);
    const unrecorded = page(receipt({ exhausted: true, stopped: undefined, limits: { max_tokens: 1000, max_usd_nanos: null }, tokens: 1000 }), 7, width);
    expect(unrecorded).toContain("1,000 / 1,000"); expect(unrecorded).toContain("not recorded"); expect(unrecorded).not.toMatch(/host stopped|not stopped|running/);
    expect(page(receipt({ exhausted: true, stopped: true, limits: { max_tokens: 1000, max_usd_nanos: null }, tokens: 1000 }), 7, width)).toContain("host stopped");
  }
  // The meter floors: 97.5% reads 97% and never draws full.
  const meter = page(receipt({ limits: { max_tokens: 2_000_000, max_usd_nanos: null }, tokens: 1_950_000 })).split("\n").find(line => line.includes("1,950,000 / 2,000,000")) ?? "";
  expect(meter).toContain("97%"); expect(meter.match(/▰/g)?.length).toBe(11);
  // Non-clean diagnostics are flagged under the budget and detailed in their own section after the collector.
  const issues = page(receipt({ uncertain_requests: 2, unbounded_requests: 1, pricing_gaps: 1, legacy_usage_gap: true }));
  expect(issues.indexOf("TASK T-001") < issues.indexOf("DIAGNOSTICS")).toBe(true);
  expect(issues.slice(issues.indexOf("DIAGNOSTICS"))).toMatch(/2 reservations[^\n]*1 unbounded/);
  // A local control plane's receipt (settled and reserved apart, no diagnostic counters) is read, not unreadable.
  const local = { admitted: true, exhausted: false, guard_paused: false, limits: { max_tokens: null, max_usd_nanos: null },
    used: { tokens: 1_200, usd_nanos: 3_000_000 }, reserved: { tokens: 800, usd_nanos: 1_000_000 }, stopped: false, active: 1 };
  for (const width of WIDTHS) {
    const text = page(local, undefined, width);
    expect(text).not.toContain("could not be read");
    for (const fact of ["2,000 tokens", "$0.004000", "1,200 tokens", "$0.003000", "800 tokens", "$0.001000"]) expect(text).toContain(fact);
    expect(text).not.toMatch(/\b0 (unresolved|reservations|pricing)|running|host stopped/);
  }
  for (const width of WIDTHS) expect(lines({ ...worker(store), budget: budgetState(local, undefined) }, width, { bodyRows: 6 })[2]).toContain("2,000 tokens");
  store.dispose();
});

test("session: an unknown member stays unknown, and a build total keeps its + in the table, the narrow rows and the digest", () => {
  const model: UsageModel = { identity: { role: "build", project: "demo", build: "slots" }, bound: false, budget: { kind: "unavailable" },
    collector: { state: { phase: "not_applicable", readAt: undefined, refreshing: false }, panel: undefined, legacy: false },
    session: { model: "anthropic/claude-sonnet-4", effort: "high", context: undefined, total: { cost: 0.4333, partial: true }, rows: [
      { key: "pane", name: "Pane", hue: undefined, tokens: 1_204_331, cost: 0.412, contextPercent: 24 },
      { key: "child:e1", name: "explore-e1", hue: 1, tokens: 88_120, cost: 0.0213, contextPercent: undefined },
      { key: "child:slots", name: "host-slots", hue: 2, tokens: 412_009, cost: undefined, contextPercent: 31 },
    ] } };
  for (const width of WIDTHS) {
    const page = lines(model, width, { bodyRows: 6 }), text = page.join("\n");
    bounded(page, width);
    expect(text).toContain("$0.433300+"); expect(text).toContain("1,704,460");
    const slots = page.find(line => line.includes("host-slots")) ?? "";
    expect(slots).toContain("unknown"); expect(slots).not.toContain("$0.0000");
    expect(page[1]).toContain("$0.433300+");
  }
  const single: UsageModel = { ...model, session: { ...model.session!, total: undefined, rows: [{ ...model.session!.rows[0]!, cost: undefined }] } };
  for (const width of WIDTHS) expect(lines(single, width).join("\n")).not.toContain("$0.000000");
});

test("collector table: numeric columns end under their headers at every width; least important drop first, values kept", async () => {
  const store = await loaded();
  for (const width of [138, 98, 59, 44, 40]) {
    const page = lines(worker(store), width);
    bounded(page, width);
    const header = page.find(line => /^ +input +output/.test(line)) ?? "";
    const edge = (head: string) => header.indexOf(head) + head.length;
    const metered = page[page.findIndex(line => line.startsWith("Metered transcript")) + 1] ?? "";
    expect(metered.slice(0, edge("input")).trimStart()).toBe("12,345,678");
    expect(metered.slice(edge("output") - 1, edge("output"))).toBe("2");
    expect(metered.slice(0, edge("reported USD")).endsWith("$0.100000")).toBe(true);
    const unknown = page[page.findIndex(line => line.startsWith("Unknown price")) + 1] ?? "";
    expect(unknown.slice(0, edge("reported USD")).endsWith("unknown")).toBe(true);
    const text = page.join("\n");
    for (const fact of ["$0.200000", "$0.300000", "usage unknown"]) expect(text).toContain(fact);
    if (width < 120) { expect(header).not.toContain("basis"); expect(text).toContain("basis reported"); }
  }
  expect(lines(worker(store), 138).find(line => /^ +input/.test(line))).toContain("basis");
  expect(lines(worker(store), 40).join("\n")).toContain("cache read 3");
  const bars = lines(worker(store), 40, { chart: true }).join("\n");
  expect(bars).toMatch(/input +█/); expect(bars).toContain("12,345,678");
  store.dispose();
});

test("the back line opens with ‹ Chat at column 0, and its hit range covers exactly that label", async () => {
  const store = await loaded();
  for (const width of [120, 44]) expect(lines(worker(store), width)[0]?.startsWith("‹ Chat · Usage")).toBe(true);
  expect(usageBackAt(0, 0)).toBe(true); expect(usageBackAt("‹ Chat".length - 1, 0)).toBe(true);
  expect(usageBackAt("‹ Chat".length, 0)).toBe(false); expect(usageBackAt(0, 1)).toBe(false);
  store.dispose();
});

test("a failed reread keeps the retained rows with their read time on screen; a failed first read infers nothing; render has no I/O", async () => {
  let reads = 0, fail = false;
  const store = new InspectionStore(identity, async () => { reads++; if (fail) throw new Error("database is locked"); return source(); }, () => {});
  await store.refresh();
  const before = reads;
  for (const width of WIDTHS) for (const bodyRows of [0, 6, 15]) lines(worker(store), width, { chart: true, legend: true, bodyRows });
  expect(reads).toBe(before);
  fail = true; await store.refresh(true);
  const read = clock(store.checkedAt ?? 0);
  for (const width of WIDTHS) {
    const page = lines(worker(store), width), text = page.join("\n");
    bounded(page, width);
    expect(text).toContain(read); expect(text).toContain("12,345,678");
  }
  expect(lines(worker(store), 101, { legend: true }).join("\n")).toContain("database is locked");
  const never = new InspectionStore(identity, async () => { throw new Error("corral inspect exited 1"); }, () => {});
  await never.refresh();
  const failed = lines(worker(never), 78).join("\n");
  expect(failed).toContain("corral inspect exited 1"); expect(failed).not.toMatch(/ +input +output/);
  store.dispose(); never.dispose();
});

test("short views open with a digest of all three questions; taller views start at the sections", async () => {
  const store = await loaded();
  for (const width of WIDTHS) {
    const short = lines(worker(store), width, { bodyRows: 6 });
    bounded(short, width);
    expect(short.findIndex(line => line.startsWith("SESSION"))).toBe(4);
    expect(short.slice(1, 4).join("\n")).toContain("248,112");
    expect(lines(worker(store), width, { bodyRows: 15 }).findIndex(line => line.startsWith("SESSION"))).toBe(1);
  }
  store.dispose();
});

test("Usage main view is read-only, r/c act only with a collector, legend appends the rules, and compact layout remains bounded", async () => {
  let reads = 0;
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => pane(), changed() {}, canInteract: () => true,
    inspection: async () => { reads++; return source(); } });
  try {
    const viewport = createChatViewport({ document: new Text("actual transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("RETAINED DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage"); await wb.inspection.refresh();
    expect(wb.hintState("usage", { items: 0, editable: 0, control: false }).usage).toEqual({ reread: true, chart: true });
    const screen = (width: number) => stripTerminalSequences(renderLayoutFrame(viewport.root, width, 160, () => {}).lines.join("\n"));
    expect(screen(120)).toContain("‹ Chat · Usage");
    const before = reads; wb.handleInput("r"); await wb.inspection.refresh(); expect(reads).toBeGreaterThan(before);
    wb.handleInput("c"); expect(screen(120)).toMatch(/input +█/);
    wb.handleInput("?"); expect(screen(120)).toContain("READING RULES");
    for (const line of renderLayoutFrame(viewport.root, 40, 160, () => {}).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(40);
    expect(screen(40)).toContain("cache read");
    expect(screen(40)).toContain("RETAINED DRAFT");
    wb.handleInput("\x1b[20~"); expect(wb.view).toBe("conversation");
    expect(screen(40)).toContain("actual transcript");
  } finally { wb.dispose(); }
});

test("paging a short Usage viewport down shows every row of the page, none skipped", () => {
  const wb = new NativeWorkbench({ identity: pm, cwd: "/tmp", snapshot: () => pane(), changed() {}, canInteract: () => true });
  try {
    const viewport = createChatViewport({ document: new Text("transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage"); wb.handleInput("?");
    const seen: string[] = [];
    for (let press = 0; press < 80; press++) {
      seen.push(stripTerminalSequences(renderLayoutFrame(viewport.root, 80, 9, () => {}).lines.join("\n")));
      wb.handleInput("\x1b[6~");
    }
    const shown = seen.join("\n");
    // The view is 78 cells (80 less the inset) and short, so the page opens with the digest; every row of it must have been on screen.
    const page = lines(usageModel({ identity: pm, snapshot: pane(), inspection: wb.inspection }), 78, { legend: true, bodyRows: 5 });
    expect(page.length).toBeGreaterThan(24);
    for (const row of page.map(line => line.trim()).filter(Boolean)) expect(shown).toContain(row);
    expect(shown).toContain("DRAFT");
  } finally { wb.dispose(); }
});

test("actual-input capture renders the canonical CLI projection in table and compact chart forms", async () => {
  const path = process.env.CORRAL_USAGE_CAPTURE;
  if (!path) return; // Opt-in offline replay: never fabricate an actual-input capture.
  const input = JSON.parse(readFileSync(path + "/inspection-source.json", "utf8"));
  const bound = { ...identity, task: input.task, run: input.run };
  const store = new InspectionStore(bound, async () => input, () => {});
  await store.refresh(); expect(store.error).toBeUndefined(); expect(store.current?.usage?.rows.length).toBeGreaterThan(0);
  const wb = new NativeWorkbench({ identity: bound, cwd: "/tmp", snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => input });
  const component = wb.mount(new Text("retained conversation", 0, 0)); wb.setView("usage"); await wb.inspection.refresh();
  for (const [name, width, chart] of [["table", 132, false], ["compact", 46, true]] as const) {
    const rows = lines(usageModel({ identity: bound, snapshot: null, inspection: store }), width, { chart });
    bounded(rows, width);
    writeFileSync(`${path}/usage-${name}.txt`, rows.join("\n") + "\n");
    if (chart) wb.handleInput("c");
    writeFileSync(`${path}/usage-${name}-viewport.txt`, stripTerminalSequences(renderLayoutFrame(component, width, 36, () => {}).lines.join("\n")) + "\n");
  }
  wb.dispose(); store.dispose();
});
