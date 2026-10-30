// Real Corral generation admission and Pi host recovery, on a private herdr server only.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { connect } from "./support/bridge-client.ts";
import { buildFixtureInstall, PACKAGE_ROOT, type FixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import { EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

const isolated = privateHerdr();
let corral = "", workspace = "", workspaceId = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (): any => JSON.parse(cli("show", "T-001", "--json"));
const frame = (cap: string, generation: number, command: string, args: Record<string, unknown>) =>
  ({ version: 1, id: crypto.randomUUID(), ctl_cap: cap, generation, command, args });

beforeAll(() => {
  if (!isolated) { console.log("lsp resume: skipped (requires private test-herdr server)"); return; }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot()!, "lsp-resume-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "main.fk"), "def alpha\nuse alpha\n");
  ok(["git", "-C", app, "add", "main.fk"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
  cli("init");
  assets = buildFixtureInstall({ parent: workspace, omit: ["role", "delivery"] });
  // Preserve the production role and add only a deterministic fixture command which scripts
  // the synthetic provider to invoke its actual LSP tool on the next turn.
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const worker = join(assets.dir, "src/roles/worker");
  copyFileSync(join(worker, "index.ts"), join(worker, "original.ts"));
  writeFileSync(join(worker, "index.ts"), `import role from './original.ts';
const extend = role.extend;
export default {...role, extend(pi, context) {
  extend?.(pi, context);
  pi.registerCommand('fixture-lsp', {description: 'script LSP', async handler() {
    globalThis[Symbol.for('corral-pi.fixture.transport')].script([
      {toolCalls: [{callId: 'lsp-call', name: 'lsp_definition', arguments: {path:'main.fk', line:1, column:1}}]},
      {text:'LSP_DONE'}]);
  }});
}};\n`);
  const fake = join(assets.dir, "src/extensions/lsp/testing/fake-server.ts");
  const config = join(workspace, ".corral/corral.toml");
  writeFileSync(config, `${readFileSync(config, "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
[agent]
assets_dir = ${JSON.stringify(assets.dir)}
[agent.capabilities.worker.lsp.languages.first]
command = ${JSON.stringify(process.execPath)}
args = [${JSON.stringify(fake)}]
extensions = { ".fk" = "fake" }
`);
  cli("new", "LSP resume", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  assets?.cleanup();
});

test("explicit lost-host resume uses changed LSP settings and retains the prior generation", async () => {
  if (!isolated) return;
  const firstBound = events!.next(e => e.kind === "agent_bound" && e.task === "T-001", 90_000, "first bind");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaceId = show().runs.at(-1).herdr.workspace_id;
  try { await firstBound; } catch (error) {
    console.error(run(["herdr", "--session", "corral-test", "pane", "read", show().runs.at(-1).herdr.pane_id]));
    throw error;
  }
  // A fresh worker session starts its first turn with the run prompt (roles/worker kickoff) only
  // once InteractiveMode binds the role's extension, after hello, so agent_bound and a bound hello
  // can precede that turn by any time under load. Generation 2 resumes a session that already has
  // messages, so it has no kickoff, and its recovery notice reaches it only through a sync.
  const call = async (generation: number, kickoffTurns: number) => {
    const dir = generationDir(workspace, "T-001", "R1", generation);
    const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
    const client = await connect(join(dir, "host.sock"));
    await within((async () => { for (;;) {
      if ((await client.request(frame(cap, generation, "hello", {}))).result?.bound === true) return;
    } })(), 30_000, "bound host");
    if (kickoffTurns > 0) {
      await events!.next(e => e.kind === "agent_turn_ended" && e.task === "T-001" && e.detail?.generation === generation && e.detail?.turn === kickoffTurns,
        90_000, `kickoff turn ${generation}`);
    }
    // Only an idle host starts the lookup as its own turn: a follow-up queued into an open turn
    // runs inside that turn, so no later turn would end.
    const state = await client.request(frame(cap, generation, "get_state", {}));
    expect(state.result?.turn).toEqual({ current: kickoffTurns, open: false });
    const scripted = await client.request(frame(cap, generation, "submit_input", { text: "/fixture-lsp", busy_policy: "follow_up", idempotency_key: `script-${generation}`, origin: "test" }));
    expect(scripted.result?.disposition).toBe("handled");
    const next = kickoffTurns + 1;
    const end = events!.next(e => e.kind === "agent_turn_ended" && e.task === "T-001" && e.detail?.generation === generation && e.detail?.turn === next,
      90_000, `LSP turn ${generation}/${next}`);
    const submitted = await client.request(frame(cap, generation, "submit_input", { text: "Run the lookup.", busy_policy: "follow_up", idempotency_key: `query-${generation}`, origin: "test" }));
    expect([submitted.result?.disposition, submitted.result?.turn]).toEqual(["started", next]);
    await end;
    client.close();
  };
  await call(1, 1);
  const old = show().runs.at(-1).agent_host.generations[0];
  const originalSnapshot = structuredClone(old.capabilities);
  expect(readFileSync(old.bound.session_file, "utf8")).toContain('\\"server\\":\\"first\\"');
  const exits = join(workspace, ".corral/state/launch-exits");
  mkdirSync(exits, { recursive: true });
  const exited = fileAppears(exits, "T-001-R1.exited");
  process.kill(old.bound.pid, "SIGKILL");
  await exited;
  const lost = events!.next(e => e.kind === "agent_lost" && e.task === "T-001", 90_000, "host lost");
  cli("sync"); await lost;
  const config = join(workspace, ".corral/corral.toml");
  writeFileSync(config, readFileSync(config, "utf8").replace("languages.first", "languages.second").replace('extensions = { ".fk"', 'root_markers = [".git"]\nextensions = { ".fk"'));
  const secondBound = events!.next(e => e.kind === "agent_bound" && e.task === "T-001" && e.detail?.generation === 2,
    90_000, "second bind");
  expect(JSON.parse(cli("agent", "resume", "T-001")).generation).toBe(2);
  await secondBound;
  await call(2, 0);
  const [previous, current] = show().runs.at(-1).agent_host.generations;
  expect(previous.capabilities).toEqual(originalSnapshot);
  expect(current.capabilities.digest).not.toBe(originalSnapshot.digest);
  expect(current.capability_transition.changes.some((c: any) => c.key === "lsp" && c.settings)).toBeTrue();
  expect(current.bound.session_id).toBe(previous.bound.session_id);
  expect(current.bound.session_file).toBe(previous.bound.session_file);
  expect(readFileSync(current.bound.session_file, "utf8")).toContain('\\"server\\":\\"second\\"');
}, 600_000);
