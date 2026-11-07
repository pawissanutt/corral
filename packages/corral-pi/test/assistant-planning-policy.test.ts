import { expect, test } from "bun:test";
import { PlanningPolicy, planningBookkeeping, planningState } from "../src/host/planning-policy.ts";

const workflow = (phase = "planning") => ({ version: 1, workflow: "W1", state_revision: 1,
  authority_epoch: 1, phase, owner: { kind: "local" }, boundary: { activities: [] as { id: string; tool: string; uncertain: boolean }[] } });

test("the fixed planning ceiling is closed, including grants, metadata-labelled tools and generic authority", async () => {
  for (const phase of ["entering", "recovering", "planning", "executing", "candidate", "paused", "handoff_preparing", "handoff_reconciling", "board_owned"]) {
    let effects = 0;
    const policy = new PlanningPolicy({ read: async () => workflow(phase), record: async () => { effects++; }, fence() {} });
    await policy.refresh();
    for (const name of ["write", "edit", "bash", "user_bash", "job_start", "mcp_connect", "mcp_activate", "mcp_resources",
      "mcp_read_resource", "mcp_read_labelled", "lsp_diagnostics", "browser_navigate", "session_export", "corral_memory_put",
      "corral_owner_action", "new_unknown_tool"]) {
      await expect(policy.execute(name, { path: "/tmp/own-artifact" }, name, async () => { effects++; })).rejects.toThrow("assistant_planning");
    }
    for (const name of ["read", "grep", "find", "ls", "web_fetch", "tool_search", "codemode", "explore", "ask_user", "job_kill"]) {
      expect(await policy.execute(name, {}, name, async () => "read")).toBe("read");
    }
    expect(effects).toBe(0);
  }
  expect(planningBookkeeping("corral_assistant_plan", { action: "publish" })).toBeTrue();
  expect(planningBookkeeping("corral_todo", { op: "done" })).toBeTrue();
  expect(planningBookkeeping("corral_assistant_plan", { action: "begin_execution" })).toBeFalse();
  expect(planningBookkeeping("corral_assistant_plan", { action: "arbitrary" })).toBeFalse();
  expect(planningState({ container: { version: 1 } }).phase).toBe("ordinary");
  for (const value of [null, {}, { container: { version: 99 } }, { ...workflow(), version: 2 }]) {
    expect(() => planningState(value)).toThrow("recovery required");
  }
});

test("entry orders before new admission and cannot claim planning until the actual effect settles", async () => {
  let state: ReturnType<typeof workflow> | { workflow: null; phase: string; boundary: ReturnType<typeof workflow>["boundary"] } = {
    workflow: null, phase: "ordinary", boundary: { activities: [] },
  };
  const launched = Promise.withResolvers<void>(), release = Promise.withResolvers<void>();
  let writes = 0;
  const policy = new PlanningPolicy({ read: async () => state, fence() {}, record: async t => {
    if (t.action === "start") state.boundary.activities.push({ id: String(t.id), tool: String(t.tool), uncertain: false });
    if (t.action === "settle") state.boundary.activities = state.boundary.activities.filter(a => a.id !== t.id);
    if (t.action === "enter") state = { ...workflow("entering"), boundary: state.boundary };
    if (t.action === "drained") state = { ...workflow(), authority_epoch: 2 };
    return state;
  } });
  await policy.refresh();
  const running = policy.execute("write", {}, "held", async () => { launched.resolve(); await release.promise; writes++; });
  await launched.promise;
  const entering = await policy.enter("genuine-input", async () => {});
  expect(entering.phase).toBe("entering");
  await expect(policy.execute("write", {}, "late", async () => { writes++; })).rejects.toThrow("assistant_planning");
  release.resolve(); await running;
  expect((await policy.drained()).phase).toBe("planning");
  expect(writes).toBe(1); // prior admitted work, not replayed or described as undone
});

test("lost start acknowledgement and old-generation activity remain fenced on recovery", async () => {
  const state = workflow("entering");
  state.boundary.activities.push({ id: "g1-old-shell", tool: "bash", uncertain: true });
  const restored = new PlanningPolicy({ read: async () => state, record: async () => { throw new Error("must not acknowledge drain"); }, fence() {} });
  expect((await restored.drained()).phase).toBe("recovering");
  const ordinary = new PlanningPolicy({ read: async () => ({ workflow: null, phase: "ordinary" }),
    record: async () => { throw new Error("lost acknowledgement"); }, fence() {} });
  let writes = 0;
  await expect(ordinary.execute("write", {}, "uncertain", async () => { writes++; })).rejects.toThrow("lost acknowledgement");
  expect(ordinary.restricted).toBeTrue();
  expect(writes).toBe(0);
});
