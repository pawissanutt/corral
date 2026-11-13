import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { agentsModel, type AgentRow, type AgentsModel } from "./agents-model.ts";
import { agentDensity, renderAgents, AgentsView, type AgentsAction, type AgentsOptions } from "./agents-view.ts";
import { agentColor } from "./agent-hues.ts";
import { AGENT_HUES, PACKAGED, type Appearance } from "./appearance.ts";
import { PANEL_MUTED, type PanelJob, type PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import type { BuildPanelState, ChildSnapshot, FleetSnapshot } from "../build/types.ts";
import { childKey } from "./build/contract.ts";
import { fleetExtras } from "./build/fleet-rows.ts";

const NOW = 1_000_000;
function helper(request: string, phase: HelperInspection["phase"], createdAt: number, extra: Partial<HelperInspection> = {}): HelperInspection {
  return { request, kind: "explore", model: "provider/model-a", generation: 1, phase, createdAt,
    ...(phase === "queued" ? {} : { startedAt: createdAt + 2000 }), ...(phase === "completed" ? { endedAt: createdAt + 30_000, result: "Answer line" } : {}),
    prompt: `Find ${request} callers\nsecond line`, truncated: false,
    messages: phase === "running" ? [{ id: "t", kind: "tool", tool: "read", phase: "running", arguments: JSON.stringify({ path: "src/a.ts" }), text: "" }] : [], ...extra };
}
function job(id: string, state: "running" | "exited" | "lost", exitCode: number | null = null): PanelJob {
  return { job: id, state, detail: { job: id, command: `bun test ${id}`, state: state === "lost" ? "exited" : state, startedAt: NOW - 20_000,
    endedAt: state === "running" ? undefined : NOW - 1000, exitCode, signal: null, groupClear: true, bytes: 10, truncated: false,
    tail: `OUTPUT_${id}\n`, lines: 1, monitorAttempts: 0, watches: [] } };
}
const fixture: PanelSnapshot = { bound: true, model: "m", effort: "off", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null,
  helpers: [helper("h-3", "running", NOW - 40_000), helper("h-2", "completed", NOW - 90_000), helper("h-1", "queued", NOW - 20_000, { kind: "oracle" })],
  jobs: [job("build", "running"), job("lint", "exited", 1)],
  agents: [{ id: "h-2", session: "s-2", kind: "explore", state: "completed", model: "model-a", transcript: true }] };
const options = (overrides: Partial<AgentsOptions> = {}): AgentsOptions =>
  ({ now: NOW, short: false, legend: false, freeze: undefined, color: agentColor, ...overrides });
const plain = (lines: readonly string[]) => lines.map(line => stripTerminalSequences(line));

function viewHost(model: () => AgentsModel, acted: [AgentsAction, string][] = []) {
  return { model, now: () => NOW, short: () => false, legend: () => false, freeze: () => undefined, color: agentColor,
    click: (event: TuiMouseEvent) => event.type === "click" && event.button === "left", focus() {},
    act(action: AgentsAction, key: string) { acted.push([action, key]); } };
}
function click(view: AgentsView, width: number, height: number, x: number, y: number): void {
  const frame = renderLayoutFrame(view.scroll, width, height, () => {});
  for (const box of getLayoutBoxesAt(frame, x, y)) {
    const result = box.component.handleMouse?.({ type: "click", button: "left", x: x - box.rect.x, y: y - box.rect.y, screenX: x, screenY: y,
      width: box.rect.width, height: box.rect.height, shift: false, alt: false, ctrl: false, clickCount: 1 });
    if (result?.handled) return;
  }
}

test("the page follows the agents, jobs, detail and timeline order at 140 and 52 columns without captions", () => {
  for (const width of [140, 52]) {
    for (const selected of [undefined, "job:build"]) {
      const rows = plain(renderAgents(agentsModel(fixture, NOW, { jobControl: true }), selected, width, options()).lines);
      const at = (needle: string) => rows.findIndex(row => row.includes(needle));
      expect(at("explore")).toBeGreaterThanOrEqual(0);
      expect(at("oracle")).toBeGreaterThan(at("explore"));
      expect(at("Jobs")).toBeGreaterThan(at("oracle"));
      expect(at("bun test lint")).toBeGreaterThan(at("bun test build"));
      expect(at("Timeline")).toBeGreaterThan(at(selected ? "OUTPUT_build" : "open conversation"));
      expect(rows.at(-1)).toContain("0s");
      const text = rows.join("\n");
      expect(text).not.toMatch(/observations, not task settlement|host observations|unavailable|No helper calls/);
    }
  }
});

test("selection stays on the same key when a newer helper is inserted above it", () => {
  let snapshot = fixture;
  const view = new AgentsView(viewHost(() => agentsModel(snapshot, NOW)));
  view.select("helper:h-2");
  snapshot = { ...fixture, helpers: [helper("h-new", "running", NOW - 1000), ...(fixture.helpers ?? [])] };
  expect(view.entry()?.row.key).toBe("helper:h-2");
  const frame = renderAgents(agentsModel(snapshot, NOW), view.selected, 140, options());
  expect(stripTerminalSequences(frame.lines[frame.selectedRow] ?? "")).toMatch(/^ ▌  ✓  explore\b/);
  view.move(1);
  expect(view.entry()?.row.key).toBe("helper:h-1");
});

test("every line fits the width, and the table density switches at the mock's 60 and 112 content cells", () => {
  // The mock measures the area minus four cells of padding: a 61-column pane is narrow, a 92-column pane is not.
  expect([63, 64, 115, 116].map(agentDensity)).toEqual(["min", "med", "med", "full"]);
  expect([46, 61, 92, 150].map(agentDensity)).toEqual(["min", "min", "med", "full"]);
  const model = agentsModel(fixture, NOW, { jobControl: true });
  for (const width of [34, 52, 80, 112, 140]) {
    for (const selected of [undefined, "helper:h-2", "job:build", "job:gone"]) {
      for (const short of [false, true]) {
        for (const line of renderAgents(model, selected, width, options({ short, legend: true })).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      }
    }
  }
  const header = (width: number) => plain(renderAgents(model, undefined, width, options()).lines).some(row => row.includes("profile · model"));
  expect(header(116)).toBe(true); expect(header(115)).toBe(false);
  const profile = (width: number) => plain(renderAgents(model, undefined, width, options()).lines).some(row => /explore\s.*model-a/.test(row));
  expect(profile(64)).toBe(true); expect(profile(63)).toBe(false);
});

test("a helper row offers no steer, cancel or model, while a child row shows them dim with the reason", () => {
  const acted: [AgentsAction, string][] = [];
  const helperView = new AgentsView(viewHost(() => agentsModel(fixture, NOW), acted));
  helperView.select("helper:h-3");
  expect(helperView.actions()).toEqual(["open"]);
  const helperText = plain(renderAgents(agentsModel(fixture, NOW), "helper:h-3", 140, options()).lines).join("\n");
  expect(helperText).toContain("open conversation");
  expect(helperText).not.toMatch(/\b(steer|cancel)\b/);
  expect(helperView.handleKey("s")).toBe(false);

  const [base] = agentsModel(fixture, NOW).agents;
  if (!base) throw new Error("fixture has no helper rows");
  const child: AgentRow = { ...base, key: "child:c-1", source: "child", id: "c-1", name: "host-slots", state: "done",
    capabilities: { open: true, transcript: false, steer: true, cancel: true, model: true },
    disabled: { steer: "not running", cancel: "not running", model: "not running" } };
  const model = () => agentsModel(fixture, NOW, { extra: { agents: [child] } });
  const childView = new AgentsView(viewHost(model, acted));
  childView.select("child:c-1");
  const frame = renderAgents(model(), "child:c-1", 140, options());
  const actionRow = frame.lines.find(line => stripTerminalSequences(line).includes("open conversation")) ?? "";
  for (const label of ["steer", "cancel", "model"]) expect(actionRow).toContain(PANEL_MUTED + label);
  // An unavailable action is its muted label alone: no key cap before it and no reason after it.
  expect(stripTerminalSequences(actionRow)).toMatch(/ ⏎  open conversation  steer  cancel  model\s*$/);
  const actionHits = [...frame.hits.values()].flat().flatMap(hit => hit.key === "child:c-1" && hit.action ? [hit.action] : []);
  expect(actionHits.filter(action => action === "steer" || action === "cancel" || action === "model")).toEqual([]);
  expect(childView.actions()).toEqual(["open"]);
  expect(childView.handleKey("x")).toBe(true);
  expect(acted).toEqual([]);
});

test("a stale host or a lost job never shows retained job output", () => {
  const stale = plain(renderAgents(agentsModel({ ...fixture, bound: false }, NOW), "job:build", 140, options()).lines).join("\n");
  expect(stale).toContain("Output unavailable: stale/disconnected");
  expect(stale).not.toContain("OUTPUT_build");
  const lost = plain(renderAgents(agentsModel({ ...fixture, jobs: [job("build", "lost")] }, NOW), "job:build", 140, options()).lines).join("\n");
  expect(lost).toContain("Detailed output unavailable");
  expect(lost).not.toContain("OUTPUT_build");
});

test("an unavailable or empty source adds no caption; an empty page says how agents appear", () => {
  for (const width of [140, 80, 52]) {
    const unknown = plain(renderAgents(agentsModel({ ...fixture, jobs: null, helpers: null }, NOW), undefined, width, options()).lines).join("\n");
    expect(unknown).not.toContain("unavailable");
    expect(unknown).not.toContain("Jobs");
    expect(unknown).toContain("No agents yet.");
  }
  const jobsOnly = plain(renderAgents(agentsModel({ ...fixture, helpers: [] }, NOW), undefined, 80, options()).lines).join("\n");
  expect(jobsOnly).toContain("Jobs");
  expect(jobsOnly).not.toMatch(/No agents yet|No helper calls/);
  const agentsOnly = plain(renderAgents(agentsModel({ ...fixture, jobs: [] }, NOW), undefined, 80, options()).lines).join("\n");
  expect(agentsOnly).not.toContain("Jobs");
});

test("clicking an action label runs that action for the shown row", () => {
  const acted: [AgentsAction, string][] = [];
  const view = new AgentsView(viewHost(() => agentsModel(fixture, NOW, { jobControl: true }), acted));
  view.select("helper:h-2");
  const lines = plain(renderAgents(agentsModel(fixture, NOW, { jobControl: true }), "helper:h-2", 140, options()).lines);
  const y = lines.findIndex(line => line.includes("transcript") && line.includes("open conversation"));
  click(view, 140, lines.length, (lines[y] ?? "").indexOf("transcript") + 1, y);
  expect(acted).toEqual([["transcript", "helper:h-2"]]);
});

test("agent hues keep at least 4.5:1 contrast on the panel background and the selected row in every appearance", () => {
  const luminance = (hex: string) => {
    const [r = 0, g = 0, b = 0] = [1, 3, 5].map(index => {
      const value = Number.parseInt(hex.slice(index, index + 2), 16) / 255;
      return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (a: string, b: string) => {
    const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return ((high ?? 0) + 0.05) / ((low ?? 0) + 0.05);
  };
  for (const appearance of Object.keys(AGENT_HUES) as Appearance[]) {
    expect(AGENT_HUES[appearance]).toHaveLength(8);
    for (const hue of AGENT_HUES[appearance]) {
      expect(contrast(hue, PACKAGED[appearance].background)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(hue, PACKAGED[appearance].selected)).toBeGreaterThanOrEqual(4.5);
    }
  }
});

function buildChild(id: string, extra: Partial<ChildSnapshot> = {}): ChildSnapshot {
  return { id, call: "call-1", title: `Deliver ${id}`, brief: `Brief for ${id}`, owns: [`src/${id}/`], after: [], profile: "default",
    model: "anthropic/model-a", state: "running", createdAt: NOW - 200_000, startedAt: NOW - 190_000, edits: [],
    usage: { input: 1200, output: 300, turns: 4, cost: 0.12, contextPercent: 41 }, steers: [], pulse: [1, 3, 5, 2], history: "qrrr", messages: [], ...extra };
}
const QUESTION = { question: "The wireframe shows two layouts; which one should the picker render?", options: [{ label: "Grid" }, { label: "List" }], askedAt: NOW - 5000 };
const MOCK_FLEET: FleetSnapshot = { maxParallel: 3, startedAt: NOW - 200_000, children: [
  buildChild("host-slots", { state: "done", endedAt: NOW - 100_000, history: "qrrrrd", report: "Slots stored per host\nmore",
    edits: [{ path: "src/host-slots/a.rs", outside: false, at: NOW - 150_000 }, { path: "src/app-store/b.rs", outside: true, at: NOW - 140_000 }] }),
  buildChild("app-store", { activity: "edit src/app-store/store.rs", steers: ["Keep the store API stable"] }),
  buildChild("picker-ui", { state: "needs", question: QUESTION, history: "qrrn" }),
  buildChild("arrival", { state: "queued", after: ["app-store"], startedAt: undefined, history: "qqqq", usage: { input: 0, output: 0, turns: 0 } }),
]};
function buildState(fleet: FleetSnapshot): BuildPanelState {
  return {
    session: { version: 1, id: "B-1", name: "save-slots", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z", repoRoot: "/repo",
      worktree: "/repo/.worktrees/build-save-slots", branch: "build/save-slots", base: null, here: false },
    settings: { exitPolicy: "gate", maxParallel: fleet.maxParallel, profiles: [] }, fleet, target: "parent", exit: { phase: "none" }, board: [],
  };
}
const buildModel = (fleet = MOCK_FLEET) => agentsModel(fixture, NOW, { extra: fleetExtras(buildState(fleet), NOW) });

test("child rows lead the table in fleet order, with their detail and key-cap actions, at 140 and 52 columns", () => {
  for (const width of [140, 52]) {
    for (const selected of [undefined, ...MOCK_FLEET.children.map(child => childKey(child.id))]) {
      const rows = plain(renderAgents(buildModel(), selected, width, options()).lines);
      const at = (needle: string) => rows.findIndex(row => row.includes(needle));
      expect(at("host-slots")).toBeGreaterThanOrEqual(0);
      expect(at("app-store")).toBeGreaterThan(at("host-slots"));
      expect(at("picker-ui")).toBeGreaterThan(at("app-store"));
      expect(at("arrival")).toBeGreaterThan(at("picker-ui"));
      expect(at("explore")).toBeGreaterThan(at("arrival"));
      expect(at("Timeline")).toBeGreaterThan(at("open conversation"));
      const text = rows.join("\n");
      expect(text).toMatch(/ ⏎  open conversation/);
      expect(text).not.toMatch(/model \S+\/|call call-1|session file/);
      if (selected === childKey("picker-ui")) expect(text).toContain("picker render");
      if (selected === childKey("app-store")) expect(text).toContain("Keep the store API stable");
    }
  }
});

test("the full table keeps the mock's column cells: header and rows aligned, a 14-cell sparkline, waiting and done times", () => {
  const lines = renderAgents(buildModel(), childKey("app-store"), 150, options()).lines, rows = plain(lines);
  const header = rows[0] ?? "", find = (needle: string) => rows.find(row => row.includes(needle)) ?? "";
  expect(header.indexOf("agent")).toBe(7);
  expect(header.indexOf("profile · model")).toBe(20);
  expect(header.indexOf("activity")).toBe(81);
  expect(header.indexOf("now")).toBe(97);
  const done = find("host-slots"), running = find("app-store"), queued = find("arrival");
  expect(done.indexOf("host-slots")).toBe(7);
  // Time is right-aligned under its heading: done shows its run time, a queued child reads `waiting`.
  expect(done.indexOf("1m30s") + "1m30s".length).toBe(header.indexOf("time") + "time".length);
  expect(queued.indexOf("waiting") + "waiting".length).toBe(header.indexOf("time") + "time".length);
  // Tool calls per 5 s bucket [1, 3, 5, 2], scaled to the busiest bucket shown, right-aligned in 14 cells.
  expect(running.slice(81, 95)).toBe("          ▁▄▇▃");
  expect(running.indexOf("edit src/app-store/store.rs")).toBe(97);
  // The selected row is the only one on another surface and carries the accent bar.
  expect(running.startsWith(" ▌")).toBeTrue();
  for (const line of lines) expect(visibleWidth(line)).toBe(150);
});

test("med and min densities fold the activity onto a second line under the name", () => {
  for (const width of [92, 46]) {
    const rows = plain(renderAgents(buildModel(), undefined, width, options()).lines);
    const name = rows.findIndex(row => row.includes("app-store"));
    expect(rows[name + 1]?.indexOf("edit src/app-store/store.rs")).toBe((rows[name] ?? "").indexOf("app-store"));
    expect(rows.some(row => row.includes("profile · model"))).toBeFalse();
    expect((rows[name] ?? "").includes("model-a")).toBe(width >= 60);
  }
});

test("the timeline spends one column per 5 s and its axis spans only the bars", () => {
  const rows = plain(renderAgents(buildModel(), undefined, 150, options()).lines);
  const axis = rows.at(-1) ?? "";
  // 200 s since the fleet started: 40 columns from the bar start at cell 13, ending in the elapsed time.
  expect(axis.indexOf("0s")).toBe(13);
  expect(axis.trimEnd().length).toBe(13 + 40);
  expect(axis.trimEnd().endsWith("3m20s")).toBeTrue();
  const done = [...rows].reverse().find(row => row.startsWith(" host-slots")) ?? "";
  expect(done.trimEnd().endsWith("✓")).toBeTrue();
  expect(done.trimEnd().length).toBeLessThanOrEqual(13 + 40);
});

test("helper rows beside child rows still offer no steer, cancel, model or answer", () => {
  const acted: [AgentsAction, string][] = [];
  const view = new AgentsView(viewHost(() => buildModel(), acted));
  view.select("helper:h-3");
  expect(view.actions()).toEqual(["open"]);
  for (const key of ["s", "x", "m", "a"]) expect(view.handleKey(key)).toBe(false);
  expect(acted).toEqual([]);
});

test("a answers only a child that asks, and the action row offers answer only while it asks", () => {
  const acted: [AgentsAction, string][] = [];
  const view = new AgentsView(viewHost(() => buildModel(), acted));
  view.select(childKey("app-store"));
  expect(view.handleKey("a")).toBe(false);
  expect(acted).toEqual([]);
  view.select(childKey("picker-ui"));
  expect(view.handleKey("a")).toBe(true);
  expect(acted).toEqual([["answer", childKey("picker-ui")]]);

  const actionRow = (key: string) => plain(renderAgents(buildModel(), key, 140, options()).lines).find(line => line.includes("open conversation")) ?? "";
  expect(actionRow(childKey("picker-ui"))).toContain("answer");
  for (const id of ["host-slots", "app-store", "arrival"]) expect(actionRow(childKey(id))).not.toContain("answer");
});

test("child rows with long ids, owns and steers stay within the width", () => {
  const long = "a-very-long-child-identifier-that-overflows-every-column";
  const fleet: FleetSnapshot = { ...MOCK_FLEET, children: [...MOCK_FLEET.children,
    buildChild(long, { state: "needs", question: QUESTION, pendingModel: "anthropic/model-with-a-very-long-identifier-name",
      owns: Array.from({ length: 12 }, (_, index) => `crates/some/deeply/nested/module-${index}/file.rs`),
      steers: Array.from({ length: 6 }, (_, index) => `Steer number ${index} with a long explanation of what to change next`),
      edits: [{ path: "outside/the/owned/paths/and/far/away.rs", outside: true, at: NOW }], error: "boom ".repeat(40) })] };
  for (const width of [34, 52, 80, 112, 140]) {
    for (const selected of [undefined, childKey(long), childKey("arrival"), childKey("host-slots")]) {
      for (const short of [false, true]) {
        for (const line of renderAgents(buildModel(fleet), selected, width, options({ short })).lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      }
    }
  }
});
