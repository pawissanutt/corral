import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { connectBridge } from "./native-bridge.mjs";
import { ProbeFailure } from "./credentials.mjs";

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
 * Assertions drive the same upstream InteractiveMode session through its private
 * bridge. Terminal answers are synthetic fixture input, never owner acceptance.
 * @param {{bridge: Awaited<ReturnType<import("./native-bridge.mjs").createBridge>>,
 * scope: Record<string,string>, state: import("./native-tools.mjs").FixtureState,
 * bootstrap: {capability:string,pane:string}, offline?: boolean}} options
 */
export async function nativeScenarios({ bridge, scope, state, bootstrap, offline = false }) {
  const identity = { path: bridge.path, capability: bootstrap.capability,
    scope: { ...scope }, generation: state.generation };
  const client = await connectBridge(identity);
  /** @type {string[]} */
  const passed = [];
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
    return current;
  }
  try {
    await restricted();
    assert.equal((await client.request("state", {}, { capability: "foreign" })).error, "foreign_session_scope");
    assert.equal((await client.request("state", {}, { scope: { ...scope, project: "foreign" } })).error, "foreign_session_scope");
    assert.equal((await client.request("state", {}, { generation: 0 })).error, "stale_generation");
    passed.push("scope_and_generation_fencing");
    const handled = client.waitFor("input_handled");
    await command("handled");
    await handled;
    passed.push("handled_without_run");
    const denied = await command("mutate");
    assert.deepEqual(denied.details, { isError: true, mutations: 0 });
    const question = client.waitFor("fixture_dialog_open");
    await command("ask");
    await question;
    const answered = client.waitFor("fixture_decision_answered");
    await herdr(["pane", "send-text", bootstrap.pane, "approve fixture-decision-1 rev1"]);
    await herdr(["pane", "send-keys", bootstrap.pane, "enter"]);
    assert.equal((await answered).approved, true);
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
    passed.push("trusted_fixture_answer_and_lost_ack_replay");
    if (!offline) {
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
      passed.push("visible_session_idle_busy_steering_followup_and_settled");
      const cancelledHold = client.waitFor("provider_held");
      const cancelled = client.waitFor("agent_settled");
      await command("start");
      await cancelledHold;
      await command("cancel");
      assert.equal((await cancelled).aborted, true);
      assert.equal((await command("state")).mutations, 1);
      passed.push("native_cancel_preserves_committed_fixture");
      const failure = client.waitFor("assistant_final");
      const failedSettled = client.waitFor("agent_settled");
      await command("fail");
      assert.equal((await failure).stopReason, "error");
      await failedSettled;
      passed.push("injected_provider_failure_not_task_completion");
    }
    await command("reload");
    await restricted();
    await command("switch_model");
    await restricted();
    const replacement = await command("replace");
    assert.equal(typeof replacement.generation, "number");
    assert.equal(typeof replacement.sessionId, "string");
    assert.equal((await client.request("state")).error, "foreign_session_scope");
    client.setSession(/** @type {number} */ (replacement.generation), /** @type {string} */ (replacement.sessionId));
    await restricted();
    assert.equal((await command("state")).mutations, 1);
    passed.push("restriction_survives_reload_and_session_replacement");
    return { decision: offline ? "fixture-only" : "bounded-native-proof",
      nativeProofComplete: !offline, passed, fixtureMutationCount: 1,
      productionMutationCount: 0, terminalActor: "simulated_fixture_terminal",
      receiptDurability: "in-memory-only", taskCompletionInferredFromIdle: false };
  } catch (error) {
    bridge.emit({ event: "scenario_failed", passed,
      assertion: error instanceof assert.AssertionError ? error.message : "fixture_operation_failed" });
    throw new ProbeFailure("native_scenario_failed");
  } finally {
    client.close();
  }
}
