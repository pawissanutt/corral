// T-133 real CLI launches. Run inside scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "A private plugin-free server is required");
const root = await mkdtemp(join(tmpdir(), "corral-review-family-"));
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-133/live");
await mkdir(evidence, { recursive: true });
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
const env = { ...process.env };
for (const key of Object.keys(env)) {
  if (key.startsWith("CORRAL_") && key !== "CORRAL_TEST_HERDR_ROOT"
      || key.startsWith("HERDR_")) delete env[key];
}
env.CORRAL_HERDR_SESSION = "corral-test";
const commands = [];
const checks = [];
const observed = [];
let passed = false;
async function run(program, args, required = true, cwd = root) {
  const child = Bun.spawn([program, ...args], {
    cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ program, args, code, stdout, stderr });
  if (required) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { code, stdout, stderr };
}
const cli = (args, required = true) => run(binary, ["--root", root, ...args], required);
const json = async args => JSON.parse((await cli(args)).stdout);
const set = (key, value) => cli(["config", "set", key, value, "--workspace"]);
const check = (name, condition) => { assert(condition, name); checks.push(name); };
const plan = join(root, "plan.md");
const notes = join(root, "notes.md");
try {
  const repo = join(root, "app");
  await mkdir(repo);
  await run("git", ["init", "-q", "-b", "main", repo]);
  await run("git", ["commit", "-q", "--allow-empty", "-m", "Initial fixture"], true, repo);
  await cli(["init"]);
  await set("review.autopilot", "false");
  await set("review.gate", '["true"]');
  await set("resources.max_load_per_core", "1000000");
  await set("resources.min_mem_available", '"1B"');
  await set("resources.min_disk_free", '"1B"');
  await set("harnesses.fixture", '{command = "true {model_args}", flags = "generic", model_flag = "--model"}');
  for (const [name, vendor] of [["author", "openai"], ["other", "anthropic"]]) {
    await set(`profiles.${name}`, `{harness = "fixture", model = "${vendor}/fixture-${name}", vendor = "${vendor}"}`);
  }
  await set("defaults.profile", '"author"');
  await set("review.reviewer", '"author"');
  await writeFile(plan, "---\ngoal: Widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [true]\nowner_decisions: []\n---\n");
  await writeFile(notes, "The disposable CLI fixture exercised the review boundary.\n");
  for (const policy of ["default", "off", "on"]) {
    if (policy !== "default") await set("review.require_cross_family", String(policy === "on"));
    for (const target of ["plan", "implementation"]) {
      const id = (await cli(["new", `${target} ${policy}`, "-p", "app", "--ready"])).stdout.trim();
      if (target === "plan") {
        // Keep the planner alive on an exact file event, never terminal input or a sleep.
        const output = join(root, ".corral/state/plans", id, "D1");
        await mkdir(output, { recursive: true });
        await run("mkfifo", [join(output, "prompt.md.fifo")]);
        await set("harnesses.fixture.command", `"true {model_args}; read -r fixture_input < '{prompt_file}.fifo'"`);
        await cli(["dispatch", id, "--plan"]);
        await cli(["plan", "submit", id, plan]);
        await set("harnesses.fixture.command", '"true {model_args}"');
      } else {
        await cli(["dispatch", id]);
        const progress = await json(["run", "progress", id, "--json"]);
        const checkpoint = join(root, `${id}-checkpoint.json`);
        await writeFile(checkpoint, JSON.stringify({
          task: progress.task, run: progress.run, spec_revision: progress.spec_revision,
          head: progress.head, base: progress.base, tree: progress.tree,
          expected_revision: 0, progress_revision: 0, criterion_ids: [], evidence_refs: [],
          next_action: "Review the disposable candidate", decisions: [], waits: [], resources: [],
        }));
        await cli(["run", "checkpoint", id, "--file", checkpoint]);
        await cli(["done", id, "--summary", "Disposable CLI candidate.\nVerified: empty committed fixture\nNot verified: none"]);
        await cli(["gate", id, "--wait"]);
      }
      const route = await json(["route", id, "--json"]);
      check(`${target} ${policy}: route explains effective policy`, route.require_cross_family === (policy === "on"));
      const reviewArgs = target === "plan" ? ["plan", "review", id, "--json"] : ["review-run", id];
      const result = await cli(reviewArgs, policy !== "on");
      if (policy === "on") {
        check(`${target}: opt-in rejects same-family launch`,
          result.code !== 0 && result.stderr.includes("cross-family review is required"));
        await set("review.reviewer", '"other"');
        await cli(reviewArgs);
      }
      const task = await json(["show", id, "--json"]);
      const review = target === "plan" ? task.plans.at(-1).reviews.at(-1) : task.runs.at(-1).review;
      check(`${target} ${policy}: accepted review has its own pane`,
        Boolean(target === "plan" ? review.herdr?.pane_id : review.pane));
      observed.push({ target, policy, route, task });
      if (target === "plan") {
        await cli(["plan", "verdict", id, "approve", "--plan", "P1", "--attempt", review.id, "--notes-file", notes]);
        const reviewed = await json(["show", id, "--json"]);
        check(`${target} ${policy}: owner acceptance remains required`,
          reviewed.status === "needs_me" && !reviewed.plans.at(-1).approval);
      } else {
        await cli(["verdict", id, "approve", "--notes-file", notes]);
        const reviewed = await json(["show", id, "--json"]);
        check(`${target} ${policy}: gate and owner policy retained`,
          reviewed.runs.at(-1).gate.result === "pass" && reviewed.status !== "done");
      }
      await set("review.reviewer", '"author"');
    }
  }
  passed = true;
  console.log(`PASS: ${checks.length} real private-server CLI checks.`);
} finally {
  await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
  await writeFile(join(evidence, "observed.json"), JSON.stringify({ passed, checks, observed }, null, 2) + "\n");
  await writeFile(join(evidence, "events.jsonl"), await readFile(join(root, ".corral/events.jsonl"), "utf8").catch(() => ""));
  await rm(root, { recursive: true, force: true });
}
