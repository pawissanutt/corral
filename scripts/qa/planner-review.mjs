// T-105 review regressions. Run in scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "A private plugin-free test server is required");
const root = await mkdtemp(join(tmpdir(), "corral-planner-review-"));
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-105/review-fixes");
await mkdir(evidence, { recursive: true });
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
const env = { ...process.env };
for (const key of ["CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const commands = [];
const checks = [];
const observed = {};
let passed = false;
async function run(program, args, required = true, cwd = root) {
  const child = Bun.spawn([program, ...args], { cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ program, args, code, stdout, stderr });
  if (required) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { code, stdout, stderr };
}
const cli = (args, required = true) => run(binary, ["--root", root, ...args], required);
const json = async args => JSON.parse((await cli(args)).stdout);
const show = id => json(["show", id, "--json"]);
const set = (key, value) => cli(["config", "set", key, value]);
const check = (name, condition) => { assert(condition, name); checks.push(name); };
const planText = `---
goal: Widget
non_goals: []
decisions: []
affected_paths: [widget.rs]
contracts: []
dependencies: []
acceptance: [{scenario: spin, expected: clockwise}]
verification: [check]
owner_decisions: []
---
`;
try {
  const repo = join(root, "app");
  await mkdir(repo);
  await run("git", ["init", "-q", "-b", "main", repo]);
  await cli(["init"]);
  await set("review.autopilot", "false");
  await set("resources.max_load_per_core", "1000000");
  await set("resources.min_mem_available", '"1B"');
  await set("resources.min_disk_free", '"1B"');
  await set("harnesses.plan-fixture", '{command = "true {model_args}; read -r fixture_input < /dev/tty", flags = "generic", model_flag = "--model"}');
  await set("harnesses.review-fixture", '{command = "true {model_args}", flags = "generic", model_flag = "--model"}');
  await set("profiles.gpt-sol", '{harness = "plan-fixture", model = "fixture-planner", vendor = "openai"}');
  await set("profiles.gpt-review", '{harness = "review-fixture", model = "fixture-reviewer", vendor = "openai"}');
  await set("defaults.profile", '"gpt-sol"');
  await set("review.reviewer", '"gpt-review"');
  const plan = join(root, "plan.md");
  await writeFile(plan, planText);
  const create = async title => (await cli(["new", title, "-p", "app", "--ready", "--design"])).stdout.trim();

  const queued = await create("Explicit pending-plan replacement");
  await cli(["plan", "submit", queued, plan]);
  await set("dispatch.paused", "true");
  await cli(["dispatch", queued, "--plan", "--queue"]);
  observed.queuedBefore = await show(queued);
  check("explicit replacement permission is persisted separately from planning mode",
    observed.queuedBefore.queued && observed.queuedBefore.dispatch_plan.plan
    && observed.queuedBefore.dispatch_plan.explicit_plan);
  await set("dispatch.paused", "false");
  observed.pmBrief = await json(["pm", "brief", "--json"]);
  observed.pmNext = await json(["pm", "next", "--json"]);
  check("PM readers return JSON without implicit pending-plan dispatch",
    observed.pmBrief.next.kind !== "dispatch" && observed.pmNext.kind !== "dispatch");
  assert.deepEqual(await show(queued), observed.queuedBefore);
  check("PM readers leave the queued task unchanged", true);
  observed.queueListing = await json(["queue", "list", "--json"]);
  check("explicit replacement is eligible after pause release",
    observed.queueListing.find(t => t.id === queued).reason === "eligible");
  await cli(["sync"]);
  observed.queuedAfter = await show(queued);
  check("sync drains the explicit replacement into an actual new planner",
    observed.queuedAfter.status === "running" && observed.queuedAfter.plan_runs.length === 1
    && observed.queuedAfter.plan_runs[0].state === "planning" && !observed.queuedAfter.queued);
  await cli(["plan", "cancel", queued]);

  const implicit = await create("Implicit planning must wait");
  await set("dispatch.paused", "true");
  await cli(["dispatch", implicit, "--queue"]);
  await cli(["plan", "submit", implicit, plan]);
  await set("dispatch.paused", "false");
  await cli(["sync"]);
  observed.implicit = await show(implicit);
  check("implicit queued planning never becomes replacement permission",
    observed.implicit.status === "ready" && observed.implicit.queued
    && !observed.implicit.plan_runs?.length && !observed.implicit.dispatch_plan.explicit_plan);
  // Finish this disposable owner fixture before enabling unrelated review work.
  await cli(["move", implicit, "dropped", "--owner-ok"]);

  const cancelled = await create("Late reviewer callback after cancellation");
  await cli(["dispatch", cancelled, "--plan"]);
  await cli(["plan", "submit", cancelled, plan]);
  const review = await json(["plan", "review", cancelled, "--json"]);
  await cli(["plan", "cancel", cancelled]);
  observed.cancelledBefore = await show(cancelled);
  const notes = join(root, "review-notes.md");
  await writeFile(notes, "Choose the smaller API contract.");
  await cli(["plan", "verdict", cancelled, "changes", "--plan", "P1", "--attempt", review.id, "--notes-file", notes]);
  observed.cancelledAfterVerdict = await show(cancelled);
  check("late review changes remain available without overwriting cancellation",
    observed.cancelledAfterVerdict.plans[0].reviews.at(-1).verdict === "changes"
    && observed.cancelledAfterVerdict.plan_runs[0].state === "cancelled");
  await set("review.autopilot", "true");
  await cli(["sync"]);
  observed.cancelledAfterSync = await show(cancelled);
  assert.deepEqual(observed.cancelledAfterSync.plan_runs, observed.cancelledBefore.plan_runs);
  check("autopilot sync preserves terminal execution identity rounds pane and end time",
    observed.cancelledAfterSync.status === "needs_me" && observed.cancelledAfterSync.dispatch_hold);
  observed.finalBrief = await json(["pm", "brief", "--json"]);
  check("late callback and sync cannot reacquire a planner slot", observed.finalBrief.slots.used === 0);
  const refused = await cli(["dispatch", cancelled], false);
  check("a cancelled planner requires explicit replacement dispatch", refused.code !== 0);
  await cli(["dispatch", cancelled, "--plan"]);
  const replacement = await show(cancelled);
  check("explicit dispatch starts a fresh D2 instead of reviving D1",
    replacement.plan_runs.length === 2 && replacement.plan_runs[0].state === "cancelled"
    && replacement.plan_runs[1].id === "D2" && replacement.plan_runs[1].state === "planning");
  await cli(["plan", "cancel", cancelled]);
  passed = true;
  console.log(`PASS: ${checks.length} private-server CLI checks for all three T-105 review findings.`);
} finally {
  await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
  await writeFile(join(evidence, "observed.json"), JSON.stringify({ passed, checks, observed }, null, 2) + "\n");
  await writeFile(join(evidence, "events.jsonl"), await readFile(join(root, ".corral/events.jsonl"), "utf8").catch(() => ""));
  await rm(root, { recursive: true, force: true });
}
