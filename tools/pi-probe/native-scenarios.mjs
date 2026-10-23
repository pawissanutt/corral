import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { connectBridge } from "./native-bridge.mjs";
import { ProbeFailure } from "./credentials.mjs";

/** T-136's six native criteria, in the order the GO record reports them. */
export const CRITERIA = /** @type {const} */ (["terminal_control", "busy_input", "cancellation",
  "lifecycle_receipts", "session_ownership", "restricted_tools"]);
/** @typedef {typeof CRITERIA[number]} Criterion */
/** @typedef {{origin: "harness_port" | "upstream_incompatibility", code: string, name?: string, detail?: string, frame?: string}} Failure */

/** @param {string[]} args */
async function herdr(args) {
  await new Promise((resolve, reject) => {
    const child = spawn("herdr", ["--session", "corral-test", ...args], { stdio: "ignore" });
    child.once("error", () => reject(new ProbeFailure("fixture_terminal_input_failed")));
    child.once("exit", (code) => code === 0 ? resolve(undefined) :
      reject(new ProbeFailure("fixture_terminal_input_failed")));
  });
}

/**
 * Keeps a harness-port failure (probe, fake transport, bridge or fixture
 * plumbing) apart from an upstream incompatibility: an assertion on observed
 * upstream behavior, a ProbeFailure tagged upstream, or an exception whose
 * first stack frame lies inside the installed packages. `detail` is omitted
 * for live runs, whose provider errors are never copied into logs.
 * @param {unknown} error @param {boolean} [detail]
 * @returns {Failure}
 */
export function classifyFailure(error, detail = true) {
  if (error instanceof ProbeFailure) {
    return { origin: error.origin === "upstream_incompatibility" ? error.origin : "harness_port", code: error.code };
  }
  const text = (error instanceof Error ? error.message : String(error)).slice(0, 300);
  if (error instanceof assert.AssertionError) {
    return { origin: "upstream_incompatibility", code: "assertion_failed", ...(detail ? { detail: text } : {}) };
  }
  const stack = error instanceof Error ? error.stack ?? "" : "";
  const frame = stack.split("\n").find((line) => /^\s+at /.test(line))?.trim().slice(0, 300) ?? "";
  return { origin: frame.includes("/node_modules/") ? "upstream_incompatibility" : "harness_port",
    code: "uncaught_error", name: error instanceof Error ? error.name : typeof error,
    ...(detail ? { detail: text, frame } : {}) };
}

/**
 * Assertions drive the same upstream InteractiveMode session through its private
 * bridge. Terminal answers are synthetic fixture input, never owner acceptance.
 * Steps run in order; after a failure the rest are NOT_RUN, since each step
 * builds on the session state the previous one left.
 * @param {{bridge: Awaited<ReturnType<import("./native-bridge.mjs").createBridge>>,
 * scope: Record<string,string>, state: import("./native-tools.mjs").FixtureState,
 * bootstrap: {capability:string,pane:string}, live?: boolean}} options
 */
export async function nativeScenarios({ bridge, scope, state, bootstrap, live = false }) {
  const identity = { path: bridge.path, capability: bootstrap.capability,
    scope: { ...scope }, generation: state.generation };
  const client = await connectBridge(identity);
  /** @param {string} kind @param {Record<string,unknown>} [data] */
  async function command(kind, data = {}) {
    const response = await client.request(kind, data);
    assert.equal(response.error, undefined, kind);
    assert.ok(response.data && typeof response.data === "object");
    return /** @type {Record<string,unknown>} */ (response.data);
  }
  async function restricted() {
    const current = await command("state");
    assert.equal(current.hasUI, true);
    assert.equal(current.mode, "tui");
    assert.deepEqual(current.activeTools, ["pm_readback"]);
    assert.equal(current.skills, 0);
    assert.equal(current.prompts, 0);
    assert.equal(current.contextFiles, 0);
    assert.equal(current.forbiddenExecutions, 0);
    const result = await command("permissions");
    const details = /** @type {{denials: Array<{denied:boolean}>, nestedReadSucceeded:boolean}} */ (result.details);
    assert.equal(details.nestedReadSucceeded, true, "allowed nested execution must work before denial proof");
    assert.equal(details.denials.length, 7);
    assert.ok(details.denials.every((item) => item.denied));
  }
  /** @type {Array<[string, Criterion[], () => Promise<void>]>} */
  const steps = [
    ["terminal_bound", ["terminal_control"], async () => {
      const current = await command("state");
      assert.equal(current.hasUI, true);
      assert.equal(current.mode, "tui");
      assert.ok(typeof current.sessionId === "string" && current.sessionId.length > 0);
    }],
    ["restricted_initial", ["restricted_tools"], restricted],
    ["scope_and_generation_fencing", ["session_ownership"], async () => {
      assert.equal((await client.request("state", {}, { capability: "foreign" })).error, "foreign_session_scope");
      assert.equal((await client.request("state", {}, { scope: { ...scope, project: "foreign" } })).error, "foreign_session_scope");
      assert.equal((await client.request("state", {}, { generation: 0 })).error, "stale_generation");
    }],
    ["handled_without_run", ["lifecycle_receipts"], async () => {
      const before = await command("state");
      const handled = client.waitFor("input_handled");
      await command("handled");
      await handled;
      const after = await command("state");
      assert.equal(after.streaming, false);
      assert.deepEqual(after.providerOutputs, before.providerOutputs);
      assert.equal(after.transportRequests, before.transportRequests);
    }],
    ["trusted_terminal_answer", ["terminal_control"], async () => {
      const denied = await command("mutate");
      assert.deepEqual(denied.details, { isError: true, mutations: 0 });
      const question = client.waitFor("fixture_dialog_open");
      await command("ask");
      await question;
      const answered = client.waitFor("fixture_decision_answered");
      await herdr(["pane", "send-text", bootstrap.pane, "approve fixture-decision-1 rev1"]);
      await herdr(["pane", "send-keys", bootstrap.pane, "enter"]);
      assert.equal((await answered).approved, true);
    }],
    ["lost_ack_replay", ["lifecycle_receipts"], async () => {
      const lost = await connectBridge(identity);
      await lost.request("state");
      const committed = client.waitFor("fixture_committed");
      lost.sendAndClose("mutate", {}, "lost-ack-fixture-1");
      await committed;
      const replay = await client.request("mutate", {}, { id: "lost-ack-fixture-1" });
      assert.equal(replay.error, undefined);
      assert.deepEqual(/** @type {{details:unknown}} */ (replay.data).details, { isError: false, mutations: 1 });
      assert.equal((await client.request("mutate", { conflict: true }, { id: "lost-ack-fixture-1" })).error,
        "conflicting_request_payload");
      assert.equal((await command("state")).mutations, 1);
    }],
    ["busy_steer_follow_up", ["busy_input"], async () => {
      const held = client.waitFor("provider_held");
      const settled = client.waitFor("agent_settled");
      assert.equal((await command("start")).disposition, "submitted_not_completed");
      await held;
      assert.equal((await command("state")).streaming, true);
      assert.equal((await command("steer")).disposition, "queued");
      assert.equal((await command("follow_up")).disposition, "queued");
      await command("release");
      assert.equal((await settled).aborted, false);
      const completed = await command("state");
      assert.equal(completed.streaming, false);
      assert.deepEqual(completed.deliveries, ["start", "steer", "follow_up"]);
      const outputs = /** @type {Array<{stopReason:string,outputTokens:number}>} */ (completed.providerOutputs);
      assert.ok(outputs.length >= 2 && outputs.length <= 3);
      assert.ok(outputs.every((output) => output.stopReason === "stop" &&
        output.outputTokens > 0 && output.outputTokens <= 1024));
    }],
    ["cancel_in_flight", ["cancellation"], async () => {
      const before = await command("state");
      const held = client.waitFor("provider_held");
      const final = client.waitFor("assistant_final");
      const settled = client.waitFor("agent_settled");
      await command("start");
      await held;
      await command("cancel");
      assert.equal((await final).stopReason, "aborted");
      assert.equal((await settled).aborted, true);
      const after = await command("state");
      assert.equal(after.streaming, false);
      assert.equal(after.mutations, 1);
      // Offline, the abort reaches the open fake response stream, not a pre-dispatch hold.
      if (!live) assert.equal(after.transportAborted, Number(before.transportAborted) + 1);
    }],
    ["provider_failure_not_completion", ["lifecycle_receipts"], async () => {
      const failure = client.waitFor("assistant_final");
      const settled = client.waitFor("agent_settled");
      await command("fail");
      assert.equal((await failure).stopReason, "error");
      await settled;
      const after = await command("state");
      assert.equal(after.streaming, false);
      assert.equal(after.mutations, 1);
      if (!live) assert.equal(after.transportFailures, 1);
    }],
    ["restriction_after_reload_and_model_switch", ["restricted_tools"], async () => {
      await command("reload");
      await restricted();
      await command("switch_model");
      await restricted();
    }],
    ["session_replacement_rebind", ["session_ownership", "restricted_tools"], async () => {
      const replacement = await command("replace");
      assert.equal(typeof replacement.generation, "number");
      assert.equal(typeof replacement.sessionId, "string");
      assert.equal((await client.request("state")).error, "foreign_session_scope");
      client.setSession(/** @type {number} */ (replacement.generation), /** @type {string} */ (replacement.sessionId));
      await restricted();
      assert.equal((await command("state")).mutations, 1);
    }],
  ];
  /** @type {Array<{name: string, criteria: Criterion[], status: "PASS" | "FAIL" | "NOT_RUN", failure?: Failure}>} */
  const results = [];
  try {
    for (const [name, criteria, run] of steps) {
      if (results.some((item) => item.status === "FAIL")) { results.push({ name, criteria, status: "NOT_RUN" }); continue; }
      try {
        await run();
        results.push({ name, criteria, status: "PASS" });
      } catch (error) {
        const failure = classifyFailure(error, !live);
        results.push({ name, criteria, status: "FAIL", failure });
        bridge.emit({ event: "scenario_failed", step: name, origin: failure.origin, code: failure.code });
      }
    }
  } finally {
    client.close();
  }
  const failures = results.flatMap((item) => item.failure ? [{ step: item.name, ...item.failure }] : []);
  const criteria = Object.fromEntries(CRITERIA.map((name) => {
    const mine = results.filter((item) => item.criteria.includes(name));
    return [name, mine.some((item) => item.status === "FAIL") ? "FAIL" :
      mine.every((item) => item.status === "PASS") ? "PASS" : "NOT_RUN"];
  }));
  return { criteria, steps: results, allPassed: failures.length === 0,
    harnessPortFailures: failures.filter((item) => item.origin === "harness_port"),
    upstreamIncompatibilities: failures.filter((item) => item.origin === "upstream_incompatibility"),
    fixtureMutationCount: state.mutations, productionMutationCount: 0,
    terminalActor: "simulated_fixture_terminal", receiptDurability: "in-memory-only",
    taskCompletionInferredFromIdle: false };
}
