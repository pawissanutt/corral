// Event-driven smoke test; run inside scripts/test-herdr.sh's private server.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { resolve } from "node:path";
import { PlanBoardCapture } from "./plan-board-capture.mjs";

assert(process.env.HOME?.startsWith("/tmp/"), "Use the private test-herdr environment");
const binary = resolve(process.argv[2] ?? "target/debug/corral");
const columns = Number(process.argv[3] ?? 80);
assert([80, 120].includes(columns));
const root = await mkdtemp("/tmp/corral-overview-");
const evidence = resolve(`.omo/evidence/task-overview/${columns}`);
const env = { ...process.env, CORRAL_ROOT: root };
delete env.CORRAL_PM;
async function run(program, args) {
  const child = Bun.spawn([program, ...args], {
    cwd: root, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  assert.equal(status, 0, `${program}: ${stderr}`);
  return { stdout, stderr };
}
const cli = (...args) => run(binary, ["--root", root, ...args]);
const capture = new PlanBoardCapture({ binary, root, evidence, columns, run });
const fullWidth = "Read the original task purpose without opening any historical reports.";
let pass = false;
try {
  await cli("init");
  await cli("config", "set", "dispatch.paused", "true");
  await cli("config", "set", "review.autopilot", "false");
  await cli("new", "Understand task planning results", "--idea", "--body",
    `# Context\nA task can have many runs.\n\n## Goal\nKeep the original purpose visible after retries.\n\n## Acceptance\n${fullWidth}`);
  await capture.start({ boardReady: s => s.includes("T-001") && s.includes("q leave") });
  await capture.stage("03-overview", "Task overview",
    s => s.includes("Keep the original purpose visible after retries.") && s.includes("Needs you"),
    () => capture.raw("\r"));
  assert(!capture.screen.includes("No live run."));
  await capture.stage("04-spec", fullWidth,
    s => s.includes(fullWidth),
    () => capture.click("[1] spec"));
  await capture.stage("05-live", "No live run",
    s => s.includes("No live run."), () => capture.raw("L"));
  await capture.stage("06-live-hidden", fullWidth,
    s => s.includes(fullWidth) && !s.includes("No live run."),
    () => capture.raw("L"));
  await capture.stage("07-overview-return", "Task overview",
    s => s.includes("Task overview"), () => capture.raw("0"));
  await capture.stage("08-history", "Timeline",
    s => s.includes("Timeline") && !s.includes("Task overview"),
    () => capture.click("[7] history"));
  await capture.stage("09-overview-final", "Task overview",
    s => s.includes("Keep the original purpose visible after retries."),
    () => capture.raw("0"));
  pass = true;
  console.log(`PASS task overview ${columns} columns`);
} finally {
  await capture.close(pass);
  await rm(root, { recursive: true, force: true });
}
