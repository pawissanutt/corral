import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { visibleWidth, stripTerminalSequences } from "@earendil-works/pi-tui";
import { PanelTerminal, type PanelSurface } from "./panel-terminal.ts";
import { NativePanels } from "./panels.ts";
import { box, panelLines, type PanelSnapshot } from "./panels-view.ts";
import { composerBar, contextLabel, overviewLines, panelFooter, panelHeader } from "./panels-chrome.ts";
import { STATUS_BINDING, STATUS_ALERTS } from "./chrome.ts";
import { parseStatus, readPanelDiff, readPanelFiles } from "./panels-git.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const,
  task: "T-001", run: "R1", generation: 3 };
const snapshot: PanelSnapshot = {
  bound: true, model: "openai-codex/example", effort: "high", streaming: true,
  context: { tokens: 1234, contextWindow: 10000, percent: 12.34 }, tokens: 5678,
  tools: ["read"], contextFiles: ["/repo/AGENTS.md"], skills: [], ledger: null, ledgerStale: false,
  jobs: [{ job: "job-1", state: "running" }], auxiliary: null, steering: 1, followUps: 2,
};
const work = mkdtempSync(join(tmpdir(), "corral-native-panels-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

function fixture() {
  const physical = new RecordingTerminal();
  const inputs: string[] = [];
  let changes = 0, consumed = false;
  const surface: PanelSurface = {
    sidebarEnabled: true,
    renderSidebar: (width, height) => box("Real inspector", ["session data"], width, height),
    handleInput: () => consumed,
    layoutChanged: () => { changes++; },
  };
  const terminal = new PanelTerminal(physical, surface);
  terminal.start(data => inputs.push(data), () => { changes++; });
  return { physical, surface, terminal, inputs, changes: () => changes, consume: () => { consumed = true; } };
}

test("public terminal adapter reserves only a wide native rail and preserves normal-screen width", () => {
  const { physical, terminal } = fixture();
  expect(terminal.columns).toBe(105);
  physical.resize(104, 34); expect(terminal.columns).toBe(104);
  physical.resize(105, 34); expect(terminal.columns).toBe(70);
  terminal.write("\x1b[?1049l"); expect(terminal.columns).toBe(105);
  terminal.stop();
});
test("main content/cursor bytes are preserved, rail is synchronized, and drawing stops before leaving alternate screen", () => {
  const { physical, terminal } = fixture();
  const frame = "\x1b[?1049h\x1b[?7l\x1b[?2026h\x1b[1;1HREAL STREAM\x1b[?2026l";
  terminal.write(frame);
  const output = physical.writes.at(-1) ?? "";
  // Only SGR default colors may be decorated; content, cursor and framing stay byte-identical.
  expect(output.replace(/\x1b\[[\d;:]*m/g, "").startsWith(frame.slice(0, -8))).toBeTrue();
  expect(output).toContain("REAL STREAM");
  expect(output).toContain("\x1b[1;107H");
  expect(output).toContain("Real inspector");
  expect(output.endsWith("\x1b[?2026l")).toBeTrue();
  terminal.write("\x1b[?1049lSHELL");
  expect(physical.writes.at(-1)).toBe("\x1b[?1049lSHELL");
  terminal.stop(); const mark = physical.writes.length; terminal.redrawSidebar();
  expect(physical.writes.length).toBe(mark);
});
test("native viewport paints an explicit canvas instead of inheriting the owner's terminal background", () => {
  const { physical, terminal } = fixture(); physical.resize(80, 34);
  terminal.write("\x1b[?1049h\x1b[2J\x1b[0mconversation\x1b[49m");
  const output = physical.writes.at(-1) ?? "";
  expect(output).toContain("\x1b[48;2;15;16;18m\x1b[2J");
  expect(output).toContain("\x1b[0m\x1b[38;2;224;227;233m\x1b[48;2;15;16;18mconversation");
  terminal.write("\x1b[?1049l"); terminal.stop();
});
test("rail painting never interrupts split clipboard/image protocol payloads", () => {
  const { physical, terminal } = fixture();
  terminal.write("\x1b[?1049h");
  terminal.write("\x1b]52;c;partial");
  expect(physical.writes.at(-1)).toBe("\x1b]52;c;partial");
  const count = physical.writes.length; terminal.redrawSidebar();
  expect(physical.writes.length).toBe(count);
  terminal.write("payload\x07");
  expect(physical.writes.at(-1)?.startsWith("payload\x07")).toBeTrue();
  expect(physical.writes.at(-1)).toContain("Real inspector");
  terminal.write("\x1b(");
  expect(physical.writes.at(-1)).toBe("\x1b(");
  const pending = physical.writes.length; terminal.redrawSidebar();
  expect(physical.writes.length).toBe(pending);
  terminal.write("B"); expect(physical.writes.at(-1)?.startsWith("B")).toBeTrue();
  expect(physical.writes.at(-1)).toContain("Real inspector");
  terminal.write("\x1b[?1049l"); terminal.stop();
});
test("main input and resize stay on the original terminal while consumed inspector keys never submit", () => {
  const f = fixture();
  f.physical.send("draft"); expect(f.inputs).toEqual(["draft"]);
  f.consume(); f.physical.send("\r"); expect(f.inputs).toEqual(["draft"]);
  f.physical.resize(80, 32); expect(f.changes()).toBe(2);
  f.terminal.stop();
});
test("main pointer gestures retain their release across the rail and Kitty releases do not toggle layout", () => {
  const native = new NativePanels({ identity, cwd: work, snapshot: () => snapshot });
  const physical = new RecordingTerminal(), terminal = native.wrap(physical), input: string[] = [];
  terminal.start(data => input.push(data), () => {});
  physical.send("\x1b[<0;10;5M"); physical.send("\x1b[<32;120;5M"); physical.send("\x1b[<0;120;5m");
  expect(input).toHaveLength(3);
  physical.send("\x1b[<0;109;2M"); physical.send("\x1b[<0;10;2m");
  expect(input).toHaveLength(3);
  physical.send("\x1b[98;5:3u"); expect(native.sidebarEnabled).toBeTrue();
  terminal.stop(); native.dispose();
});
test("native panel boxes are bounded at supported widths, sanitize controls and reserve wide glyph cells", () => {
  for (const width of [34, 52, 80, 140]) {
    const lines = box("Session\x1b[2J", ["界界界".repeat(100)], width, 7, true);
    expect(lines).toHaveLength(7);
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    expect(lines.join("\n")).not.toContain("\x1b[2J");
  }
});
test("view data uses measured values and explicit unavailable/empty states, never fixture quotas or progress", () => {
  const context = panelLines("Context", identity, snapshot, { kind: "unread" }).map(line => line.text).join("\n");
  expect(context).toContain("1 active tools · 0 skills"); expect(context).toContain("5,678 recorded session tokens");
  expect(context).toContain("job-1 · running");
  expect(panelLines("Plan", identity, snapshot, { kind: "unread" }).map(line => line.text)).toContain("No canonical plan recorded");
  expect(contextLabel(null)).toBe("Context unavailable");
  expect(contextLabel(snapshot)).toBe("1.2k/10k · 12% context");
  expect(overviewLines(snapshot, { kind: "ready", files: [] }).map(row => row.text)).toContain("Working tree clean");
  expect(stripTerminalSequences(context)).not.toContain("42%");
});
test("ambient context inspection does not repeat composer model or context-usage chrome", () => {
  const context = panelLines("Context", identity, snapshot, { kind: "unread" }).map(line => line.text).join("\n");
  expect(context).not.toContain("openai-codex/example");
  expect(context).not.toContain("1,234 / 10,000");
});
test("model and measured context have one ambient owner, with binding and refusals retained", () => {
  for (const width of [52, 80, 105, 140]) for (const inRail of [false, true]) {
    const statuses = new Map([[STATUS_BINDING, "● corral g3"], [STATUS_ALERTS, "2 tool denied"]]);
    const header = panelHeader(identity, "/repo/demo", "main", snapshot, width);
    const footer = panelFooter(snapshot, inRail, statuses, width);
    const inspector = (inRail ? overviewLines(snapshot, { kind: "ready", files: [] })
      : panelLines("Context", identity, snapshot, { kind: "unread" })).map(row => row.text);
    const screen = stripTerminalSequences([...header, composerBar(snapshot, width), ...footer, ...inspector].join("\n"));
    expect(screen.match(/example/g)).toHaveLength(1);
    expect(screen.match(/12% context/g)).toHaveLength(1);
    expect(screen).toContain("Bound to Corral");
    expect(screen).toContain("2 tool denied");
    expect(screen).not.toContain("● corral g3");
    for (const line of [...header, composerBar(snapshot, width), ...footer]) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
  }
});
test("narrow metadata is one row at rest; focused and expanded details are bounded and retain selection", () => {
  const native = new NativePanels({ identity, cwd: work, snapshot: () => snapshot });
  const physical = new RecordingTerminal(); physical.resize(80, 34);
  native.wrap(physical);
  expect(native.renderInline(80, 34)).toHaveLength(1);
  expect(native.handleInput("\x1b[17~")).toBeTrue();
  expect(native.renderInline(80, 34)).toHaveLength(3);
  native.handleInput("\x1b"); expect(native.renderInline(80, 34)).toHaveLength(1);
  native.handleInput("\x02"); expect(native.renderInline(80, 34)).toHaveLength(5);
  physical.resize(52, 24);
  for (const line of native.renderInline(52, 24)) expect(visibleWidth(line)).toBe(52);
  expect(native.tab).toBe("Context");
  native.dispose();
});
test("Git porcelain parsing handles real NUL separators, rename source fields, spaces and Unicode", () => {
  expect(parseStatus(" M a file.ts\0R  新.ts\0old.ts\0?? --flag.ts\0")).toEqual([
    { status: " M", path: "a file.ts" }, { status: "R ", path: "新.ts" }, { status: "??", path: "--flag.ts" },
  ]);
});
test("cancelled native reads reject instead of leaving an owned Git request alive", async () => {
  const abort = new AbortController(); abort.abort();
  await expect(readPanelFiles(work, abort.signal)).rejects.toThrow();
});
test("file inspector reads an actual owned Git candidate and diff without editing its files or index", async () => {
  const git = (...args: string[]) => execFileSync("git", ["-C", work, ...args], { encoding: "utf8" });
  git("init", "-q"); git("config", "user.name", "Native Panel Test"); git("config", "user.email", "fixture@example.invalid");
  writeFileSync(join(work, "candidate.ts"), "export const value = 1;\n");
  git("add", "candidate.ts"); git("commit", "-qm", "base");
  writeFileSync(join(work, "candidate.ts"), "export const value = 2;\n");
  const before = git("status", "--porcelain=v1");
  const files = await readPanelFiles(work); expect(files).toEqual([{ path: "candidate.ts", status: " M" }]);
  const file = files[0]; if (!file) throw new Error("expected changed file");
  const diff = await readPanelDiff(work, file);
  expect(diff).toContain("-export const value = 1;"); expect(diff).toContain("+export const value = 2;");
  expect(git("status", "--porcelain=v1")).toBe(before);
});
