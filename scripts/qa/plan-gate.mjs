// Real CLI acceptance against the isolated corral-test terminal engine.
import assert from "node:assert/strict";
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, watch, writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(".omo/evidence/T-059");
mkdirSync(evidence, { recursive: true });
assert(process.env.TMPDIR, "Set TMPDIR to the task-owned run root");
const root = mkdtempSync(join(process.env.TMPDIR, "corral-plan-qa-"));
const env = { ...process.env, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of ["CORRAL_ROOT", "HERDR_BIN_PATH"]) delete env[key];
const transcript = [];
const results = [];
const owned = new Set();
const planText = `---
goal: Give the widget an explicit spin operation
non_goals: [No scheduler changes]
decisions: [Keep the existing widget contract]
affected_paths: [widget.rs]
contracts: [Widget.spin]
dependencies: []
acceptance:
  - criterion: C1
    scenario: widget --spin
    expected: One clockwise rotation
verification: [cargo test -p widget]
owner_decisions: []
---
Implement a single spin operation.
`;
const planFile = join(root, "plan.md");
const notesFile = join(root, "notes.md");

function invoke(program, args, pass = true, cwd = root) {
  const out = spawnSync(program, args, { cwd, env, encoding: "utf8", input: "", maxBuffer: 20_000_000 });
  transcript.push({ command: [program, ...args], cwd, status: out.status, stdout: out.stdout, stderr: out.stderr, error: out.error?.message });
  if (pass) assert.equal(out.status, 0, `${program} ${args.join(" ")}: ${out.stderr}`);
  return out;
}
function cli(args, pass = true) {
  return invoke(binary, ["--root", root, ...args], pass);
}
function json(args) { return JSON.parse(cli(args).stdout); }
function herdr(args) {
  const value = JSON.parse(invoke("herdr", ["--session", "corral-test", ...args]).stdout);
  assert(!value.error, JSON.stringify(value.error));
  return value.result;
}
function task(id = "T-001") { return json(["show", id, "--json"]); }
function plans(id = "T-001") { return json(["plan", "show", id, "--json"]).plans; }
function own(task) {
  for (const run of task.runs ?? []) if (run.herdr) owned.add(run.herdr.workspace_id);
  for (const plan of task.plans ?? []) for (const review of plan.reviews ?? []) {
    if (review.reviewer !== "pm" && review.herdr) owned.add(review.herdr.workspace_id);
  }
}
function snapshot(id = "T-001") {
  const t = task(id);
  own(t);
  return {
    runs: t.runs ?? [], queued: t.queued ?? false, status: t.status,
    worktrees: existsSync(join(root, "app/.worktrees")) ? readdirSync(join(root, "app/.worktrees")).sort() : [],
    events: readFileSync(join(root, ".corral/events.jsonl"), "utf8"),
    panes: herdr(["pane", "list"]).panes ?? [],
  };
}
function denied(name, expected, id = "T-001", extra = []) {
  const before = snapshot(id);
  const out = cli(["dispatch", id, "--profile", "fake-worker", ...extra], false);
  assert.notEqual(out.status, 0, name);
  assert.match(out.stderr, expected);
  const after = snapshot(id);
  for (const key of ["runs", "queued", "status", "worktrees", "events"]) assert.deepEqual(after[key], before[key], `${name}: ${key}`);
  const local = (s) => s.panes.filter((p) => JSON.stringify(p).includes(root))
    .map((p) => ({ pane_id: p.pane_id, workspace_id: p.workspace_id }))
    .sort((a, b) => a.pane_id.localeCompare(b.pane_id));
  assert.deepEqual(local(after), local(before), `${name}: terminal side effects`);
  results.push({ name, pass: true, invocation: transcript.at(-3)?.command, error: out.stderr, before, after });
}
function submit(id = "T-001") {
  return json(["plan", "submit", id, planFile, "--json"]);
}

// Subscribe before launch. No polling, fixed delay or timing-based completion.
async function reviewer(id = "T-001", profile = "fake-reviewer") {
  const history = plans(id);
  const p = history.at(-1);
  const attempt = `PR${(p.reviews ?? []).length + 1}`;
  const marker = join(root, ".corral/prompts", `${id}-${p.id}-${attempt}-plan-review.md.seen`);
  let timer;
  let watcher;
  const seen = new Promise((resolveSeen, reject) => {
    watcher = watch(dirname(marker), (_event, filename) => {
      if (String(filename) === basename(marker) && existsSync(marker)) {
        resolveSeen(readFileSync(marker, "utf8").trim());
      }
    });
    timer = setTimeout(() => reject(new Error(`reviewer did not execute prompt ${marker}`)), 10_000);
  });
  try {
    const record = json(["plan", "review", id, "--reviewer", profile, "--json"]);
    own(task(id));
    const promptPath = await seen;
    assert.equal(promptPath, marker.slice(0, -5));
    writeFileSync(join(evidence, `${id}-${p.id}-${attempt}-review-prompt.md`), readFileSync(promptPath));
    return { plan: p.id, attempt: record.id };
  } finally {
    clearTimeout(timer);
    watcher.close();
  }
}
function verdict(identity, value = "approve", id = "T-001") {
  return json(["plan", "verdict", id, value, "--plan", identity.plan,
    "--attempt", identity.attempt, "--notes-file", notesFile, "--json"]);
}
function fresh(id = "T-001") {
  writeFileSync(planFile, planText);
  return submit(id);
}
async function approved(id = "T-001") {
  const p = fresh(id);
  const identity = await reviewer(id);
  assert.equal(verdict(identity, "approve", id).verdict, "approve");
  return p;
}
function newTask(title) {
  return cli(["new", title, "-p", "app", "--ready", "--body", "The widget spins clockwise."]).stdout.trim();
}

let failed;
try {
  herdr(["workspace", "list"]);
  mkdirSync(join(root, "app"));
  for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "corral plan QA"],
    ["config", "user.email", "corral@test.invalid"], ["config", "commit.gpgsign", "false"]]) {
    invoke("git", args, true, join(root, "app"));
  }
  writeFileSync(join(root, "app/initial.txt"), "initial\n");
  invoke("git", ["add", "-A"], true, join(root, "app"));
  invoke("git", ["commit", "-qm", "initial"], true, join(root, "app"));
  cli(["init"]);
  cli(["config", "set", "agents.fake-worker.command", '"true"']);
  for (const name of ["fake-reviewer", "unauthorized-reviewer"]) {
    cli(["config", "set", `agents.${name}.command`,
      `printf '%s\\n' '{prompt_file}' > '{prompt_file}.seen.tmp' && mv '{prompt_file}.seen.tmp' '{prompt_file}.seen'`]);
  }
  cli(["config", "set", "review.reviewer", "fake-reviewer"]);
  cli(["config", "set", "review.accept", "owner"]);
  cli(["config", "set", "review.autopilot", "false"]);
  cli(["config", "set", "limits.running", "1"]);
  assert.equal(newTask("Plan gated widget"), "T-001");
  assert.equal(newTask("Default direct widget"), "T-002");
  cli(["dispatch", "T-002", "--profile", "fake-worker"]);
  const off = task("T-002");
  own(off);
  assert.equal(off.status, "running");
  assert.equal(off.runs.length, 1);
  const offPrompt = readFileSync(join(root, ".corral/prompts/T-002-R1.md"), "utf8");
  const offBindings = [...offPrompt.matchAll(/```json\n([\s\S]*?)\n```/g)].map((m) => JSON.parse(m[1]));
  assert(!offBindings.some((binding) => binding.target === "plan"));
  results.push({ name: "default_off_direct_dispatch", pass: true, task: off });
  cli(["move", "T-002", "dropped", "--force"]);
  cli(["plan", "policy", "T-001", "required"]);
  writeFileSync(notesFile, "Reviewed exact plan and task; no unresolved decisions.\n");
  denied("missing", /plan required/, "T-001", ["--queue"]);
  writeFileSync(planFile, planText);
  submit();
  denied("submitted_pending", /pending/);
  const firstReview = await reviewer();
  assert.equal((task().runs ?? []).length, 0);
  denied("review_pending", /pending/);
  assert.equal(verdict(firstReview, "changes").verdict, "changes");
  denied("rejected", /rejected/);

  await approved();
  writeFileSync(planFile, `${planText}Changed decisions.\n`);
  denied("edited_plan", /stale plan.*bytes changed/);
  const edited = fresh();
  const staleReview = await reviewer();
  submit();
  const old = cli(["plan", "verdict", "T-001", "approve", "--plan", edited.id,
    "--attempt", staleReview.attempt, "--notes-file", notesFile, "--json"], false);
  assert.notEqual(old.status, 0);
  assert.match(old.stderr, /replaced/);
  assert.equal((plans().at(-1).reviews ?? []).length, 0);
  results.push({ name: "stale_replacement_callback", pass: true, stderr: old.stderr, history: plans() });

  await approved();
  const spec = task();
  // The task file is the canonical spec-edit surface. This is isolated QA data only.
  writeFileSync(spec.path, readFileSync(spec.path, "utf8").replace("The widget spins clockwise.", "The widget spins counterclockwise."));
  denied("changed_task_criteria", /stale plan.*spec or criteria changed/);

  fresh();
  const unauthorized = await reviewer("T-001", "unauthorized-reviewer");
  assert.equal(verdict(unauthorized).verdict, "escalate");
  denied("unauthorized_reviewer", /escalated/);
  writeFileSync(planFile, planText.replace("owner_decisions: []", "owner_decisions: [Choose storage]"));
  submit();
  const unresolved = await reviewer();
  assert.equal(verdict(unresolved).verdict, "escalate");
  denied("unresolved_owner_decision", /escalated/);

  const final = await approved();
  const ready = task();
  assert.equal(ready.status, "ready");
  assert.equal((ready.runs ?? []).length, 0);
  assert.equal(ready.review, undefined);
  assert.equal(json(["plan", "show", "T-001", "--json"]).required, true);
  assert.equal(cli(["config", "get", "review.accept"]).stdout.trim(), "owner");
  assert.notEqual(cli(["verdict", "T-001", "approve", "--notes-file", notesFile], false).status, 0);
  assert.notEqual(cli(["approve", "T-001"], false).status, 0);
  assert.equal(json(["list", "--all", "--json"]).length, 2);
  results.push({ name: "approval_is_plan_only", pass: true, task: ready });

  assert.equal(newTask("Existing WIP widget"), "T-003");
  cli(["dispatch", "T-003", "--profile", "fake-worker"]);
  own(task("T-003"));
  denied("approved_still_obeys_wip", /no free slot/);
  cli(["move", "T-003", "dropped", "--force"]);
  cli(["dispatch", "T-001", "--profile", "fake-worker"]);
  const running = task();
  own(running);
  assert.equal(running.status, "running");
  assert.equal(running.runs.length, 1);
  assert(existsSync(running.runs[0].worktree));
  const workerPrompt = readFileSync(join(root, ".corral/prompts/T-001-R1.md"), "utf8");
  const bindings = [...workerPrompt.matchAll(/```json\n([\s\S]*?)\n```/g)].map((m) => JSON.parse(m[1]));
  const binding = bindings.find((b) => b.target === "plan");
  assert.equal(binding.sha256, final.artifact.sha256);
  assert.equal(binding.spec_revision, final.spec_revision);
  assert.equal(binding.path, final.artifact.path);
  assert.equal(binding.plan, final.id);
  writeFileSync(join(evidence, "prompt.md"), workerPrompt);
  const events = readFileSync(join(root, ".corral/events.jsonl"), "utf8");
  const ordered = events.trim().split("\n").map(JSON.parse).filter((e) => e.task === "T-001");
  const dispatchEvent = ordered.findLast((e) => e.kind === "dispatched");
  assert.equal(dispatchEvent.detail.approved_plan.sha256, final.artifact.sha256);
  const approvalIndex = ordered.findLastIndex((e) => e.kind === "plan_verdict");
  assert(approvalIndex < ordered.findLastIndex((e) => e.kind === "dispatched"));
  assert(ordered.filter((e) => e.kind.startsWith("plan_") && e.kind !== "plan_policy").every((e) => e.detail.target === "plan"));
  writeFileSync(join(evidence, "events.jsonl"), events);
  writeFileSync(join(evidence, "plan-dispatch.json"), JSON.stringify({ plan: final, task: running, binding }, null, 2));
  results.push({ name: "explicit_dispatch_exact_plan_prompt_and_event", pass: true, binding });
  assert.equal(newTask("Planning-only cleanup widget"), "T-004");
  fresh("T-004");
  await reviewer("T-004");
  const planning = task("T-004");
  const planningWorkspace = planning.plans.at(-1).reviews.at(-1).herdr.workspace_id;
  assert.equal((planning.runs ?? []).length, 0);
  cli(["move", "T-004", "dropped", "--force"]);
  cli(["sync"]);
  assert(!herdr(["workspace", "list"]).workspaces.some((w) => w.workspace_id === planningWorkspace));
  assert(!herdr(["pane", "list"]).panes.some((p) => p.workspace_id === planningWorkspace));
  results.push({ name: "dropping_planning_only_task_closes_reviewer", pass: true, planningWorkspace });
} catch (error) {
  failed = error;
  results.push({ name: "failure", pass: false, error: String(error), stack: error.stack });
} finally {
  if (existsSync(join(root, ".corral/corral.toml"))) {
    for (const t of json(["list", "--all", "--json"])) own(t);
  }
  let cleanupError;
  try {
    let workspaces = herdr(["workspace", "list"]).workspaces;
    for (const workspace of workspaces) {
      if (owned.has(workspace.workspace_id)) herdr(["workspace", "close", workspace.workspace_id]);
    }
    workspaces = herdr(["workspace", "list"]).workspaces;
    assert(workspaces.every((workspace) => !owned.has(workspace.workspace_id)));
    const remainingPanes = herdr(["pane", "list"]).panes ?? [];
    assert(remainingPanes.every((pane) => !owned.has(pane.workspace_id)));
    rmSync(root, { recursive: true });
    assert(!existsSync(root));
    writeFileSync(join(evidence, "cleanup.json"), JSON.stringify({
      pass: true, rootRemoved: root, ownedWorkspacesClosed: [...owned],
      remainingOwnedWorkspaces: 0, remainingOwnedPanes: 0,
    }, null, 2));
  } catch (error) {
    cleanupError = error;
    writeFileSync(join(evidence, "cleanup.json"), JSON.stringify({ pass: false, root, error: String(error) }, null, 2));
  }
  writeFileSync(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2));
  writeFileSync(join(evidence, "qa.json"), JSON.stringify({
    binary, head: execFileSync("git", ["rev-parse", "HEAD"], { env, encoding: "utf8", input: "" }).trim(),
    pass: !failed && !cleanupError, results,
  }, null, 2));
  if (cleanupError && !failed) failed = cleanupError;
}
if (failed) throw failed;
console.log(`PASS ${results.length} real CLI scenarios; ${evidence}; owned panes/workspaces/temp repository removed`);
