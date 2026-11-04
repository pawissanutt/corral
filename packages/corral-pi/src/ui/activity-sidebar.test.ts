import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { Text, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { createChatViewport } from "../../node_modules/@earendil-works/pi-coding-agent/dist/modes/interactive/chat-viewport.js";
import { NativePanels } from "./panels.ts";
import { activitySidebar } from "./activity-sidebar.ts";
import { overviewLines } from "./panels-chrome.ts";
import { activityLines } from "./activity-view.ts";
import { PANEL_TABS, type PanelSnapshot, type PanelJob } from "./panels-view.ts";
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

test("overview replaces Plan and mixed counters with distinct, actionable Agents and Jobs", () => {
  const rows = overviewLines(base, { kind: "ready", files: [] });
  expect(rows.filter(row => row.heading).map(row => row.text)).toEqual(["Changed files", "Agents · 2 active", "Jobs · 1 running"]);
  expect(rows.find(row => row.text.includes("first callers"))?.action).toEqual({ kind: "helper", request: "first" });
  expect(rows.find(row => row.text.includes("bun test build"))?.action).toEqual({ kind: "job", job: "build" });
  expect(rows.some(row => row.text.includes("Helper slots"))).toBeFalse();
});

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

test("rail mouse and F8/Enter open the same native agent chat directly, leaving the draft and parent intact", () => {
  const h = fixture();
  try {
    h.click("Find first");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "first" });
    expect(h.text()).toContain("ANSWER_first_0"); expect(h.text()).toContain("REAL_DRAFT");
    expect(h.text()).not.toContain("Helper conversations"); expect(h.text()).not.toContain("switches jobs/helpers");
    h.physical.send("\x1b[20~"); expect(h.native.workbench.view).toBe("conversation");
    h.physical.send("\x1b[19~"); expect(h.native.focused).toBeTrue();
    expect(h.native.workbench.view).toBe("conversation");
    h.physical.send("\x1b[B"); h.physical.send("\r");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "second" });
    expect(h.text()).toContain("ANSWER_second_0");
    expect(identity.generation).toBe(1); expect(h.text()).toContain("REAL_DRAFT");
  } finally { h.stop(); }
});

test("main navigation has no duplicate Agents or Jobs tab in any view or width", () => {
  expect(PANEL_TABS).toEqual(["Context", "Files", "Plan"]);
  const h = fixture();
  try {
    for (const select of [() => h.native.workbench.setView("conversation"),
      () => h.native.workbench.openHelpers("first"), () => h.native.workbench.openJob("build")]) {
      select();
      for (const width of [52, 80, 140]) {
        const bar = stripTerminalSequences(h.frame(width).lines[0] ?? "");
        expect(bar).not.toMatch(/\bAgents?\b|\bJobs?\b/);
        expect(bar).toContain(width < 64 ? " C " : "Chat");
        expect(bar).toContain(width < 64 ? " D " : "Changes");
        expect(bar).toContain(width < 64 ? " A " : "Activity");
      }
    }
    h.physical.send("\x1b[19~");
    expect(h.native.focused).toBeTrue();
    expect(h.native.tab).toBe("Context");
    const rail = h.native.renderSidebar(34, 34);
    expect(stripTerminalSequences(rail[1] ?? "")).toContain("Context Files Plan");
    expect(stripTerminalSequences(rail[1] ?? "")).not.toContain("Agents");
    expect(plain(rail)).toContain("Agents · 2 active");
    expect(plain(rail)).toContain("Jobs · 1 running");
    expect(plain(rail)).not.toContain("24%"); // the bottom bar owns context
    h.physical.send("\r");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "first" });
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
    h.click("bun test build"); expect(h.text()).toContain("OUTPUT_build");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "job", job: "build" });
    h.set({ ...base, jobs: [job("new"), job("build")] });
    expect(h.text()).toContain("OUTPUT_build"); expect(h.text()).not.toContain("OUTPUT_new");
    h.click("Find first");
    h.set({ ...base, helpers: [helper("new"), ...(base.helpers ?? [])] });
    expect(h.text()).toContain("ANSWER_first_0"); expect(h.text()).not.toContain("ANSWER_new");
    h.set({ ...base, helpers: [] });
    expect(h.text()).toContain("no longer retained");
    expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "first" });
  } finally { h.stop(); }
});

test("F8 compact drawer supports keyboard selection and pointer history expansion at narrow widths", () => {
  for (const width of [52, 80]) {
    const h = fixture();
    try {
      h.physical.resize(width, 24); h.physical.send("\x1b[19~");
      expect(h.native.focused).toBeTrue();
      const drawer = h.native.renderInline(width, 24);
      expect(drawer.length).toBeGreaterThan(3);
      for (const line of drawer) expect(visibleWidth(line)).toBe(width);
      h.physical.send("\x1b[B"); h.physical.send("\r");
      expect(h.native.workbench.selectedActivity).toEqual({ kind: "helper", request: "second" });
      expect(h.native.renderInline(width, 24)).toHaveLength(1);
      h.set({ ...base, helpers: [helper("done", "completed")], jobs: [] });
      h.physical.send("\x1b[19~");
      const rows = h.native.renderInline(width, 24), y = rows.findIndex(line => stripTerminalSequences(line).includes("Completed"));
      expect(y).toBeGreaterThan(0);
      h.native.inlineMouse({ type: "click", button: "left", x: 3, y, screenX: 3, screenY: y, width, height: 24, shift: false, ctrl: false, alt: false });
      expect(plain(h.native.renderInline(width, 24))).toContain("Find done");
    } finally { h.stop(); }
  }
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
