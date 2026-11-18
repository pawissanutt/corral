import { expect, test } from "bun:test";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { usageBackAt, usageLines, type UsageViewOptions } from "./usage-view.ts";
import { usageModel, type UsageModel } from "./usage-model.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativeWorkbench } from "./workbench.ts";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
const identity = { store: "s", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
const WIDTHS = [101, 78, 68, 44];
const pane = (extra: Partial<PanelSnapshot> = {}): PanelSnapshot => ({ bound: true, model: "anthropic/claude-sonnet-4", effort: "high", streaming: false,
  context: { tokens: 48_000, contextWindow: 200_000, percent: 24 }, tokens: 248_112, cost: 0.048201, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, jobs: null, auxiliary: null, steering: 0, followUps: 0, ...extra });
const lines = (model: UsageModel, width: number, extra: Partial<UsageViewOptions> = {}) =>
  usageLines(model, width, { legend: false, bodyRows: 0, ...extra }).map(row => stripTerminalSequences(row.text));
const worker = (snapshot: PanelSnapshot | null = pane()) => usageModel({ identity, snapshot });
const bounded = (page: readonly string[], width: number) => { for (const line of page) expect(visibleWidth(line)).toBeLessThanOrEqual(width); };

test("a single pane shows its tokens, estimate, context and model at every width; before it reports, usage is unknown, never zero", () => {
  for (const width of WIDTHS) {
    const page = lines(worker(), width), text = page.join("\n");
    bounded(page, width);
    for (const fact of ["SESSION", "248,112", "$0.048201", "24%", "anthropic/claude-sonnet-4"]) expect(text).toContain(fact);
    const silent = lines(worker(null), width).join("\n");
    expect(silent).toContain("unknown, not zero"); expect(silent).not.toMatch(/\$0\.0+\b|\b0 tokens/);
  }
});

test("session: an unknown member stays unknown, and a build total keeps its + in the table, the narrow rows and the digest", () => {
  const model: UsageModel = { identity: { role: "build", project: "demo", build: "slots" },
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

test("the back line opens with ‹ Chat at column 0, and its hit range covers exactly that label", () => {
  for (const width of [120, 44]) expect(lines(worker(), width)[0]?.startsWith("‹ Chat · Usage")).toBe(true);
  expect(usageBackAt(0, 0)).toBe(true); expect(usageBackAt("‹ Chat".length - 1, 0)).toBe(true);
  expect(usageBackAt("‹ Chat".length, 0)).toBe(false); expect(usageBackAt(0, 1)).toBe(false);
});

test("short views open with the session digest; taller views start at the section; ? appends the reading rules", () => {
  for (const width of WIDTHS) {
    const short = lines(worker(), width, { bodyRows: 6 });
    bounded(short, width);
    expect(short.findIndex(line => line.startsWith("SESSION"))).toBe(2);
    expect(short[1]).toContain("248,112");
    expect(lines(worker(), width, { bodyRows: 15 }).findIndex(line => line.startsWith("SESSION"))).toBe(1);
    expect(lines(worker(), width).join("\n")).not.toContain("READING RULES");
    expect(lines(worker(), width, { legend: true }).join(" ")).toContain("not a bill and not a provider quota");
  }
});

test("the Usage main view keeps the draft, appends the rules with ?, stays bounded when compact and F9 restores the transcript", () => {
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => pane(), changed() {}, canInteract: () => true });
  try {
    const viewport = createChatViewport({ document: new Text("actual transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("RETAINED DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage");
    const screen = (width: number) => stripTerminalSequences(renderLayoutFrame(viewport.root, width, 160, () => {}).lines.join("\n"));
    expect(screen(120)).toContain("‹ Chat · Usage");
    wb.handleInput("?"); expect(screen(120)).toContain("READING RULES");
    for (const line of renderLayoutFrame(viewport.root, 40, 160, () => {}).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(40);
    expect(screen(40)).toContain("248,112");
    expect(screen(40)).toContain("RETAINED DRAFT");
    wb.handleInput("\x1b[20~"); expect(wb.view).toBe("conversation");
    expect(screen(40)).toContain("actual transcript");
  } finally { wb.dispose(); }
});

test("paging a short Usage viewport down shows every row of the page, none skipped", () => {
  const wb = new NativeWorkbench({ identity, cwd: "/tmp", snapshot: () => pane(), changed() {}, canInteract: () => true });
  try {
    const viewport = createChatViewport({ document: new Text("transcript", 0, 0), pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0),
      editor: new Text("DRAFT", 0, 0), footer: new Text("", 0, 0), mainView: transcript => wb.mount(transcript) });
    wb.setView("usage"); wb.handleInput("?");
    const seen: string[] = [];
    for (let press = 0; press < 20; press++) {
      seen.push(stripTerminalSequences(renderLayoutFrame(viewport.root, 80, 9, () => {}).lines.join("\n")));
      wb.handleInput("\x1b[6~");
    }
    const shown = seen.join("\n");
    // The view is 78 cells (80 less the inset) and short, so the page opens with the digest; every row of it must have been on screen.
    const page = lines(worker(), 78, { legend: true, bodyRows: 5 });
    expect(page.length).toBeGreaterThan(8);
    for (const row of page.map(line => line.trim()).filter(Boolean)) expect(shown).toContain(row);
    expect(shown).toContain("DRAFT");
  } finally { wb.dispose(); }
});
