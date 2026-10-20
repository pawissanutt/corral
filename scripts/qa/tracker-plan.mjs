// Real tracker Plan readback and navigation. Use scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { appendFile, copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { PlanBoardCapture } from "./plan-board-capture.mjs";

assert(process.env.HOME?.startsWith("/tmp/corral-herdr-")
  && process.env.CORRAL_HERDR_SESSION === "corral-test", "Use the private plugin-free test server");
const columns = Number(process.argv[3] ?? 80);
assert([80, 160].includes(columns));
const evidence = resolve(process.argv[4] ?? `.omo/evidence/T-179/live-${columns}`);
await mkdir(evidence, { recursive: true });
const root = await mkdtemp("/tmp/corral-tracker-plan-");
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
const env = { ...process.env };
for (const key of ["CORRAL_ROOT", "CORRAL_PM", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const transcript = [];
async function run(program, args) {
  const child = Bun.spawn([program, ...args], { cwd: root, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ program, args, code, stdout, stderr });
  assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { stdout, stderr };
}
const cli = (...args) => run(binary, ["--root", root, ...args]);
const json = async (...args) => JSON.parse((await cli(...args)).stdout);
let capture;
let pass = false;
try {
  await mkdir(join(root, "app"));
  await run("git", ["init", "-q", "-b", "main", join(root, "app")]);
  await run("git", ["-C", join(root, "app"), "commit", "--allow-empty", "-qm", "Fixture initial base"]);
  await cli("init");
  await cli("config", "set", "dispatch.paused", "true");
  await cli("config", "set", "ui.columns", '["backlog", "ready"]');
  const planFile = join(root, "scope.md");
  await writeFile(planFile, `---
title: Ship the widget
project: app
approved_scope:
  goal: Ship a cohesive widget
  acceptance:
    - {criterion: C1, scenario: Run the widget, expected: Works end to end}
  delivery_target: {kind: integration, branch: widget-integration}
tasks:
  - {key: api, title: Implement widget API}
  - {key: ui, title: Connect widget screen, depends: [api]}
---
Tracker Plan fixture.
`);
  const imported = await json("import", planFile, "--origin", "agent", "--json");
  const id = imported.tracker.id;
  const child = imported.tasks[0].id;
  const before = await json("plan", "progress", id, "--json");
  const original = await json("show", id, "--json");
  assert.equal(before.counts.missing, 2);
  assert.equal(before.unmapped_criteria[0].criterion, "C1");
  await writeFile(join(evidence, "projection-before.json"), JSON.stringify(before, null, 2));
  await mkdir(join(root, ".corral/state"), { recursive: true });
  await writeFile(join(root, ".corral/state/tour-seen"), "");
  capture = new PlanBoardCapture({ binary, root, evidence, columns, run });
  await capture.start({ tour: false, boardMarker: "2 missing", boardReady: s => s.includes(id) && s.includes("missing") });
  await capture.stage("03-tracker-overview", "Task overview", s => s.includes("Task overview"),
    () => capture.raw("\r"));
  await capture.stage("04-plan-facts", "Final acceptance", s => s.includes("Design approval:")
    && s.includes("Delivery target:") && s.includes("Final acceptance:"),
    () => capture.raw("8"));
  assert(!capture.screen.includes("[Approve") && !capture.screen.includes("[Accept"));
  assert(capture.screen.includes("Plan integration branch"));
  await capture.stage("05-read-only", "Final acceptance", s => s.includes("Final acceptance:")
    && !s.includes("approve plan") && !s.includes("request plan changes"),
    () => capture.raw("c18"));
  await capture.stage("06-readiness-coverage", "MISSING COVERAGE", s => s.includes("MISSING COVERAGE")
    && s.includes(`${imported.tasks[1].id} -> ${child}`),
    () => capture.raw("\x1b[6~".repeat(8)));
  await capture.stage("07-child-selected", "Tracker Plan", s => s.includes("Tracker Plan") && s.includes(`▸ ${child}`),
    () => capture.raw("\x1b[H"));
  await capture.stage("08-child-detail", `parent ${id}`, s => s.includes(child) && s.includes(`parent ${id}`),
    () => capture.raw("\r"));
  await capture.stage("09-parent-backlink", "Final acceptance", s => s.includes(id) && s.includes("Final acceptance:"),
    () => capture.raw("P8"));
  await capture.close(true);
  capture = undefined;
  const after = await json("plan", "progress", id, "--json");
  assert.deepEqual(after, before);
  capture = new PlanBoardCapture({ binary, root, evidence: join(evidence, "restart"), columns, run });
  await capture.start({ tour: false, boardMarker: "2 missing", boardReady: s => s.includes(id) && s.includes("missing") });
  await capture.stage("03-restarted-plan", "Final acceptance", s => s.includes("Final acceptance:")
    && s.includes("2 missing"), () => capture.raw("\r8"));
  assert.deepEqual(await json("plan", "progress", id, "--json"), before);
  const task = await json("show", id, "--json");
  assert.deepEqual(task.approved_scope, original.approved_scope);
  assert.deepEqual(task.plans, original.plans);
  assert.equal(task.status, original.status);
  await writeFile(join(evidence, "projection-after.json"), JSON.stringify(after, null, 2));
  await capture.close(true);
  capture = undefined;
  await writeFile(join(root, "plain.md"), `---
title: Plain imported tracker
project: app
tasks: [{key: plain, title: Plain imported child}]
---
Tracking only.
`);
  const plain = await json("import", join(root, "plain.md"), "--origin", "agent", "--json");
  await cli("config", "set", "board.card.density", '"compact"');
  capture = new PlanBoardCapture({ binary, root, evidence: join(evidence, "compact-plain"), columns, run });
  await capture.start({ tour: false, boardMarker: "2 missing", boardReady: s => s.includes("0/1 done") && s.includes("0 accepted") && s.includes("2 missing") });
  await capture.stage("03-plain-overview", "Task overview",
    s => s.includes("Plain imported tracker") && s.includes("Task overview"),
    async () => { await capture.click(plain.tracker.id); await capture.click(plain.tracker.id); });
  await capture.stage("04-plain-children", "Imported plan tracker",
    s => s.includes("Imported plan tracker") && s.includes(`▸ ${plain.tasks[0].id}`)
      && !s.includes("Final acceptance:"),
    () => capture.raw("8"));
  await capture.stage("05-plain-child", `parent ${plain.tracker.id}`,
    s => s.includes(`parent ${plain.tracker.id} [P]`) && s.includes(plain.tasks[0].id)
      && s.includes("Overview") && s.includes("Plain imported child"), () => capture.raw("\r"));
  await capture.stage("06-plain-backlink", "Imported plan tracker",
    s => s.includes(plain.tracker.id) && s.includes("Imported plan tracker"), () => capture.raw("P8"));
  await capture.close(true);
  capture = undefined;

  // Synthetic accepted work in this disposable store; the projection still checks real Git ancestry.
  const repo = join(root, "app");
  const sha = (await run("git", ["-C", repo, "rev-parse", "main"])).stdout.trim();
  await cli("config", "set", "projects.app.path", JSON.stringify(repo));
  const externalId = (await cli("new", "Accepted external prerequisite", "-p", "app", "--origin", "agent")).stdout.trim();
  const external = (await json("list", "--all", "--json")).find(task => task.id === externalId);
  assert(external);
  external.status = "done";
  external.runs = [{
    id: "R1", agent: "fixture", worktree: join(root, "accepted-worktree"), branch: "external", base: "main",
    base_sha: sha, started: "2026-10-20T00:00:00Z", ended: "2026-10-20T00:00:00Z", outcome: "done",
  }];
  const taskFile = (await readdir(join(root, ".corral/tasks"))).find(name => name.startsWith(`${externalId}-`));
  assert(taskFile);
  await writeFile(join(root, ".corral/tasks", taskFile), `---\n${JSON.stringify(external, null, 2)}\n---\nSynthetic accepted prerequisite.\n`);
  await appendFile(join(root, ".corral/events.jsonl"), JSON.stringify({
    ts: "2026-10-20T00:00:00Z", task: externalId, kind: "approved",
    detail: { run: "R1", sha, strategy: "fast_forward", base: "main" },
  }) + "\n");
  await writeFile(join(root, "initial-target.md"), `---
title: New integration readiness
project: app
approved_scope:
  goal: Build on accepted external work
  acceptance: [{scenario: Launch consumer, expected: Includes accepted prerequisite}]
  delivery_target: {kind: integration, branch: new-widget-integration}
tasks:
  - {key: consumer, title: Consumer task, depends: [${externalId}]}
---
New target readiness without execution authorization.
`);
  const newPlan = await json("import", join(root, "initial-target.md"), "--origin", "agent", "--json");
  const readiness = await json("plan", "progress", newPlan.tracker.id, "--json");
  const prerequisite = readiness.work[0].prerequisites[0];
  assert(prerequisite.accepted && prerequisite.satisfied);
  assert.equal(prerequisite.proposed_initial_base, sha);
  assert.equal(prerequisite.target_head, null);
  const newTargetTask = await json("show", newPlan.tracker.id, "--json");
  await writeFile(join(evidence, "new-target-before.json"), JSON.stringify(readiness, null, 2));
  await cli("config", "set", "ui.columns", '["backlog"]');
  const readyFrame = screen => {
    const text = screen.replaceAll("│", " ").replace(/\s+/g, " ");
    return text.includes(`proposed initial base ${sha}`)
      && text.includes(externalId) && text.includes("satisfied") && !text.includes("head unknown");
  };
  const readyMarker = `│\\s+(?:base\\s+)?${sha}`;
  capture = new PlanBoardCapture({ binary, root, evidence: join(evidence, "initial-target"), columns, run });
  await capture.start({ tour: false, boardMarker: "New integration readiness", boardReady: s => s.includes(newPlan.tracker.id) });
  await capture.stage("03-new-target-overview", "Task overview",
    s => s.includes("Task overview") && s.includes("New integration readiness"),
    async () => { await capture.click(newPlan.tracker.id); await capture.click(newPlan.tracker.id); });
  await capture.stage("04-new-target-facts", "Final acceptance",
    s => s.includes("Final acceptance:") && s.includes("new-widget-integration"),
    () => capture.raw("8"));
  await capture.stage("05-proposed-initial-base", readyMarker, readyFrame,
    () => capture.raw("\x1b[6~".repeat(8)));
  await capture.close(true);
  capture = undefined;
  capture = new PlanBoardCapture({ binary, root, evidence: join(evidence, "initial-target-restart"), columns, run });
  await capture.start({ tour: false, boardMarker: "New integration readiness", boardReady: s => s.includes(newPlan.tracker.id) });
  await capture.stage("03-restarted-target-overview", "Task overview",
    s => s.includes("Task overview") && s.includes("New integration readiness"),
    async () => { await capture.click(newPlan.tracker.id); await capture.click(newPlan.tracker.id); });
  await capture.stage("04-restarted-target-facts", "Final acceptance",
    s => s.includes("Final acceptance:") && s.includes("new-widget-integration"),
    () => capture.raw("8"));
  await capture.stage("05-restarted-initial-base", readyMarker, readyFrame,
    () => capture.raw("\x1b[6~".repeat(8)));
  const restarted = await json("plan", "progress", newPlan.tracker.id, "--json");
  assert.deepEqual(restarted, readiness);
  assert.deepEqual(await json("show", newPlan.tracker.id, "--json"), newTargetTask);
  const refs = (await run("git", ["-C", repo, "for-each-ref", "--format=%(refname)", "refs/heads/new-widget-integration"])).stdout;
  assert.equal(refs.trim(), "", "Read-only readiness must not initialize a target");
  await writeFile(join(evidence, "new-target-after.json"), JSON.stringify(restarted, null, 2));
  pass = true;
  console.log(`PASS: tracker Plan ${columns} columns; shared JSON and proposed initial-base SHA stable across restart; no owner approval or target initialization; compact/plain child/backlink navigation`);
} finally {
  if (capture) await capture.close(pass);
  await writeFile(join(evidence, "commands.log"), transcript.map(x => JSON.stringify(x)).join("\n") + "\n");
  await rm(root, { recursive: true, force: true });
}
