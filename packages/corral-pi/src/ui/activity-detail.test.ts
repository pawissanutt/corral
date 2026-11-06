import { beforeAll, expect, test } from "bun:test";
import { getMarkdownTheme, initTheme } from "@earendil-works/pi-coding-agent";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { numberedTail, renderActivityView, TOOLS_FOLDED, type ActivityViewInput } from "./activity-detail.ts";
import { activityEntries } from "./activity-list.ts";
import { NativeWorkbench } from "./workbench.ts";
import type { PanelJob, PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";

beforeAll(() => initTheme("dark", false));
const NOW = 1_000_000;
const identity = { store: "s", role: "worker" as const, project: "p", task: "T-1", run: "r-3", generation: 1 };
function job(id: string, state: "running" | "exited", exitCode: number | null, tail = "", lines?: number): PanelJob {
  return { job: id, state, detail: { job: id, command: `cmd ${id}`, state, startedAt: NOW - 8000, endedAt: state === "exited" ? NOW - 1000 : undefined,
    exitCode, signal: null, groupClear: true, bytes: 10, truncated: false, tail, monitorAttempts: 0, watches: [], ...(lines === undefined ? {} : { lines }) } };
}
function helper(request: string, phase: HelperInspection["phase"], tools = 0): HelperInspection {
  return { request, kind: "explore", model: "p/m", generation: 1, phase, createdAt: NOW - 5000, startedAt: NOW - 4000, truncated: false, prompt: `prompt ${request}`,
    messages: [...Array.from({ length: tools }, (_, index) => ({ id: `t${index}`, kind: "tool" as const, tool: "read", phase: "completed" as const,
      arguments: JSON.stringify({ path: `f${index}.rs` }), text: "" })), { id: "a", kind: "assistant" as const, text: `answer ${request}` }] };
}
const tail = Array.from({ length: 30 }, (_, index) => `line ${index + 1}`).join("\n") + "\n";
const snapshot: PanelSnapshot = { bound: true, model: "m", effort: "off", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null,
  jobs: [job("j-1", "running", null, tail, 30), job("j-2", "exited", 0), job("j-3", "exited", 101)],
  helpers: [helper("h-1", "running", 6), helper("h-2", "completed"), helper("h-3", "uncertain")] };
const input = (overrides: Partial<ActivityViewInput>): ActivityViewInput => ({ snapshot, identity, selected: undefined, width: 140, height: 24, now: NOW,
  legend: false, expanded: false, freeze: undefined, markdownTheme: getMarkdownTheme(), ...overrides });
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));

test("list glyphs and tones distinguish running, passed, failed and uncertain work", () => {
  const glyphs = Object.fromEntries(activityEntries(snapshot, NOW).map(entry => [entry.key, [entry.glyph, entry.tone]]));
  expect(glyphs).toEqual({ "job:j-1": ["●", "accent"], "job:j-2": ["✓", "success"], "job:j-3": ["✗", "error"],
    "helper:h-1": ["●", "accent"], "helper:h-2": ["✓", "success"], "helper:h-3": ["!", "warning"] });
  const rows = plain(renderActivityView(input({ selected: { kind: "job", job: "j-1" } })).lines);
  expect(rows.find(row => row.includes("cmd j-1"))?.startsWith("▌●")).toBe(true);
  expect(rows.find(row => row.includes("cmd j-3"))).toContain("exit 101");
});

test("retained tail numbers end at the host line count, with the open line while running", () => {
  expect(numberedTail("a\nb\n", 2, true).map(line => line.number)).toEqual([1, 2, 3]);
  expect(numberedTail("a\nb\n", 2, false).map(line => line.number)).toEqual([1, 2]);
  expect(numberedTail("a\nb", 2, false).map(line => line.number)).toEqual([1, 2]);
  expect(numberedTail("x\x1b[31mred\x1b[0m\rdone\n", 9, false)).toEqual([{ number: 9, text: "done" }]);
  expect(numberedTail("a\n", undefined, true).every(line => line.number === undefined)).toBe(true);
  const gutter = plain(renderActivityView(input({ selected: { kind: "job", job: "j-1" } })).lines)
    .flatMap(row => /│ (\d+) │/.exec(row.slice(42))?.[1] ?? []).map(Number);
  expect(gutter.at(-1)).toBe(31);
  expect(gutter.every((value, index) => index === 0 || value === (gutter[index - 1] ?? 0) + 1)).toBe(true);
});

test("tool calls fold after the first few and expand to every call", () => {
  const toolRows = (expanded: boolean) => plain(renderActivityView(input({ selected: { kind: "helper", request: "h-1" }, expanded })).lines)
    .filter(row => row.includes("▎ ▤ read"));
  expect(toolRows(false)).toHaveLength(TOOLS_FOLDED);
  const more = plain(renderActivityView(input({ selected: { kind: "helper", request: "h-1" } })).lines).join("\n");
  expect(/▸ (\d+) more/.exec(more)?.[1]).toBe(String(6 - TOOLS_FOLDED));
  expect(toolRows(true)).toHaveLength(6);
});

test("below the split width the list collapses to a one-line picker and nothing overflows", () => {
  for (const width of [140, 105, 104, 80, 52]) {
    const lines = renderActivityView(input({ width, selected: { kind: "job", job: "j-2" } })).lines;
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const rows = plain(lines);
    expect(rows.some(row => row.includes("JOBS"))).toBe(width >= 105);
    expect(/‹ (\d+)\/(\d+) ›/.exec(rows[0] ?? "")?.slice(1)).toEqual(width >= 105 ? undefined : ["2", "6"]);
  }
});

test("workbench keys walk the shared list, pause follow and expand helper tool calls", () => {
  let current = snapshot;
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => current, changed() {}, canInteract: () => true });
  const viewport = createChatViewport({ document: new Text("", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
    editor: new Text("", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
  const screen = () => stripTerminalSequences(renderLayoutFrame(viewport.root, 140, 30, () => {}).lines.join("\n"));
  try {
    wb.openJob("j-1");
    expect(screen()).toContain("▌● cmd j-1");
    wb.handleInput("f");
    current = { ...snapshot, jobs: [job("j-1", "running", null, tail + "late\n", 31), ...(snapshot.jobs ?? []).slice(1)] };
    expect(screen()).toContain("‖"); expect(screen()).not.toContain("late");
    wb.handleInput("f"); expect(screen()).toContain("late");
    wb.handleInput("j"); expect(screen()).toContain("▌✓ cmd j-2");
    wb.openHelpers("h-1");
    const count = () => screen().split("\n").filter(row => row.includes("▎ ▤ read")).length;
    expect(count()).toBe(TOOLS_FOLDED);
    wb.handleInput("\r"); expect(count()).toBe(6);
  } finally { wb.dispose(); }
});

test("job control keys preserve the captured target through Stop confirmation and do not treat Background as Stop", async () => {
  const scoped: PanelSnapshot = { ...snapshot, inspector: { session: "session", boundSession: "session", turn: 1, turnOpen: false,
    capabilities: null, guidance: { projectText: false, skillPaths: [], promptPaths: [], prompts: [], skills: [] },
    settings: [], budget: [], events: { events: [], next: 0, omitted: 0 } as never, receipts: [], receiptsOmitted: 0 } };
  const calls: { job: string; action: string }[] = [];
  let confirmations = 0;
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => scoped, changed() {}, canInteract: () => true,
    jobControl: async (target, action) => { calls.push({ job: target.job, action }); return { state: "running", text: "current output" }; } });
  wb.bind({ notify() {}, custom: async () => undefined, confirm: async () => {
    confirmations++; wb.openJob("j-2"); return true;
  } } as never);
  try {
    wb.openJob("j-1");
    wb.handleInput("b"); await Promise.resolve();
    wb.handleInput("p"); await Promise.resolve();
    wb.handleInput("x"); await Promise.resolve(); await Promise.resolve();
    expect(confirmations).toBe(1);
    expect(calls).toEqual([{ job: "j-1", action: "background" }, { job: "j-1", action: "peek" }, { job: "j-1", action: "stop" }]);
    expect(wb.selectedActivity).toEqual({ kind: "job", job: "j-2" });
  } finally { wb.dispose(); }
});

test("unavailable job/helper sources are never shown as empty at narrow or wide widths", () => {
  const unknown: PanelSnapshot = { ...snapshot, jobs: null, helpers: null };
  for (const width of [140, 80, 52]) {
    const screen = plain(renderActivityView(input({ snapshot: unknown, width })).lines).join("\n");
    expect(screen).toContain("unavailable");
    expect(screen).not.toContain("No jobs or helpers yet");
    expect(screen).not.toContain("No background jobs or helper calls yet");
  }
  const empty = plain(renderActivityView(input({ snapshot: { ...snapshot, jobs: [], helpers: [] }, width: 80 })).lines).join("\n");
  expect(empty).toContain("No jobs or helpers yet"); expect(empty).not.toContain("unavailable");
});
