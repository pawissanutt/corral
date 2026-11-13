import { expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { usageBackAt, usageLines, budgetRows } from "./usage-view.ts";
import { InspectionStore, type Inspection } from "./inspection.ts";
import { NativeWorkbench } from "./workbench.ts";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
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
const budget = { limits: { max_tokens: 1000, max_usd_nanos: null }, tokens: 600, usd_nanos: 9000000,
  uncertain_requests: 1, pricing_gaps: 1, unbounded_requests: 1, legacy_usage_gap: true, exhausted: false, stopped: false };
const rows = (store: InspectionStore, width = 120, chart = false, legend = false) =>
  usageLines(store, budget, true, width, chart, legend).map(row => stripTerminalSequences(row.text));
const text = (store: InspectionStore, width = 120, chart = false, legend = false) => rows(store, width, chart, legend).join("\n");
test("metered reported amounts never become verified bills; subscription estimates and missing price remain separate", async () => {
  const store = new InspectionStore(identity, async () => source(), () => {}); await store.refresh();
  for (const width of [40, 120]) {
    const output = text(store, width);
    expect(output).toContain("$0.100000"); expect(output).toContain("$0.200000"); expect(output).toContain("$0.300000");
    // One caveat line stays on screen at every width; `?` shows the full sentences.
    expect(output).toMatch(/billed|verified bill/); expect(output).toContain("estimate");
    expect(output).not.toContain("not subscription charges");
    // The legend wraps at narrow widths; the sentence is present whole once its rows are joined.
    const legend = text(store, width, false, true).replace(/\s+/g, " ");
    expect(legend).toContain("not verified billed metered spend"); expect(legend).toContain("not subscription charges");
    expect(output).toContain("usage unknown"); expect(output).toContain("unknown");
    expect(output).toContain("UNCERTAIN spend");
    expect(output).toContain("12,345,678");
  }
  const unresolved = budgetRows(budget).rows.find(r => r.label === "Unresolved");
  expect(unresolved?.value).toContain("1 reservation"); expect(unresolved?.tone).toBe("warn");
  const bars = text(store, 40, true);
  expect(bars).toContain("token bars"); expect(bars).toMatch(/input +█/);
  expect(text(store)).not.toMatch(/input +█/);
  store.dispose();
});
test("unavailable/legacy budgets are never unlimited or zero; zero configured limits are real", () => {
  for (const value of [undefined, {}, { unavailable: "legacy" }, { ...budget, pricing_gaps: -1 }]) {
    const output = budgetRows(value).rows.map(r => r.value).join("\n"); expect(output).toContain("unavailable / legacy"); expect(output).not.toContain("not configured");
  }
  const zero = budgetRows({ ...budget, limits: { max_tokens: 0, max_usd_nanos: 0 } }).rows;
  expect(zero.find(r => r.label === "Token limit")?.value).toBe("0");
  expect(zero.find(r => r.label === "USD limit")?.value).toBe("$0.000000");
});
test("numeric columns end under their headers at every width; least important columns drop first, values kept", async () => {
  const store = new InspectionStore(identity, async () => source(), () => {}); await store.refresh();
  for (const width of [138, 98, 59, 44, 40]) {
    const lines = rows(store, width);
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const header = lines.find(line => /^ +input +output/.test(line)) ?? "";
    const edge = (head: string) => header.indexOf(head) + head.length;
    // The metered row is the first line after its section rule; its values end where their headers end.
    const metered = lines[lines.findIndex(line => line.startsWith("METERED TRANSCRIPT")) + 1] ?? "";
    expect(metered.slice(0, edge("input")).trimStart()).toBe("12,345,678");
    expect(metered.slice(edge("output") - 1, edge("output"))).toBe("2");
    expect(metered.slice(0, edge("reported USD")).endsWith("$0.100000")).toBe(true);
    const unknown = lines[lines.findIndex(line => line.startsWith("UNKNOWN PRICE")) + 1] ?? "";
    expect(unknown.slice(0, edge("reported USD")).endsWith("unknown")).toBe(true);
    if (width < 120) {
      expect(header).not.toContain("basis");
      expect(lines.join("\n")).toContain("basis reported");
    }
  }
  expect(rows(store, 138).find(line => /^ +input/.test(line))).toContain("basis");
  expect(rows(store, 40).find(line => /^ +input/.test(line))).not.toContain("cache read");
  expect(text(store, 40)).toContain("cache read 3");
  store.dispose();
});
test("the back line opens with ‹ Chat at column 0, and its hit range covers exactly that label", async () => {
  const store = new InspectionStore(identity, async () => source(), () => {}); await store.refresh();
  for (const width of [120, 44]) expect(rows(store, width)[0]?.startsWith("‹ Chat · Usage")).toBe(true);
  expect(usageBackAt(0, 0)).toBe(true); expect(usageBackAt("‹ Chat".length - 1, 0)).toBe(true);
  expect(usageBackAt("‹ Chat".length, 0)).toBe(false); expect(usageBackAt(0, 1)).toBe(false);
  store.dispose();
});
test("missing and failed accounting reads retain explicit stale state, and render has no I/O", async () => {
  let reads = 0, fail = false;
  const store = new InspectionStore(identity, async () => { reads++; if (fail) throw new Error("offline"); return source(); }, () => {});
  expect(text(store)).toContain("unavailable / legacy"); await store.refresh();
  const before = reads; text(store); text(store, 40, true); expect(reads).toBe(before);
  fail = true; await store.refresh(true); expect(text(store)).toContain("STALE / unavailable: offline"); expect(text(store)).toContain("METERED TRANSCRIPT");
  store.dispose();
});
test("Usage main view is read-only, chart toggle keeps tables, legend shows every caveat, and compact layout remains bounded", async () => {
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => source() });
  try {
    const viewport = createChatViewport({ document: new Text("actual transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("RETAINED DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage"); await wb.inspection.refresh();
    const screen = (width: number) => stripTerminalSequences(renderLayoutFrame(viewport.root, width, 160, () => {}).lines.join("\n"));
    expect(screen(120)).toContain("‹ Chat · Usage");
    wb.handleInput("c"); expect(screen(120)).toContain("token bars");
    wb.handleInput("?"); expect(screen(120)).toContain("not verified billed metered spend");
    for (const line of renderLayoutFrame(viewport.root, 40, 160, () => {}).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(40);
    expect(screen(40)).toContain("cache read");
    expect(screen(40)).toContain("RETAINED DRAFT");
    wb.handleInput("\x1b[20~"); expect(wb.view).toBe("conversation");
    expect(screen(40)).toContain("actual transcript");
  } finally { wb.dispose(); }
});
test("actual-input capture renders the canonical CLI projection in table and compact chart forms", async () => {
  const path = process.env.CORRAL_USAGE_CAPTURE;
  if (!path) return; // Opt-in offline replay: never fabricate an actual-input capture.
  const input = JSON.parse(readFileSync(path + "/inspection-source.json", "utf8"));
  const store = new InspectionStore({ ...identity, task: input.task, run: input.run }, async () => input, () => {});
  await store.refresh(); expect(store.error).toBeUndefined(); expect(store.current?.usage?.rows.length).toBeGreaterThan(0);
  const wb = new NativeWorkbench({ identity: { ...identity, task: input.task, run: input.run }, cwd: "/tmp", snapshot: () => null,
    changed() {}, canInteract: () => true, inspection: async () => input });
  const component = wb.mount(new Text("retained conversation", 0, 0)); wb.setView("usage"); await wb.inspection.refresh();
  for (const [name, width, chart] of [["table", 132, false], ["compact", 46, true]] as const) {
    const rows = usageLines(store, undefined, true, width, chart).map(row => stripTerminalSequences(row.text));
    for (const line of rows) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    writeFileSync(`${path}/usage-${name}.txt`, rows.join("\n") + "\n");
    if (chart) wb.handleInput("c");
    writeFileSync(`${path}/usage-${name}-viewport.txt`, stripTerminalSequences(renderLayoutFrame(component, width, 36, () => {}).lines.join("\n")) + "\n");
  }
  wb.dispose(); store.dispose();
});
