// Real plan-review launch and persisted verdict, with an offline scripted provider and private herdr.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { EventFollower, corralBinary, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

const PLAN = `---
goal: Make the widget spin
non_goals: []
decisions: []
affected_paths: [widget.rs]
contracts: []
dependencies: []
acceptance:
  - scenario: widget spins
    expected: clockwise
verification: [cargo test]
owner_decisions: []
---
Implementation details.
`;

test("launched pi-native plan reviewer records a fixture verdict", async () => {
  if (!privateHerdr()) {
    console.log("plan review cross-lane: skipped (requires private test-herdr server)");
    return;
  }
  const workspace = mkdtempSync(join(privateRoot()!, "plan-review-"));
  let reviewerWorkspace: string | undefined;
  let events: EventFollower | undefined;
  try {
    const corral = corralBinary();
    const app = join(workspace, "app");
    mkdirSync(app);
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "fixture"],
      ["config", "user.email", "fixture@example.invalid"]]) ok(["git", "-C", app, ...args]);
    writeFileSync(join(app, "README.md"), "fixture\n");
    ok(["git", "-C", app, "add", "README.md"]);
    ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
    const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
    cli("init");
    const assets = buildFixtureInstall({ parent: workspace });
    for (const role of ["planner", "plan-reviewer"]) {
      rmSync(join(assets.dir, "src/roles", role), { recursive: true, force: true });
      cpSync(join(PACKAGE_ROOT, "src/roles", role), join(assets.dir, "src/roles", role), { recursive: true });
    }
    const authPath = join(assets.dir, "src/auth/index.ts");
    const auth = readFileSync(authPath, "utf8").replace("return fixtureTransport().send;", `fixtureTransport().script([
      { toolCalls: [{ callId: "fixture-verdict", name: "corral_plan_verdict", arguments: { verdict: "changes", notes: "Fixture review notes." } }] },
      { text: "Done." },
    ]); return fixtureTransport().send;`);
    writeFileSync(authPath, auth);
    const configPath = join(workspace, ".corral/corral.toml");
    writeFileSync(configPath, `${readFileSync(configPath, "utf8")}\n[agent]\nassets_dir = ${JSON.stringify(assets.dir)}\n[agent.capabilities.plan_reviewer]\nrole_enabled = true\n[profiles.pi]\nharness = "pi-native"\nmodel = "gpt-6.1-sol"\nvendor = "openai"\neffort = "high"\n`);
    cli("new", "Fixture plan", "-p", "app", "--ready");
    const plan = join(workspace, "plan.md");
    writeFileSync(plan, PLAN);
    cli("plan", "submit", "T-001", plan);
    events = new EventFollower(join(workspace, ".corral/events.jsonl"));
    const verdict = events.next((event) => event.task === "T-001" && event.kind === "plan_verdict");
    cli("plan", "review", "T-001", "--reviewer", "pi");
    const started = JSON.parse(cli("show", "T-001", "--json"));
    reviewerWorkspace = started.plans.at(-1).reviews.at(-1).herdr.workspace_id;
    await verdict;
    const task = JSON.parse(cli("show", "T-001", "--json"));
    const review = task.plans.at(-1).reviews.at(-1);
    expect(review.reviewer).toBe("pi");
    expect(review.verdict).toBe("changes");
    expect(review.notes).toContain("Fixture review notes.");
  } finally {
    events?.close();
    if (reviewerWorkspace) run(["herdr", "--session", "corral-test", "workspace", "close", reviewerWorkspace]);
    rmSync(workspace, { recursive: true, force: true });
  }
}, 300_000);
