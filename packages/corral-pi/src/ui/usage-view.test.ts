import { expect, test } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { usageLines, budgetLines } from "./usage-view.ts";
import { InspectionStore, type Inspection } from "./inspection.ts";
import { NativeWorkbench } from "./workbench.ts";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
function source(): Inspection {
  return { version: 1, task: identity.task, run: identity.run, root: "/repo", base: "a".repeat(40), head: "b".repeat(40), dirty: false,
    manifest_digest: null, audited: false, manifest_failures: [], criteria: [], gate: null, review: null, findings: [], findings_omitted: 0,
    artifacts: [], artifacts_omitted: 0, worker_summary: null, task_status: "running", usage: { source: "stored collector snapshot", models_omitted: 0, rows: [
      { scope: "Metered transcript", usage: { input: 10, output: 2, cache_read: 3, cache_write: 4, reported_usd: 0.1, estimated_usd: 0.2, unpriced_tokens: 0, cost_basis: "reported" }, wall_seconds: 12, known_runs: 1, runs: 1 },
      { scope: "Subscription API-equivalent", usage: { input: 20, output: 3, cache_read: 0, cache_write: 0, reported_usd: null, estimated_usd: 0.3, unpriced_tokens: 0, cost_basis: "estimated" }, wall_seconds: null, known_runs: 1, runs: 1 },
      { scope: "Unknown price", usage: { input: 50, output: 0, cache_read: 0, cache_write: 0, reported_usd: null, estimated_usd: null, unpriced_tokens: 50, cost_basis: null }, wall_seconds: null, known_runs: 1, runs: 2 },
      { scope: "Legacy missing", usage: null, wall_seconds: null, known_runs: 0, runs: 1 },
    ] } };
}
const budget = { limits: { max_tokens: 1000, max_usd_nanos: null }, tokens: 600, usd_nanos: 9000000,
  uncertain_requests: 1, pricing_gaps: 1, unbounded_requests: 1, legacy_usage_gap: true, exhausted: false, stopped: false };
const text = (store: InspectionStore, width = 120, chart = false) => usageLines(store, budget, true, width, chart).map(row => row.text).join("\n");
test("metered reported amounts never become verified bills; subscription estimates and missing price remain separate", async () => {
  const store = new InspectionStore(identity, async () => source(), () => {}); await store.refresh();
  for (const width of [40, 120]) {
    const output = text(store, width);
    expect(output).toContain("$0.100000"); expect(output).toContain("$0.200000"); expect(output).toContain("$0.300000");
    expect(output).toContain("not verified billed metered spend"); expect(output).toContain("not subscription charges");
    expect(output).toContain("Coverage 1/2"); expect(output).toContain("Usage unknown"); expect(output).toContain("unknown");
    expect(output).toContain("UNCERTAIN spend"); expect(output).toContain("Unresolved reservations: 1");
  }
  const bars = text(store, 40, true);
  expect(bars).toContain("Token bars"); expect(bars).toContain("Input 10"); expect(bars).toContain("input: █");
  expect(text(store)).not.toContain("Token bars");
  store.dispose();
});
test("unavailable/legacy budgets are never unlimited or zero; zero configured limits are real", () => {
  for (const value of [undefined, {}, { unavailable: "legacy" }, { ...budget, pricing_gaps: -1 }]) {
    const output = budgetLines(value).join("\n"); expect(output).toContain("unavailable / legacy"); expect(output).not.toContain("not configured");
  }
  expect(budgetLines({ ...budget, limits: { max_tokens: 0, max_usd_nanos: 0 } }).join("\n")).toContain("Token limit: 0 · USD limit: $0.000000");
});
test("missing and failed accounting reads retain explicit stale state, and render has no I/O", async () => {
  let reads = 0, fail = false;
  const store = new InspectionStore(identity, async () => { reads++; if (fail) throw new Error("offline"); return source(); }, () => {});
  expect(text(store)).toContain("unavailable / legacy"); await store.refresh();
  const before = reads; text(store); text(store, 40, true); expect(reads).toBe(before);
  fail = true; await store.refresh(true); expect(text(store)).toContain("STALE / unavailable: offline"); expect(text(store)).toContain("Metered transcript");
  store.dispose();
});
test("Usage main view is read-only, chart toggle keeps tables, and compact layout remains bounded", async () => {
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => null, changed() {}, canInteract: () => true, inspection: async () => source() });
  try {
    const viewport = createChatViewport({ document: new Text("actual transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("RETAINED DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage"); await wb.inspection.refresh();
    const screen = (width: number) => renderLayoutFrame(viewport.root, width, 160, () => {}).lines;
    expect(screen(120).join("\n")).toContain("Measured usage");
    wb.handleInput("c"); expect(screen(120).join("\n")).toContain("Token bars");
    for (const line of screen(40)) expect(visibleWidth(line)).toBeLessThanOrEqual(40);
    expect(screen(40).join("\n")).toContain("cache read");
    expect(screen(40).join("\n")).toContain("RETAINED DRAFT");
    wb.handleInput("\x1b[20~"); expect(wb.view).toBe("conversation");
    expect(screen(40).join("\n")).toContain("actual transcript");
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
    const rows = usageLines(store, undefined, true, width, chart).flatMap(row => new Text(row.text, 0, 0).render(width)).map(stripTerminalSequences);
    for (const line of rows) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    writeFileSync(`${path}/usage-${name}.txt`, rows.join("\n") + "\n");
    if (chart) wb.handleInput("c");
    writeFileSync(`${path}/usage-${name}-viewport.txt`, stripTerminalSequences(renderLayoutFrame(component, width, 36, () => {}).lines.join("\n")) + "\n");
  }
  wb.dispose(); store.dispose();
});
