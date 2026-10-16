// Real CLI lifecycle checks. Run only inside scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { existsSync, watch } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { PlanBoardCapture } from "./plan-board-capture.mjs";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "Start a private plugin-free server with scripts/test-herdr.sh");
const sourceBinary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-100/plan-runs");
await mkdir(evidence, { recursive: true });
const root = await mkdtemp(join(tmpdir(), "corral-plan-runs-"));
// Cargo's shared target can replace the candidate while another run builds.
// Keep one immutable executable for this store's runtime identity.
const binary = join(root, "corral");
await copyFile(sourceBinary, binary);
const repo = join(root, "app");
await mkdir(repo);
const env = { ...process.env };
for (const key of ["CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const transcript = [];
const checks = [];
const columns = Number(process.argv[4] ?? 0);
assert(columns === 0 || columns === 80 || columns === 120);
const decisions = process.argv.includes("--tall") ? 12 : 0;
assert(!decisions || columns, "Tall-form checks require a live board");
let board;
let passed = false;

async function run(program, args, pass = true, cwd = root) {
  const child = Bun.spawn([program, ...args], { cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ program, args, cwd, code, stdout, stderr });
  if (pass) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { stdout, stderr, code };
}
const cli = (args, pass = true) => run(binary, ["--root", root, ...args], pass);
const json = async args => JSON.parse((await cli(args)).stdout);
const show = id => json(["show", id, "--json"]);
const set = (key, value) => cli(["config", "set", key, value]);
const check = (name, condition) => { assert(condition, name); checks.push({ name, pass: true }); };

async function marker(path, trigger) {
  await mkdir(dirname(path), { recursive: true });
  let timer;
  let watcher;
  const ready = new Promise((resolveReady, reject) => {
    const observe = () => { if (existsSync(path)) resolveReady(); };
    watcher = watch(dirname(path), observe);
    timer = setTimeout(() => reject(new Error(`Agent did not write ${path}`)), 60000);
  });
  ready.catch(() => {});
  try {
    await trigger();
    if (!existsSync(path)) await ready;
  } finally {
    clearTimeout(timer);
    watcher.close();
  }
}

const planText = `---
goal: Give the widget a reviewed API and command
non_goals: [No scheduler changes]
decisions: [Keep widget ownership explicit]
affected_paths: [widget.rs, cli.rs]
contracts: [Widget.spin]
dependencies:
  - task: widget-api
    write_scope: [widget.rs]
    depends_on: []
    acceptance: [{scenario: Call spin, expected: clockwise}]
  - task: widget-cli
    write_scope: [cli.rs]
    depends_on: [widget-api]
    acceptance: [{scenario: widget --spin, expected: calls spin once}]
acceptance: [{scenario: widget --spin, expected: clockwise}]
verification: [cargo test -p widget]
owner_decisions: [${Array.from({ length: decisions }, (_, i) => `Owner decision ${i + 1}`).join(", ")}]
---
## Alternatives considered
An implicit command was rejected in favor of an explicit operation.
## Risks
The command and API can drift; the command test verifies the boundary.
`;

try {
  await run("git", ["init", "-q", "-b", "main", repo]);
  await writeFile(join(repo, "protected.txt"), "original\n");
  await cli(["init"]);
  await set("harnesses.plan-fixture", `{command = "touch '{prompt_file}.seen'; printf changed > '{worktree}/protected.txt'; true {model_args}", flags = "generic", model_flag = "--model"}`);
  await set("harnesses.plan-review-fixture", `{command = "touch '{prompt_file}.seen'; true {model_args}", flags = "generic", model_flag = "--model"}`);
  for (const [name, model] of [["gpt-sol", "fixture-planner"], ["gpt-review", "fixture-reviewer"]]) {
    await set(`profiles.${name}`, `{harness = "${name === "gpt-sol" ? "plan-fixture" : "plan-review-fixture"}", model = "${model}", vendor = "openai"}`);
  }
  await set("defaults.profile", '"gpt-sol"');
  await set("review.reviewer", '"gpt-review"');
  await set("review.plan_accept", '"owner"');
  // Host pressure is not part of this isolated CLI scenario; dedicated admission tests cover it.
  await set("resources.max_load_per_core", "1000000");
  await set("resources.min_mem_available", '"1B"');
  await set("resources.min_disk_free", '"1B"');
  const id = (await cli(["new", "Design widget", "-p", "app", "--ready", "--design"])).stdout.trim();
  const output = join(root, ".corral/state/plans", id, "D1");
  await marker(join(output, "prompt.md.seen"), () => cli(["dispatch", id]));
  let task = await show(id);
  check("automatic design dispatch records only plan history", !task.runs?.length && task.plan_runs.length === 1);
  check("planner profile is routed gpt-sol", task.plan_runs[0].profile === "gpt-sol");
  check("no worktree or branch is created", !existsSync(join(repo, ".worktrees"))
    && !(await readdir(join(repo, ".git/refs/heads"))).includes("corral"));
  check("repository writes are denied by the live plan sandbox", await readFile(join(repo, "protected.txt"), "utf8") === "original\n");
  check("plan run rejects implementation completion", (await cli(["done", id, "--summary", "Not a code run"], false)).code !== 0);
  check("plan run rejects a code gate", (await cli(["gate", id], false)).code !== 0);
  if (columns) {
    board = new PlanBoardCapture({ binary, root, evidence: join(evidence, "board"), columns, run, decisions });
    await board.start();
  }
  const planFile = join(output, "plan.md");
  await writeFile(planFile, process.argv.includes("--crlf") ? planText.replaceAll("\n", "\r\n") : planText);
  let submitted;
  const submit = async () => { submitted = await json(["plan", "submit", id, planFile, "--json"]); await cli(["sync"]); };
  if (board) await board.stage("03-plan-review", "round 1",
    s => s.includes("◎ plan") && s.includes("round 1"), submit);
  else await submit();
  check("plan artifact is associated with D1 rather than an implementation", submitted.artifact.run === "D1");
  const reviewMarker = join(root, ".corral/prompts", `${id}-P1-PR1-plan-review.md.seen`);
  if (!existsSync(reviewMarker)) await marker(reviewMarker, () => cli(["sync"]));
  task = await show(id);
  const review = task.plans[0].reviews[0];
  check("autopilot launches the configured different-model reviewer", review.reviewer === "gpt-review"
    && task.plan_runs[0].model !== "fixture-reviewer");
  const notes = join(root, "review-notes.md");
  await writeFile(notes, "The scope and verification are sufficient.\n");
  const approveReview = () => cli(["plan", "verdict", id, "approve", "--plan", "P1", "--attempt", review.id, "--notes-file", notes]);
  if (board) await board.stage("04-awaiting-owner", "approval", s => s.includes("▲ 1") && s.includes("approval"), approveReview);
  else await approveReview();
  task = await show(id);
  check("reviewer approval waits for owner in Needs me", task.status === "needs_me"
    && task.plan_runs[0].state === "awaiting_owner" && !task.plans[0].approval);
  check("no children exist before owner approval", !task.children?.length
    && (await json(["list", "--all", "--json"])).length === 1);
  if (board) await board.inspectAndApprove(id);
  else await cli(["plan", "verdict", id, "approve", "--owner", "--plan", "P1", "--notes-file", notes]);
  task = await show(id);
  const [api, command] = await Promise.all(task.children.map(show));
  check("owner approval is recorded separately", task.plans[0].approval.by === "owner"
    && task.plans[0].reviews.at(-1).reviewer === "owner");
  check("multi-task plan completes the parent without implementation", task.status === "done" && !task.runs?.length);
  check("decomposition creates linked Backlog children", api.status === "backlog" && command.status === "backlog"
    && api.plan === `${id}@P1` && command.plan === `${id}@P1`);
  check("local dependency names become child task IDs", command.depends_on.includes(api.id));
  check("acceptance and write scope are copied", api.body.includes("clockwise")
    && command.body.includes("calls spin once") && api.write_scope[0] === "widget.rs");
  if (board) await board.openChild(api.id);
  if (process.argv.includes("--compact")) {
    assert(board, "Compact checks require a live board");
    await board.close(true);
    board = undefined;
    await set("board.card.density", '"compact"');
    board = new PlanBoardCapture({ binary, root, evidence: join(evidence, "compact"), columns: 360, run });
    await board.start({ parent: id });
  }
  await writeFile(join(evidence, "parent.json"), `${JSON.stringify(task, null, 2)}\n`);
  await writeFile(join(evidence, "children.json"), `${JSON.stringify([api, command], null, 2)}\n`);
  await writeFile(join(evidence, "events.jsonl"), await readFile(join(root, ".corral/events.jsonl")));
  console.log(`PASS: ${checks.length} real CLI checks; no implementation run, worktree, gate or landing.`);
  passed = true;
} finally {
  if (!passed) {
    const inventory = await run("herdr", ["--session", "corral-test", "pane", "list"], false);
    if (inventory.code === 0) {
      for (const pane of JSON.parse(inventory.stdout).result.panes ?? []) {
        await run("herdr", ["--session", "corral-test", "pane", "read", pane.pane_id, "--source", "visible"], false);
      }
    }
  }
  if (board) await board.close(passed);
  await writeFile(join(evidence, "commands.log"), transcript.map(x => JSON.stringify(x)).join("\n") + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify({ checks, root, cleanup: "private server helper owns all panes; fixture removed" }, null, 2) + "\n");
  await rm(root, { recursive: true, force: true });
}
