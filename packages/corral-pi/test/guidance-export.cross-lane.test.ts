// guidance-and-session cross-lane (T-564): the real corral binary dispatches a pi-native worker
// running the production worker role from a fixture installation, in the private plugin-free
// herdr server of scripts/test-herdr.sh. Through the bridge and the visible pane it observes the
// packaged role skill winning over a home impostor, trusted project guidance, template and skill
// slash expansion, and `/corral-export` attaching one durable artifact per session snapshot through
// corral's real artifact operation (a repeat replays its receipt).
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { connect } from "./support/bridge-client.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { EventFollower, corralBinary, generationDir, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

const isolated = privateHerdr();
const herdrRoot = privateRoot() ?? "";
let corral = "";
let workspace = "";
let install: FixtureInstall | undefined;
let events: EventFollower | undefined;
let pane = "";
const workspaces: string[] = [];
let turnsEnded = 0;

/** The next turn's end: the follower replays seen events, so count them to the exact Nth one. */
function nextTurnEnd(): Promise<unknown> {
  const target = ++turnsEnded;
  let seen = 0;
  return events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001" && ++seen === target);
}

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));
const frame = (cap: string, command: string, args: Record<string, unknown> = {}) =>
  ({ version: 1, id: `xg-${command}-${Math.random().toString(36).slice(2, 10)}`, ctl_cap: cap, generation: 1, command, args });

function put(path: string, text: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
}

function skill(dir: string, name: string, body: string): void {
  put(join(dir, name, "SKILL.md"), `---\nname: ${name}\ndescription: ${name} fixture. Use when testing guidance.\n---\n${body}\n`);
}

function screen(): string {
  return ok(["herdr", "--session", "corral-test", "pane", "read", pane, "--source", "recent", "--lines", "400"]);
}

function exportsOf(task: string): { path: string; title?: string; kind?: string }[] {
  return (show(task).artifacts as { path: string; title?: string; kind?: string }[])
    .filter((artifact) => /session-[0-9a-f]{16}\.html$/.test(artifact.path));
}

beforeAll(() => {
  if (!isolated) {
    console.log("guidance-export cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(herdrRoot, "pi-guide-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  put(join(app, "AGENTS.md"), "XL-PROJECT-GUIDE\n");
  put(join(app, ".agents/prompts/xl-echo.md"), "---\ndescription: Echo a token\n---\nReply only $1.\n");
  skill(join(app, ".agents/skills"), "xl-skill", "XL-SKILL-BODY. Reply only SKILLXL.");
  ok(["git", "-C", app, "add", "."]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  // The private HOME of scripts/test-herdr.sh: a home skill and an impostor of the role skill.
  const home = process.env.HOME ?? "";
  skill(join(home, ".agents/skills"), "xl-home", "XL-HOME-BODY");
  skill(join(home, ".agents/skills"), "corral-worker", "HOME-IMPOSTOR");
  cli("init");
  install = buildFixtureInstall({ parent: workspace });
  for (const tree of ["src/roles", "src/extensions", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(install.dir, tree), { recursive: true });
  }
  writeFileSync(join(workspace, ".corral/corral.toml"), `${readFileSync(join(workspace, ".corral/corral.toml"), "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
assets_dir = ${JSON.stringify(install.dir)}
`);
  cli("new", "Guidance and export", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  const home = process.env.HOME;
  if (isolated && home) {
    for (const name of ["xl-home", "corral-worker"]) rmSync(join(home, ".agents/skills", name), { recursive: true, force: true });
  }
});

test("a dispatched worker keeps its role skill, loads trusted guidance and expands slash commands in its pane", async () => {
  if (!isolated) return;
  // Given a dispatched worker whose kickoff turn has ended:
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  const kickoff = nextTurnEnd();
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  const dispatched = show("T-001").runs.at(-1);
  workspaces.push(dispatched.herdr.workspace_id);
  pane = dispatched.herdr.pane_id;
  await bound;
  await kickoff;
  const dir = generationDir(workspace, "T-001", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  try {
    // Then the host's readback shows the installation's corral-worker skill (not the home
    // impostor), the home and repository skills, and the worktree's trusted AGENTS.md.
    const state = (await client.request(frame(cap, "get_state"))).result as {
      guidance: { project_text: boolean; context_files: string[]; skills: { name: string; path: string }[]; export: boolean };
    };
    const skills = Object.fromEntries(state.guidance.skills.map((item) => [item.name, item.path]));
    expect(skills["corral-worker"]).toBe(join(install!.dir, "src/roles/worker/skills/corral-worker/SKILL.md"));
    const pack = ["frontend", "visual-qa", "programming", "debugging", "git-master"];
    expect(Object.keys(skills).sort()).toEqual(["corral-worker", "xl-home", "xl-skill", ...pack].sort());
    for (const name of pack) {
      expect(skills[name]).toBe(join(process.env.HOME ?? "", ".corral/skills/pi-native", name, "SKILL.md"));
      const installed = readFileSync(skills[name]!, "utf8");
      expect(installed).toMatch(new RegExp(`<!-- corral-skill: ${name} version=\\S+ sha256=[a-f0-9]{64} -->`));
    }
    console.log(`PI-PACK-DISPATCH ${JSON.stringify(Object.fromEntries(pack.map((name) => [name, skills[name]])))}`);
    expect(state.guidance.project_text).toBeTrue();
    expect(state.guidance.context_files).toEqual([join(dispatched.worktree, "AGENTS.md")]);
    expect(state.guidance.export).toBeTrue();
    // When the owner's input names the project template and the repository skill:
    for (const [text, key] of [["/xl-echo TEMPLATEXL", "xg-template"], ["/skill:xl-skill now", "xg-skill"]] as const) {
      const ended = nextTurnEnd();
      const reply = await client.request(frame(cap, "submit_input", {
        text, busy_policy: "follow_up", idempotency_key: key, origin: "cross-lane" }));
      expect(reply.result?.disposition).toBe("started");
      await ended;
    }
    // Then the pane shows both expansions and the fixture model's answers to them.
    const visible = screen();
    console.log(`XL-SCREEN-BEGIN\n${visible}\nXL-SCREEN-END`);
    for (const token of ["Reply only TEMPLATEXL.", "TEMPLATEXL", "SKILLXL"]) expect(visible).toContain(token);
  } finally {
    client.close();
  }
}, 600_000);

test("/corral-export attaches one artifact per snapshot through corral's artifact operation and replays a repeat", async () => {
  if (!isolated) return;
  const dir = generationDir(workspace, "T-001", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  try {
    const exported = async (key: string) => {
      const from = ((await client.request(frame(cap, "get_state"))).result as { last_seq: number }).last_seq;
      // A handled command has finished, so its export and receipt are complete.
      const reply = await client.request(frame(cap, "submit_input", {
        text: "/corral-export", busy_policy: "follow_up", idempotency_key: key, origin: "cross-lane" }));
      expect(reply.result?.disposition).toBe("handled");
      const page = (await client.request(frame(cap, "events_since", { seq: from }))).result as {
        events: { kind: string; path?: string; state?: string; replayed?: boolean }[];
      };
      const done = page.events.filter((event) => event.kind === "session_exported");
      if (done.length !== 1) console.log(`XL-EXPORT-EVENTS ${JSON.stringify(page.events)}`);
      expect(done).toHaveLength(1);
      return done[0]!;
    };
    // Given an unchanged snapshot, when the owner exports it twice:
    const first = await exported("xg-export-1");
    const second = await exported("xg-export-2");
    // Then corral committed the first attachment, replayed the second receipt, and holds exactly
    // one durable artifact whose stored copy is the offline HTML.
    expect([first.state, first.replayed]).toEqual(["committed", false]);
    expect([second.state, second.replayed, second.path]).toEqual(["committed", true, first.path]);
    const once = exportsOf("T-001");
    expect(once).toHaveLength(1);
    expect(once[0]?.kind).toBe("log");
    const stored = once[0]!.path.startsWith("/") ? once[0]!.path : join(workspace, ".corral", once[0]!.path);
    const html = readFileSync(stored, "utf8");
    expect(html).toContain("<html");
    expect(html).not.toMatch(/<(script|link|img)[^>]+(src|href)=["']https?:/i);
    // When the session gains a turn and is exported again:
    const ended = nextTurnEnd();
    const reply = await client.request(frame(cap, "submit_input", {
      text: "Reply only AFTERXL.", busy_policy: "follow_up", idempotency_key: "xg-after", origin: "cross-lane" }));
    expect(reply.result?.disposition).toBe("started");
    await ended;
    const third = await exported("xg-export-3");
    // Then the new snapshot is a second artifact.
    expect([third.state, third.replayed]).toEqual(["committed", false]);
    expect(third.path).not.toBe(first.path);
    expect(exportsOf("T-001")).toHaveLength(2);
    console.log(`XL-EXPORTS ${JSON.stringify({ first, second, third, artifacts: exportsOf("T-001") })}`);
  } finally {
    client.close();
  }
}, 600_000);
