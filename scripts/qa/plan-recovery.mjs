// T-100 R1 recovery checks; run inside scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "A private plugin-free herdr server is required");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-100/R1-live-recovery");
await mkdir(evidence, { recursive: true });
const root = await mkdtemp(join(tmpdir(), "corral-plan-recovery-"));
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
const env = { ...process.env };
for (const key of ["CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const commands = [];
const observed = {};
async function run(program, args, required = true) {
  const child = Bun.spawn([program, ...args], { cwd: root, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ program, args, stdout, stderr, code });
  if (required) assert.equal(code, 0, `${args.join(" ")}\n${stderr}`);
  return { stdout, stderr, code };
}
const cli = (args, required = true) => run(binary, ["--root", root, ...args], required);
const json = async args => JSON.parse((await cli(args)).stdout);
const show = id => json(["show", id, "--json"]);
const set = (key, value) => cli(["config", "set", key, value]);
const planText = goal => `---
goal: ${goal}
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
let passed = false;
try {
  await mkdir(join(root, "app"));
  await run("git", ["init", "-q", "-b", "main", join(root, "app")]);
  await cli(["init"]);
  await set("harnesses.plan-fixture", '{command = "true {model_args}; read -r fixture_input < /dev/tty", flags = "generic", model_flag = "--model"}');
  await set("harnesses.review-fixture", '{command = "true {model_args}", flags = "generic", model_flag = "--model"}');
  for (const [name, model] of [["gpt-sol", "fixture-planner"], ["gpt-review", "fixture-reviewer"]]) {
    await set(`profiles.${name}`, `{harness = "${name === "gpt-sol" ? "plan-fixture" : "review-fixture"}", model = "${model}", vendor = "openai"}`);
  }
  await set("defaults.profile", '"gpt-sol"');
  await set("review.reviewer", '"gpt-review"');
  await set("limits.running", "1");
  await set("limits.review", "1");
  await set("sync.startup_grace_secs", "1");
  await set("resources.max_load_per_core", "1000000");
  await set("resources.min_mem_available", '"1B"');
  await set("resources.min_disk_free", '"1B"');
  const notes = join(root, "notes.md");
  await writeFile(notes, "Review the widget contract.\n");
  const verdict = (id, plan, attempt, value, required = true) => cli([
    "plan", "verdict", id, value, "--plan", plan,
    ...(attempt ? ["--attempt", attempt] : ["--owner"]), "--notes-file", notes,
  ], required);

  const replacement = (await cli(["new", "Replacement widget", "-p", "app", "--ready", "--design"])).stdout.trim();
  for (const revision of ["P1", "P2"]) {
    await cli(["dispatch", replacement, "--plan"]);
    const path = join(root, `${revision}.md`);
    await writeFile(path, planText(`Widget ${revision}`));
    await cli(["plan", "submit", replacement, path]);
    await cli(["sync"]);
    const review = (await show(replacement)).plans.at(-1).reviews.at(-1);
    await verdict(replacement, revision, review.id, "approve");
    await verdict(replacement, revision, null, "approve");
    assert.equal((await show(replacement)).plan, `${replacement}@${revision}`);
  }
  observed.replacement = await show(replacement);
  assert.equal(observed.replacement.plans.length, 2);
  assert.equal(observed.replacement.plan_runs.length, 2);
  assert(observed.replacement.plans.every(p => p.approval));
  await cli(["move", replacement, "dropped"]);

  const lost = (await cli(["new", "Lost reviewer", "-p", "app", "--ready", "--design"])).stdout.trim();
  await cli(["dispatch", lost]);
  await cli(["plan", "submit", lost, join(root, "P1.md")]);
  await cli(["sync"]);
  const attempt = (await show(lost)).plans[0].reviews[0];
  // Time itself is tested: subscribe to the exact whole-second grace boundary.
  const graceElapsed = new Promise(resolveReady =>
    setTimeout(resolveReady, Math.max(0, Date.parse(attempt.at) + 2100 - Date.now())));
  await run("herdr", ["--session", "corral-test", "workspace", "close", attempt.herdr.workspace_id]);
  await graceElapsed;
  await cli(["sync"]);
  observed.lost = await show(lost);
  assert.equal(observed.lost.plan_runs[0].state, "escalated");
  assert.equal(observed.lost.plans[0].reviews[0].verdict, "escalate");
  assert.equal((await json(["pm", "brief", "--json"])).slots.used, 0);
  assert.notEqual((await verdict(lost, "P1", attempt.id, "approve", false)).code, 0);

  const retry = await json(["plan", "review", lost, "--json"]);
  assert.notEqual(retry.id, attempt.id);
  assert.equal((await json(["pm", "brief", "--json"])).slots.used, 1);
  await verdict(lost, "P1", null, "changes");
  const superseded = await show(lost);
  assert.equal(superseded.plans[0].reviews.find(r => r.id === retry.id).verdict, "escalate");
  assert.equal((await json(["pm", "brief", "--json"])).slots.used, 0);
  assert.notEqual((await verdict(lost, "P1", retry.id, "approve", false)).code, 0);
  await cli(["sync"]);
  observed.revised = await show(lost);
  assert.equal(observed.revised.plan_runs[0].rounds, 1);
  assert.equal(observed.revised.status, "running");
  assert.equal(observed.revised.plan_runs[0].ended, null);
  assert.equal((await json(["pm", "brief", "--json"])).slots.used, 1);
  passed = true;
  console.log("PASS: replacement acceptance, lost-reviewer retry, stale callbacks and one-slot planner recovery.");
} finally {
  await writeFile(join(evidence, "commands.log"), commands.map(c => JSON.stringify(c)).join("\n") + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify({
    status: passed ? "PASS" : "FAIL", observed,
    cleanup: "private server helper owns panes; fixture removed",
  }, null, 2) + "\n");
  await rm(root, { recursive: true, force: true });
}
