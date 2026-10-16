// T-105 real CLI checks, only inside scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { existsSync, readFileSync, watch } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "Start a private plugin-free test server");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-105/live");
await mkdir(evidence, { recursive: true });
const root = await mkdtemp(join(tmpdir(), "corral-planner-lifecycle-"));
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
const env = { ...process.env };
for (const key of ["CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const commands = [];
const checks = [];
const observed = {};
let passed = false;
async function run(program, args, required = true) {
  const child = Bun.spawn([program, ...args], { cwd: root, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ program, args, stdout, stderr, code });
  if (required) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { stdout, stderr, code };
}
const cli = (args, required = true) => run(binary, ["--root", root, ...args], required);
const json = async args => JSON.parse((await cli(args)).stdout);
const show = id => json(["show", id, "--json"]);
const set = (key, value) => cli(["config", "set", key, value]);
const check = (name, condition) => { assert(condition, name); checks.push(name); };
async function fileEvent(path, accept, trigger) {
  await mkdir(dirname(path), { recursive: true });
  let timer;
  let watcher;
  const ready = new Promise((resolveReady, reject) => {
    const observe = () => {
      if (existsSync(path) && accept(readFileSync(path, "utf8"))) resolveReady();
    };
    watcher = watch(dirname(path), observe);
    timer = setTimeout(() => reject(new Error(`No expected file event: ${path}`)), 10000);
    observe();
  });
  ready.catch(() => {});
  try { await trigger(); await ready; }
  finally { clearTimeout(timer); watcher.close(); }
}
const plan = revision => `---
goal: Widget ${revision}
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
  await mkdir(join(root, "app"));
  await run("git", ["init", "-q", "-b", "main", join(root, "app")]);
  await writeFile(join(root, "app", "protected.txt"), "original");
  await cli(["init"]);
  await set("review.autopilot", "false");
  await set("sync.startup_grace_secs", "1");
  await set("resources.max_load_per_core", "1000000");
  await set("resources.min_mem_available", '"1B"');
  await set("resources.min_disk_free", '"1B"');
  const fixture = join(root, "planner.mjs");
  await writeFile(fixture, `
import { appendFileSync, openSync, readSync, writeFileSync } from "node:fs";
const prompt = process.argv[2];
const tty = openSync("/dev/tty", "r");
writeFileSync(prompt + ".ready", "bun-started");
console.log("Planner fixture ready");
const byte = Buffer.alloc(1);
let text = "";
while (readSync(tty, byte) > 0) {
  text += byte.toString();
  if (text.endsWith("\\n")) {
    appendFileSync(prompt + ".reply", text);
    console.log("received: " + text.trim());
    if (text.includes("exit-planner")) process.exit(17);
    text = "";
  }
}
`);
  const harness = `bun '${fixture}' '{prompt_file}'`;
  await set("agents.planner", `{command = ${JSON.stringify(harness)}}`);
  await set("defaults.profile", '"planner"');
  const create = async title => (await cli(["new", title, "-p", "app", "--ready", "--design"])).stdout.trim();
  const start = async id => {
    const prompt = join(root, ".corral/state/plans", id, "D1/prompt.md");
    await fileEvent(`${prompt}.ready`, x => x === "bun-started", () => cli(["dispatch", id, "--plan"]));
    return prompt;
  };

  const live = await create("Live Bun planner");
  const prompt = await start(live);
  const running = await show(live);
  check("real Bun harness starts with usable dev tty and proc", running.status === "running" && running.plan_runs[0].state === "planning");
  await fileEvent(`${prompt}.reply`, x => x.includes("hello-planner"), () => cli(["reply", live, "hello-planner"]));
  check("reply reaches live planner through real herdr", (await readFile(`${prompt}.reply`, "utf8")).includes("hello-planner"));
  await fileEvent(join(dirname(prompt), "exit-0.status"), x => x.trim() === "17",
    () => cli(["reply", live, "exit-planner"]));
  await cli(["sync"]);
  observed.exit = await show(live);
  observed.exitLog = await readFile(join(dirname(prompt), "failure-0.log"), "utf8");
  check("ordinary sync detects exit without autopilot", observed.exit.status === "needs_me" && observed.exit.plan_runs[0].state === "failed");
  check("late exit creates failure log artifact", observed.exit.artifacts.some(a => a.run === "D1" && a.kind === "log"));
  check("dead planner reply gives plan specific error", (await cli(["reply", live, "hello"], false)).stderr.includes("planner"));
  await cli(["plan", "cancel", live]);
  check("failed planner cancellation preserves history", (await show(live)).plan_runs[0].state === "cancelled");

  const shell = await create("Legacy shell planner");
  await start(shell);
  const shellTask = await show(shell);
  // This check exercises startup grace itself, so await its exact clock boundary.
  const graceElapsed = new Promise(resolveReady => setTimeout(resolveReady,
    Math.max(0, Date.parse(shellTask.plan_runs[0].started) + 1001 - Date.now())));
  await run("herdr", ["--session", "corral-test", "pane", "send-keys", shellTask.plan_runs[0].herdr.pane_id, "ctrl+c"]);
  // Subscribe to the shell's next output before asking it for a deterministic marker.
  // A pane run on the terminal queues behind Ctrl-C, proving the foreground group ended.
  const shellMarker = join(root, "shell-returned");
  await fileEvent(shellMarker, x => x === "shell", () => run("herdr", [
    "--session", "corral-test", "pane", "run", shellTask.plan_runs[0].herdr.pane_id,
    `printf shell > '${shellMarker}'`,
  ]));
  await graceElapsed;
  await cli(["sync"]);
  observed.shell = await show(shell);
  check("ordinary sync detects shell-only legacy planner", observed.shell.plan_runs[0].state === "failed");
  await cli(["plan", "cancel", shell]);

  await set("agents.crash", '{command = "printf \\"panic(main thread): abort() called\\\\n\\" >&2; exit 134"}');
  const crash = await create("Immediate crash");
  const crashed = await cli(["dispatch", crash, "--plan", "--profile", "crash"], false);
  observed.crash = await show(crash);
  observed.crashLog = await readFile(join(root, ".corral/state/plans", crash, "D1/failure-0.log"), "utf8");
  check("launch smoke rejects immediate crash", crashed.code !== 0 && observed.crash.status === "needs_me" && observed.crash.plan_runs[0].state === "failed");
  check("crash reason includes exit status", observed.crash.plan_runs[0].pending_reply.includes("134"));
  check("crash artifact retains the actual panic output", observed.crashLog.includes("abort()") && observed.crashLog.includes("134"));
  await cli(["plan", "cancel", crash]);

  const recovery = await create("T-048 stale D1 recovery fixture");
  for (const revision of ["P1", "P2", "P3"]) {
    const path = join(root, `${revision}.md`);
    await writeFile(path, plan(revision));
    await cli(["plan", "submit", recovery, path]);
  }
  await set("harnesses.review-fixture", '{command = "true {model_args}", flags = "generic", model_flag = "--model"}');
  await set("profiles.gpt-review", '{harness = "review-fixture", model = "fixture-reviewer", vendor = "openai"}');
  await set("review.reviewer", '"gpt-review"');
  const review = await json(["plan", "review", recovery, "--json"]);
  const notes = join(root, "notes.md");
  await writeFile(notes, "The plan is sufficient; owner acceptance remains separate.");
  await cli(["plan", "verdict", recovery, "approve", "--plan", "P3", "--attempt", review.id, "--notes-file", notes]);
  const blocked = await cli(["dispatch", recovery], false);
  check("pending P3 refuses implicit manual replanning with reason", blocked.code !== 0 && blocked.stderr.includes("P3") && blocked.stderr.includes("owner"));
  await start(recovery);
  const before = await show(recovery);
  await cli(["plan", "cancel", recovery]);
  observed.recovery = await show(recovery);
  assert.deepEqual(observed.recovery.plans, before.plans);
  check("cancel stale D1 leaves all three revisions and P3 awaiting owner", observed.recovery.plans.length === 3
    && observed.recovery.plans.at(-1).id === "P3" && !observed.recovery.plans.at(-1).approval
    && observed.recovery.plan_runs[0].state === "cancelled" && observed.recovery.status === "needs_me");
  await cli(["plan", "cancel", recovery]);
  check("cancellation is idempotent", (await show(recovery)).plan_runs.length === 1);
  check("every planner slot is released", (await json(["pm", "brief", "--json"])).slots.used === 0);
  check("source remains unchanged", await readFile(join(root, "app/protected.txt"), "utf8") === "original");
  passed = true;
  console.log(`PASS: ${checks.length} real planner lifecycle checks in private plugin-free herdr.`);
} finally {
  if (!passed) {
    const inventory = await run("herdr", ["--session", "corral-test", "pane", "list"], false);
    if (inventory.code === 0) {
      for (const pane of JSON.parse(inventory.stdout).result.panes ?? []) {
        await run("herdr", ["--session", "corral-test", "pane", "read", pane.pane_id, "--source", "visible"], false);
        await run("herdr", ["--session", "corral-test", "pane", "process-info", "--pane", pane.pane_id], false);
      }
    }
  }
  await writeFile(join(evidence, "commands.log"), commands.map(c => JSON.stringify(c)).join("\n") + "\n");
  await writeFile(join(evidence, "events.jsonl"), await readFile(join(root, ".corral/events.jsonl"), "utf8").catch(() => ""));
  await writeFile(join(evidence, "result.json"), JSON.stringify({
    status: passed ? "PASS" : "FAIL", checks, observed,
    cleanup: "private server helper terminates all owned panes; temporary fixture removed",
  }, null, 2) + "\n");
  await rm(root, { recursive: true, force: true });
}
