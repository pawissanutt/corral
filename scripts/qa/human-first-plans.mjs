// T-458 acceptance. Run inside scripts/test-herdr.sh's private plugin-free server.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { PlanBoardCapture } from "./plan-board-capture.mjs";

assert(process.env.CORRAL_TEST_HERDR_ROOT, "A private plugin-free herdr server is required");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-458/live");
const root = await mkdtemp(join(tmpdir(), "corral-human-plans-"));
const binary = join(root, "corral");
await copyFile(resolve(process.argv[2] ?? "target/debug/corral"), binary);
await mkdir(evidence, { recursive: true });
const env = { ...process.env, GIT_MASTER: "1" };
for (const key of ["CORRAL_ROOT", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const commands = [];
const checks = [];
const scenes = [];
let board;
let passed = false;
async function run(program, args, pass = true, cwd = root) {
  const child = Bun.spawn([program, ...args], { cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ program, args, code, stdout, stderr });
  if (pass) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { stdout, stderr, code };
}
const cli = (args, pass = true) => run(binary, ["--root", root, ...args], pass);
const json = async args => JSON.parse((await cli(args)).stdout);
const check = (name, value) => { assert(value, name); checks.push(name); };
const reviewBody = screen => screen.split("\n").slice(screen.split("\n").findIndex(line => line.includes("[i Implementation details]")) + 1).join("\n");
const words = text => text.replace(/[│╭╰─╮╯]/g, " ").replace(/\s+/g, " ");
const body = `## Implementation details
1. Add Widget.spin in widget.rs.
2. Add the --spin flag in cli.rs after the API exists.
3. Run cargo test -p widget.
`;
const legacy = `---
goal: Let operators rotate a widget once on demand
non_goals: [No scheduling changes, No notifications]
decisions: [Keep the existing widget contract; the CLI calls the same operation]
affected_paths: [widget.rs, cli.rs]
contracts: [Widget.spin]
dependencies:
  - task: widget-api
    write_scope: [widget.rs]
    depends_on: []
  - task: widget-cli
    write_scope: [cli.rs]
    depends_on: [widget-api]
acceptance:
  - scenario: Run widget --spin
    expected: One clockwise rotation
  - scenario: Wait for an existing scheduled rotation
    expected: It runs on its existing schedule
verification: [cargo test -p widget]
owner_decisions: []
---
${body}`;
const human = legacy.replace("goal:", `human_review:
  problem: Operators must wait for the scheduler to rotate a widget.
  changes: [Add an explicit spin operation through the API and CLI]
  retained: [Automatic rotations keep their current schedule]
goal:`);
const notification = human.replace("No scheduling changes, No notifications", "No scheduling changes")
  .replace("changes: [Add an explicit spin operation through the API and CLI]",
    "changes: [Add an explicit spin operation through the API and CLI, Notify the operator after each requested rotation]")
  .replace("verification:", "  - scenario: Run widget --spin with notifications enabled\n    expected: Receive one completion notification\nverification:");
const technical = human.replaceAll("widget.rs", "src/widget.rs").replaceAll("cli.rs", "src/cli.rs")
  .replaceAll("cargo test -p widget", "cargo test -p widget --lib");
async function submit(id, name, text, pass = true) {
  const path = join(evidence, `${name}.md`);
  await writeFile(path, text);
  return cli(["plan", "submit", id, path, "--json"], pass);
}
async function create(title) {
  return (await cli(["new", title, "-p", "app", "--ready", "--design"])).stdout.trim();
}
async function scrollFrame(name) {
  const read = (await board.rpc("pane.read", { pane_id: board.pane, source: "visible", strip_ansi: true })).read;
  const prior = reviewBody(read.text);
  let timer;
  const changed = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error("Overview scroll did not render")), 10000);
    board.onFrame = frame => {
      if (frame.type !== "terminal.frame") return;
      board.rpc("pane.read", { pane_id: board.pane, source: "visible", strip_ansi: true })
        .then(({ read }) => { if (reviewBody(read.text) !== prior) accept(read); }, reject);
    };
  });
  changed.catch(() => {});
  try {
    // The owned viewport's frame stream is subscribed before this single input.
    // Header-only frames do not satisfy the changed-content predicate.
    await board.raw("\x1b[<65;40;24M");
    board.screen = await board.capture(name, await changed);
  } finally {
    clearTimeout(timer);
    board.onFrame = undefined;
  }
}
async function capture(id, name, columns, humanFirst) {
  const directory = join(evidence, `${name}-${columns}`);
  board = new PlanBoardCapture({ binary, root, evidence: directory, columns, run });
  await board.start({ tour: false, boardMarker: "skip tour|q leave", boardReady: s => s.includes("skip tour") || s.includes("q leave") });
  if (board.screen.includes("skip tour")) {
    await board.stage("board", "q leave", s => s.includes("q leave"), () => board.raw("\x1b"), false);
  }
  // The attention header also contains task IDs. Select in the Ready column,
  // not by the first matching text occurrence, which may be a header action.
  await board.stage("selected-card", `▌ ${id} `, s => s.includes(`▌ ${id} `),
    () => board.raw("h".repeat(8) + "l" + "j".repeat(Number(id.slice(2)) - 1)), false);
  const open = () => board.raw("r");
  const marker = humanFirst ? "Implementation details" : "Revisions";
  await board.stage("01-overview", marker, s => s.includes(marker), open);
  if (humanFirst) {
    check(`${name} ${columns}: overview hides task checklist`, !board.screen.includes("widget-api"));
    if (name === "notifications" || name === "technical") {
      // Overlapping three-line scroll steps cover every intermediate revision
      // value, including a removed exclusion between the top and acceptance.
      const finalExpected = name === "notifications"
        ? "Receive one completion notification" : "It runs on its existing schedule";
      let sawCanonicalProblem = board.screen.includes("Operators must wait for the scheduler");
      let sawCanonicalChoices = board.screen.includes("Important choices");
      let page = 0;
      while (!(sawCanonicalProblem && sawCanonicalChoices && board.screen.includes("What counts as done")
        && words(reviewBody(board.screen)).includes(finalExpected))) {
        assert(++page <= 60, "The bounded fixture overview must reach its final acceptance scenario");
        await scrollFrame(`01-scroll-${String(page).padStart(2, "0")}`);
        sawCanonicalProblem ||= board.screen.includes("Operators must wait for the scheduler");
        sawCanonicalChoices ||= board.screen.includes("Important choices");
      }
      await board.stage("01-revision-top", "Changes since the previous revision",
        s => s.includes("Changes since the previous revision"), () => board.raw("\x1b[H"));
    }
    if (name === "after") {
      await board.stage("01a-included-and-retained", "Automatic rotations|explicit spin operation",
        s => s.includes("explicit spin operation"), () => board.raw("\x1b[6~"));
      if (columns === 80) {
        await board.stage("01b-boundaries", "No scheduling changes",
          s => s.includes("No scheduling changes") && s.includes("No notifications"),
          () => board.raw("\x1b[B".repeat(3)));
      }
    }
    await board.stage("02-acceptance", "One clockwise rotation|completion notification",
      s => s.includes("One clockwise rotation") || s.includes("completion notification"), () => board.raw("\x1b[6~".repeat(12)));
    await board.stage("03-details-keyboard", "Review overview", s => s.includes("Review overview"), () => board.raw("i"));
    await board.stage("04-overview-keyboard", "Implementation details", s => s.includes("Implementation details"), () => board.raw("o"));
    await board.stage("05-details-mouse", "Review overview", s => s.includes("Review overview"), () => board.click("[i Implementation details]"));
    await board.stage("06-overview-mouse", "Implementation details", s => s.includes("Implementation details"), () => board.click("[o Review overview]"));
    if (name === "notifications") {
      check(`${columns}: scope addition foregrounded`, board.screen.includes("What will change") && board.screen.includes("Notify the operator"));
    }
    if (name === "technical") {
      check(`${columns}: technical revision does not invent scope change`, board.screen.includes("unchanged"));
    }
  }
  await board.close(true);
  scenes.push({ name, columns, rows: 30, directory, scenes: board.result.scenes });
  board = undefined;
}
try {
  const repo = join(root, "app");
  await mkdir(repo);
  await run("git", ["init", "-q", "-b", "main", repo]);
  await cli(["init"]);
  // This driver inspects plans; it never grants reviewer or dispatch authority.
  await cli(["mode", "pause"]);
  const before = await create("Before: explicit widget rotation");
  await submit(before, "before", legacy);
  const after = await create("After: explicit widget rotation");
  await submit(after, "after", human);
  const revised = await create("Revised: widget notifications");
  await submit(revised, "notifications-p1", human);
  await submit(revised, "notifications-p2", notification);
  const technicalId = await create("Technical: widget paths");
  await submit(technicalId, "technical-p1", human);
  await submit(technicalId, "technical-p2", technical);
  const invalid = await create("Invalid overview");
  const prior = await json(["show", invalid, "--json"]);
  const denied = await submit(invalid, "missing-exclusions", human.replace("non_goals: [No scheduling changes, No notifications]", "non_goals: []"), false);
  check("missing explicit exclusions rejected before review", denied.code !== 0);
  const unchanged = await json(["show", invalid, "--json"]);
  assert.deepEqual(unchanged.plans, prior.plans);
  assert.deepEqual(unchanged.artifacts, prior.artifacts);
  check("invalid submission creates neither review nor artifact", true);
  await submit(invalid, "explicit-no-exclusions", human.replace("non_goals: [No scheduling changes, No notifications]", "non_goals: [No exclusions]"));
  check("explicit no-exclusions statement accepted", true);
  check("legacy plan accepted without rewriting", (await json(["show", before, "--json"])).plans[0].document.human_review === undefined);
  for (const columns of [120, 80]) {
    for (const [id, name, humanFirst] of [[before, "before", false], [after, "after", true], [revised, "notifications", true], [technicalId, "technical", true]]) {
      await capture(id, name, columns, humanFirst);
    }
  }
  passed = true;
} finally {
  if (board) await board.close(false);
  await rm(root, { recursive: true, force: true });
  await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify({
    status: passed ? "PASS" : "FAIL", checks, scenes,
    cleanup: "Owned boards, viewport controllers, subscriptions and disposable store removed; private server owned by caller",
  }, null, 2) + "\n");
}
console.log(`PASS: ${checks.length} human-first plan checks; ${scenes.length} terminal scenarios`);
