import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { NativePanels } from "./panels.ts";
import { activitySidebar } from "./activity-sidebar.ts";
import { activityLines } from "./activity-view.ts";
import type { PanelSnapshot, PanelJob } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

beforeAll(() => initTheme("dark", false));
const identity = { store: "fixture", project: "demo", role: "assistant" as const, assistant: "A-demo", generation: 1 };
const helper = (request: string, phase: HelperInspection["phase"] = "running"): HelperInspection => ({
  request, phase, kind: "explore", model: "fixture/model", generation: 1, prompt: `Find ${request} callers`, createdAt: 1,
  truncated: false, messages: [{ id: "answer", kind: "assistant", text: Array.from({ length: 50 }, (_, i) => `ANSWER_${request}_${i}`).join("\n\n") }],
});
const job = (id: string, code?: number): PanelJob => ({ job: id, state: code === undefined ? "running" : "exited", detail: {
  job: id, command: `bun test ${id}`, state: code === undefined ? "running" : "exited", startedAt: 1000,
  endedAt: code === undefined ? undefined : 3000, exitCode: code ?? null, signal: null, groupClear: code !== undefined, bytes: 10, tail: `OUTPUT_${id}`,
  truncated: false, monitorAttempts: 0, watches: [],
} });
const base: PanelSnapshot = { bound: true, model: "fixture", effort: "high", streaming: false, tokens: 0,
  context: { tokens: 48000, contextWindow: 200000, percent: 24 }, tools: [], skills: [], contextFiles: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null, helpers: [helper("first"), helper("second")], jobs: [job("build")] };
const plain = (lines: readonly string[]) => stripTerminalSequences(lines.join("\n"));
function fixture() {
  let snapshot = base;
  const native = new NativePanels({ identity, cwd: "/tmp", snapshot: () => snapshot });
  const physical = new RecordingTerminal(), terminal = native.wrap(physical);
  terminal.start(() => {}, () => {});
  const viewport = createChatViewport({ document: new Text(Array.from({ length: 100 }, (_, i) => `PARENT_${i}`).join("\n"), 0, 0),
    pendingMessages: new Text("", 0, 0), status: new Text("", 0, 0), editor: new Text("REAL_DRAFT", 0, 0), footer: new Text("", 0, 0),
    mainView: transcript => native.mainView(transcript) });
  const frame = (width = 80) => renderLayoutFrame(viewport.root, width, 30, () => {});
  return { native, physical, viewport, frame, text: () => plain(frame().lines), set(value: PanelSnapshot) { snapshot = value; },
    click(needle: string) {
      const rows = native.renderSidebar(34, 34), row = rows.findIndex(line => stripTerminalSequences(line).includes(needle));
      if (row < 0) throw new Error(`Missing rail target: ${needle}\n${plain(rows)}`);
      physical.send(`\x1b[<0;109;${row + 1}M`); physical.send(`\x1b[<0;109;${row + 1}m`);
    }, stop() { terminal.stop(); native.dispose(); } };
}

test("history collapses successful completions but keeps failures and uncertain agents visible", () => {
  const snapshot = { ...base, helpers: [helper("done", "completed"), helper("interrupted", "uncertain")], jobs: [job("pass", 0), job("fail", 7)] };
  const collapsed = activitySidebar(snapshot), text = collapsed.map(row => row.text).join("\n");
  expect(text).not.toContain("Find done"); expect(text).not.toContain("bun test pass");
  expect(text).toContain("interrupted"); expect(text).toContain("bun test fail");
  expect(collapsed.find(row => row.action?.kind === "history")?.text).toBe("▸ Completed (2)");
  const expanded = activitySidebar(snapshot, undefined, true).map(row => row.text).join("\n");
  expect(expanded).toContain("Find done"); expect(expanded).toContain("bun test pass");
  expect(activitySidebar(null)[0]?.text).toContain("unavailable");
  expect(activitySidebar({ ...base, bound: false, helpers: null, jobs: null }).map(row => row.text).join("\n")).toContain("observations may be stale");
});

test("rail Running rows and F8 open Activity, leaving the draft and parent intact", () => {
  const h = fixture();
  try {
    h.physical.resize(140, 30);
    h.click("Find first");
    expect(h.native.workbench.view).toBe("activity");
    expect(h.text()).toContain("REAL_DRAFT");
    h.physical.send("\x1b[20~"); expect(h.native.workbench.view).toBe("conversation");
    h.physical.send("\x1b[19~"); expect(h.native.workbench.view).toBe("activity");
    expect(identity.generation).toBe(1); expect(h.text()).toContain("REAL_DRAFT");
  } finally { h.stop(); }
});

test("main navigation has no duplicate Agents or Jobs tab in any view or width", () => {
  const h = fixture();
  try {
    for (const select of [() => h.native.workbench.setView("conversation"),
      () => h.native.workbench.openHelpers("first"), () => h.native.workbench.openJob("build")]) {
      select();
      // Below the rail breakpoint the tabs row is the navigation.
      h.physical.resize(100, 30);
      for (const width of [52, 80, 140]) {
        const bar = stripTerminalSequences(h.frame(width).lines[0] ?? "");
        expect(bar).not.toMatch(/\bAgents?\b|\bJobs?\b/);
        expect(bar).toContain(width < 64 ? " C " : "Chat");
        expect(bar).toContain(width < 64 ? " D " : "Changes");
        expect(bar).toContain(width < 64 ? " A " : "Activity");
      }
    }
    // With the rail visible it is the navigation: no tabs row, and the rail lists destinations.
    h.physical.resize(140, 30);
    expect(stripTerminalSequences(h.frame(105).lines[0] ?? "")).not.toContain(" Changes ");
    const nav = plain(h.native.renderSidebar(34, 30));
    expect(nav).toContain("Assistant · A-demo"); expect(nav).toContain("Activity"); expect(nav).not.toMatch(/\bAgents?\b|\bJobs?\b/);
    expect(nav).toMatch(/Running\s+3/); expect(nav).toContain("Explore · Find first callers"); expect(nav).toContain("bun test build");
    expect(nav).not.toContain("24%"); // the bottom bar owns context
  } finally { h.stop(); }
});

test("merged job guards suppress retained output for disconnected hosts and lost jobs", () => {
  const disconnected = activityLines({ ...base, bound: false }, 0).join("\n");
  expect(disconnected).toContain("Output unavailable: stale/disconnected");
  expect(disconnected).not.toContain("OUTPUT_build");
  const lost = activityLines({ ...base, jobs: [{ ...job("build"), state: "lost" }] }, 0).join("\n");
  expect(lost).toContain("Detailed output unavailable");
  expect(lost).not.toContain("OUTPUT_build");
});

test("Jobs open output, keep stable identity through insertion, and never steal the selected agent view", () => {
  const h = fixture();
  try {
    h.native.workbench.openJob("build"); expect(h.text()).toContain("OUTPUT_build");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "job", job: "build" });
    h.set({ ...base, jobs: [job("new"), job("build")] });
    expect(h.text()).toContain("OUTPUT_build"); expect(h.text()).not.toContain("OUTPUT_new");
    h.native.workbench.openHelpers("first");
    h.set({ ...base, helpers: [helper("new"), ...(base.helpers ?? [])] });
    expect(h.text()).toContain("ANSWER_first_0"); expect(h.text()).not.toContain("ANSWER_new");
    h.set({ ...base, helpers: [] });
    expect(h.text()).toContain("no longer retained");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "first" });
  } finally { h.stop(); }
});

test("agent and parent reading positions survive return, live updates and a different selected item", () => {
  const h = fixture();
  try {
    h.frame(); h.viewport.transcript.scrollTo(12, { disableFollow: true }); h.frame();
    const parentTop = h.viewport.transcript.scrollTop;
    h.native.workbench.openHelpers("first"); h.frame();
    h.native.workbench.handleInput("\x1b[6~");
    const reading = h.text(); expect(reading).not.toContain("ANSWER_first_0");
    h.native.workbench.handleInput("\x1b[20~"); h.frame();
    expect(h.viewport.transcript.scrollTop).toBe(parentTop);
    h.native.workbench.openHelpers("second"); h.frame();
    h.native.workbench.openHelpers("first");
    expect(h.text()).toBe(reading);
  } finally { h.stop(); }
});
