// The Agents page of a session that is not a build session: its fleet children, merged with the helper observations.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text } from "@earendil-works/pi-tui";
import type { AgentMessage, ChildSnapshot, FleetControl, FleetSnapshot } from "../build/types.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { agentsModel } from "./agents-model.ts";
import { verdictText } from "./build/fleet-rows.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { NativeWorkbench } from "./workbench.ts";

const NOW = Date.now();
const F8 = "\x1b[19~";
const worker = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1" };
beforeAll(() => sdk.initTheme("dark", false));
const work = mkdtempSync(join(tmpdir(), "corral-agents-fleet-"));
afterAll(() => rmSync(work, { recursive: true, force: true }));

function child(id: string, extra: Partial<ChildSnapshot> = {}): ChildSnapshot {
  return { id, kind: "slice", call: "call-1", title: `Deliver ${id}`, brief: `Brief for ${id}`, owns: [`src/${id}`], after: [], profile: "default",
    model: "anthropic/model-a", state: "running", createdAt: NOW - 50_000, startedAt: NOW - 49_000, edits: [],
    usage: { input: 10, output: 5, turns: 1 }, steers: [], pulse: [1], history: "r", messages: [], ...extra };
}
function fleet(children: readonly ChildSnapshot[], mail: readonly AgentMessage[] = []): FleetSnapshot {
  return { children, mail, maxParallel: 3, startedAt: NOW - 60_000 };
}
function helper(request: string, kind: HelperInspection["kind"]): HelperInspection {
  return { request, kind, model: "provider/model-a", phase: "completed", createdAt: NOW - 40_000, startedAt: NOW - 39_000,
    endedAt: NOW - 30_000, prompt: `Look into ${request}`, result: "Answer", truncated: false, messages: [] };
}
function snapshot(extra: Partial<PanelSnapshot> = {}): PanelSnapshot {
  return { bound: true, model: "openai-codex/example", effort: "high", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [],
    skills: [], ledger: null, ledgerStale: false, jobs: [], auxiliary: null, steering: 0, followUps: 0, helpers: [], ...extra };
}
function control(current: () => FleetSnapshot, calls: string[][]): FleetControl {
  return {
    snapshot: current, subscribe: () => () => {}, answer: () => true, roster: () => [], wait: async () => [],
    message: async () => { throw new Error("Unused fixture messaging"); },
    steer: async (id, text) => { calls.push(["steer", id, text]); return true; },
    cancel(id) { calls.push(["cancel", id]); return true; },
    setModel(id, model) { calls.push(["setModel", id, model]); },
    models: () => [], profiles: () => [],
  };
}
function workbench(state: PanelSnapshot, fleetControl?: FleetControl, ui: Partial<ExtensionUIContext> = {}): NativeWorkbench {
  const wb = new NativeWorkbench({ cwd: work, identity: worker, snapshot: () => state, changed() {}, canInteract: () => true,
    ...(fleetControl ? { fleet: () => fleetControl } : {}) });
  wb.mount(new Text("PARENT", 0, 0));
  wb.bind({ notify() {}, custom: async () => undefined, confirm: async () => true, input: async () => undefined, select: async () => undefined,
    getEditorText: () => "", setEditorText() {}, ...ui } as unknown as ExtensionUIContext);
  return wb;
}
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}

test("a worker's slice and review children are two rows, the verdict on the review row and the message in both conversations", () => {
  const verdict = { verdict: "approve" as const, summary: "Looks right", findings: [], at: NOW - 1000 };
  const mail: AgentMessage[] = [{ seq: 1, at: NOW - 20_000, from: "alpha", to: "check", text: "MESSAGE_BODY", delivery: "delivered" }];
  const wb = workbench(snapshot({ fleet: fleet([child("alpha"), child("check", { kind: "review", state: "done", endedAt: NOW - 1000, verdict })], mail) }));
  try {
    const model = wb.agentsModel();
    expect(model.agents.map(row => [row.source, row.id])).toEqual([["child", "alpha"], ["child", "check"]]);
    expect(model.agents[1]?.activity).toBe(verdictText(verdict));
    expect(model.messages).toBe(1);
    for (const row of model.agents) expect(row.mail?.map(message => message.text)).toEqual(["MESSAGE_BODY"]);
  } finally { wb.dispose(); }
});

test("an explore call that is a fleet child and a helper observation shows once, as the fleet row; other helpers stay", () => {
  const request = "a".repeat(64);
  const wb = workbench(snapshot({ fleet: fleet([child(`e-${request.slice(0, 30)}`, { kind: "explore" })]),
    helpers: [helper(request, "explore"), helper("oracle-call", "oracle")] }));
  try {
    expect(wb.agentsModel().agents.map(row => [row.source, row.profile.split(" · ")[0]])).toEqual([["child", "explore"], ["helper", "oracle"]]);
  } finally { wb.dispose(); }
});

test("without fleet data the model is the helper observations alone, whether the snapshot has no fleet or an empty one", () => {
  const helpers = [helper("b-call", "explore"), helper("a-call", "oracle")];
  const expected = agentsModel(snapshot({ helpers }), NOW).agents.map(row => [row.key, row.hue]);
  for (const state of [snapshot({ helpers }), snapshot({ helpers, fleet: fleet([]) })]) {
    const wb = workbench(state);
    try {
      const model = wb.agentsModel();
      expect(model.agents.map(row => [row.key, row.hue])).toEqual(expected);
      expect(model.maxParallel).toBeUndefined(); expect(model.messages).toBeUndefined();
    } finally { wb.dispose(); }
  }
});

test("a child's rows are read-only without a fleet control and offer steer, cancel and model with one", () => {
  const state = snapshot({ fleet: fleet([child("alpha")]) });
  const readOnly = workbench(state), controlled = workbench(state, control(() => state.fleet!, []));
  try {
    expect(readOnly.agentsModel().agents[0]?.capabilities.cancel).toBeUndefined();
    expect(controlled.agentsModel().agents[0]?.capabilities.cancel).toBe(true);
  } finally { readOnly.dispose(); controlled.dispose(); }
});

test("cancelling a child from the Agents page calls fleetControl.cancel with its id", async () => {
  const state = snapshot({ fleet: fleet([child("alpha"), child("beta")]) }), calls: string[][] = [];
  const wb = workbench(state, control(() => state.fleet!, calls));
  try {
    expect(wb.handleInput(F8)).toBeTrue();
    wb.agentsView.select("child:beta");
    expect(wb.handleInput("x")).toBeTrue();
    await settle();
    expect(calls).toEqual([["cancel", "beta"]]);
  } finally { wb.dispose(); }
});

test("s on a child sends the typed message through fleetControl.steer", async () => {
  const state = snapshot({ fleet: fleet([child("alpha")]) }), calls: string[][] = [];
  const wb = workbench(state, control(() => state.fleet!, calls), { input: async () => "Check the edge case" });
  try {
    wb.handleInput(F8);
    wb.agentsView.select("child:alpha");
    expect(wb.handleInput("s")).toBeTrue();
    await settle();
    expect(calls).toEqual([["steer", "alpha", "Check the edge case"]]);
  } finally { wb.dispose(); }
});
