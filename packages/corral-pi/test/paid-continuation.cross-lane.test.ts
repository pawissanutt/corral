// Two offline user journeys through real corral launches and HostSession.create's metered catalog.
// No admission adapter is called by this test: the fake endpoint sees only physical guarded POSTs.
import { expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, watch, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { connect } from "./support/bridge-client.ts";
import { StoreEventFollower, storeEvents } from "./support/canonical-store.ts";
import { corralBinary, ok, privateHerdr, privateRoot, run, type StoreEvent } from "./support/workspace.ts";
import { within } from "./support/wait.ts";

const isolated = privateHerdr();
if (!isolated) console.log("paid continuation cross-lane: blocked, skipped (requires private test-herdr server)");
const journey = test.skipIf(!isolated);
const ALLOWANCE = 1_000_000_000_000;
let binary: string | undefined;
let inputSequence = 0;

interface Generation {
  n: number;
  connection: { connection: string; candidate: string | null; digest: string };
  fenced?: { reason: string };
  bound?: { session_id: string; session_file: string };
}
interface NativeState {
  binding: { generation: number; health: string };
  generations: Generation[];
  paid_episode?: { episode: string; epoch: number };
  budget: { requests: { connection?: string; generation: number; request: string; response: string | null; tokens: number; usd_nanos: number }[] };
  receipts: { op: string; result: Record<string, unknown> }[];
  turns?: { generation: number; seq: number; class?: string; provider_failure?: { category: string; connection: string } }[];
}
interface WorkerRun {
  id: string;
  outcome: string | null;
  agent_host: NativeState;
  herdr: { workspace_id: string; pane_id: string };
}
interface Assistant {
  session: { id: string };
  container: { state: NativeState; owner_inputs?: { id: string; text: string }[] };
}
interface Ready { runtimeDir: string; identity: { generation: number } }
interface PhysicalPost { connection: string; method: string; url: string }

function patch(path: string, needle: string, replacement: string): void {
  const source = readFileSync(path, "utf8");
  if (source.split(needle).length !== 2) throw new Error(`fixture hook missing or ambiguous: ${needle}`);
  writeFileSync(path, source.replace(needle, replacement));
}

async function readyOrStopped(root: string, name: string): Promise<Ready> {
  const stopped = name.replace(/^ready-/, "stopped-");
  const { promise, resolve, reject } = Promise.withResolvers<Ready>();
  const inspect = () => {
    for (const failure of [stopped, "stopped-before-bootstrap.json"]) {
      if (existsSync(join(root, failure))) {
        reject(new Error(`fixture host stopped: ${readFileSync(join(root, failure), "utf8")}`));
        return;
      }
    }
    if (existsSync(join(root, name))) resolve(JSON.parse(readFileSync(join(root, name), "utf8")) as Ready);
  };
  const watcher = watch(root, inspect);
  try {
    inspect();
    return await within(promise, 90_000, name);
  } finally { watcher.close(); }
}
const quote = (text: string) => `'${text.replaceAll("'", "'\\''")}'`;

function fixture(vendor: "anthropic" | "openai") {
  const parent = privateRoot();
  if (!parent) throw new Error("private test-herdr root required");
  const root = mkdtempSync(join(parent, "paid-continuation-"));
  const app = join(root, "app");
  const corral = binary ??= corralBinary();
  const cli = (...args: string[]) => ok([corral, "--root", root, ...args], { cwd: root });
  const workspaces = new Set<string>();
  const panes = new Set<string>();
  mkdirSync(app);
  for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "fixture"],
    ["config", "user.email", "fixture@example.invalid"]]) ok(["git", "-C", app, ...args]);
  writeFileSync(join(app, "README.md"), "offline paid continuation fixture\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-qm", "fixture"]);
  cli("init");
  // Keep the real worker/assistant roles and their authority checks; replace only auth/delivery.
  const assets = buildFixtureInstall({ parent: root, isolatePiState: true, omit: ["role"] });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const posts = join(root, "physical-posts.jsonl");
  const exits = join(root, "managed-exits.jsonl");
  writeFileSync(posts, "");
  writeFileSync(exits, "");
  const main = join(assets.dir, "src/host/main.ts");
  writeFileSync(main, `import { writeFileSync as fixtureWrite, appendFileSync as fixtureAppend } from "node:fs";
let fixtureStopped = "stopped-before-bootstrap.json";
let fixtureStage = "bootstrap";
${readFileSync(main, "utf8")}`);
  patch(main, "  host = await HostSession.create({", `  fixtureStopped = "stopped-" + config.identity.role + "-" + (config.identity.run ?? config.identity.assistant) + "-g" + config.identity.generation + ".json";
  fixtureStage = "create";
  host = await HostSession.create({`);
  patch(main, "  await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));",
    `  fixtureStage = "hello";
  await session.hello(config.secrets.bindNonce, config.pane, sha256Hex(config.secrets.ctlCap));
  fixtureStage = "hello-complete";`);
  patch(main, "  await terminal.init();", `  fixtureStage = "terminal-init";
  await terminal.init();
  fixtureStage = "terminal-init-complete";`);
  patch(main, "  const message = error instanceof Error ? error.message : String(error);",
    `  const message = error instanceof Error ? error.message : String(error);
  fixtureWrite(${JSON.stringify(root)} + "/" + fixtureStopped, JSON.stringify({ stage: fixtureStage, message, stack: error instanceof Error ? error.stack : undefined }));`);
  patch(main, '  session.markAttached("interactive");', `  session.markAttached("interactive");
  fixtureWrite(${JSON.stringify(root)} + "/ready-" + config.identity.role + "-" + (config.identity.run ?? config.identity.assistant) + "-g" + config.identity.generation + ".json", JSON.stringify({ runtimeDir: config.runtimeDir, identity: config.identity }));`);
  patch(main, "    process.exit(code);", `    fixtureAppend(${JSON.stringify(exits)}, JSON.stringify({ code, generation: config.identity.generation }) + "\\n");
    process.exit(code);`);
  const auth = join(assets.dir, "src/auth/index.ts");
  patch(auth, "  transport() {", `  transport(context) {`);
  patch(auth, "    return fixtureTransport().send;", `    if (context.connection === "openai-subscription") fixtureTransport().script([
      { match: "PAID_ASSISTANT_QUOTA", error: { status: 429, body: { error: { type: "usage_limit_reached", message: "Synthetic subscription quota", resets_at: 1800000600 } } } },
    ]);
    return async (input, init) => {
      appendFileSync(${JSON.stringify(posts)}, JSON.stringify({ connection: context.connection, method: init?.method ?? "GET", url: input instanceof Request ? input.url : String(input) }) + "\\n");
      return fixtureTransport().send(input, init);
    };`);
  writeFileSync(auth, `import { appendFileSync } from "node:fs";\n${readFileSync(auth, "utf8")}${vendor === "anthropic" ? claudeFixture() : ""}`);
  if (vendor === "anthropic") {
    // The fake SDK never spawns Claude Code; do not require its optional native executable.
    patch(join(assets.dir, "src/claude/sdk.ts"),
      "  const candidates = claudeCodeCandidates(deps.platform, deps.arch);",
      '  if (override) return "/synthetic/offline-claude";\n  const candidates = claudeCodeCandidates(deps.platform, deps.arch);');
  }

  const model = vendor === "anthropic" ? "claude-sonnet-5-5" : "gpt-6.1-sol";
  const candidate = (id: string, connection: string, cost: string) => `{ id = ${JSON.stringify(id)}, harness = "corral-pi", connection = ${JSON.stringify(connection)}, provider = ${JSON.stringify(vendor)}, model = ${JSON.stringify(model)}, effort = "high", args = [], evidence = "offline-owner-approval", cost = ${cost} }`;
  const configPath = join(root, ".corral/corral.toml");
  writeFileSync(configPath, `${readFileSync(configPath, "utf8")}
[agent]
assets_dir = ${JSON.stringify(assets.dir)}
[profiles.paid-fixture]
harness = "corral-pi"
connection = "${vendor}-subscription"
model = "${model}"
vendor = "${vendor}"
effort = "high"
[profiles.paid-fixture.metadata]
display_name = "Offline paid fixture"
purpose = "Synthetic quota continuation"
instructions = "Use only offline scripted transport"
revision = "offline-v1"
candidates = [${candidate("subscription", `${vendor}-subscription`, '{ kind = "subscription" }')}, ${candidate("api", `${vendor}-api`, '{ kind = "metered", currency = "USD", maximum_micros = 1000000000 }')}]
[profiles.paid-fixture.metadata.guidance]
[profiles.paid-fixture.metadata.eligibility]
enabled = true
automatic = true
roles = ["implementation"]
projects = []
[profiles.paid-fixture.metadata.fallback]
automatic = true
paid_roles = ["${vendor === "anthropic" ? "worker" : "assistant"}"]
approved_providers = ["${vendor}"]
max_switches = 1
allow_unknown_subscription_cost = true
approval = "offline-owner-approval"
spending = { currency = "USD", maximum_micros = 1000000000 }
`);
  // One offline host only: resource pressure on the shared machine is not under test.
  cli("config", "set", "resources.max_load_per_core", "1000000.0");
  cli("config", "set", "assistant.agent", '"paid-fixture"');
  return {
    root, app, corral, cli, workspaces, panes,
    worker: () => JSON.parse(cli("show", "T-001", "--json")) as { runs: WorkerRun[] },
    assistant: () => JSON.parse(cli("assistant", "show", "A-1", "--json")) as Assistant,
    posts: () => readFileSync(posts, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line) as PhysicalPost),
    exits: () => readFileSync(exits, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line) as { code: number; generation: number }),
    async ready(role: "worker" | "assistant", id: string, generation: number): Promise<Ready> {
      const name = `ready-${role}-${id}-g${generation}.json`;
      return readyOrStopped(root, name);
    },
    diagnose(error: unknown) {
      console.error(`paid continuation ${vendor} failure: ${error instanceof Error ? error.stack : String(error)}`);
      // Print canonical state before cleanup: private test-herdr removes its whole root on exit.
      for (const args of [["show", "T-001", "--json"], ["assistant", "show", "A-1", "--json"],
        ["export", "events", "--json"], ["mode", "status", "--json"]]) {
        const result = run([corral, "--root", root, ...args], { cwd: root });
        console.error(JSON.stringify({ fixture: vendor, command: args, ...result }));
      }
      console.error(JSON.stringify({ fixture: vendor, posts: readFileSync(posts, "utf8"), exits: readFileSync(exits, "utf8") }));
      for (const name of readdirSync(root).filter(name => name.startsWith("stopped-"))) {
        console.error(JSON.stringify({ fixture: vendor, stopped: name, error: readFileSync(join(root, name), "utf8") }));
      }
      for (const pane of panes) {
        const result = run(["herdr", "--session", "corral-test", "pane", "read", "--source", "recent-unwrapped", "--lines", "120", "--format", "text", pane]);
        console.error(JSON.stringify({ fixture: vendor, pane, ...result }));
      }
    },
    close() {
      for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
      assets.cleanup();
      rmSync(root, { recursive: true, force: true });
    },
  };
}

/** The real SDK lane consumes a pushed prompt before yielding each response, including reuse. */
function claudeFixture(): string {
  return `
import { overrideSdk } from "../claude/sdk.ts";
import type { Query } from "@anthropic-ai/claude-agent-sdk";
enableAnthropicFixture();
overrideSdk({
  query: ({ prompt }) => Object.assign((async function* () {
    for await (const pushed of prompt) {
      if (JSON.stringify(pushed).includes("PAID_WORKER_QUOTA")) {
        yield { type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1800000600 } };
        yield { type: "assistant", error: "rate_limit", message: { content: [], stop_reason: null } };
      } else {
        yield { type: "result", subtype: "success", is_error: false, result: "Offline subscription ready.", stop_reason: "end_turn", usage: { input_tokens: 32, output_tokens: 3 } };
      }
    }
  })(), { interrupt: async () => undefined, close: () => undefined }) as Query,
  createSdkMcpServer: options => ({ type: "sdk", name: options.name, instance: options }) as never,
});
`;
}

async function submit(ready: Ready, text: string): Promise<void> {
  const client = await connect(join(ready.runtimeDir, "host.sock"));
  const key = `paid-input-${++inputSequence}`;
  try {
    const reply = await client.request({ version: 1, id: key, ctl_cap: readFileSync(join(ready.runtimeDir, "ctl.cap"), "utf8"),
      generation: ready.identity.generation, command: "submit_input",
      args: { text, busy_policy: "prompt", idempotency_key: key, origin: "paid-offline-fixture" } });
    expect(reply.ok).toBeTrue();
    expect(reply.result?.disposition).toBe("started");
  } finally { client.close(); }
}
function ended(event: StoreEvent, generation: number, runId?: string, turn?: number): boolean {
  return event.kind === "agent_turn_ended" && event.detail?.generation === generation
    && (runId === undefined || event.detail?.container === `T-001/${runId}`)
    && (turn === undefined || event.detail?.turn === turn);
}
function paidState(state: NativeState, connection: string, generation: number): void {
  expect(state.binding).toMatchObject({ health: "healthy", generation });
  expect(state.generations.at(-1)?.connection).toMatchObject({ connection, candidate: "api" });
  expect(state.paid_episode?.episode).toBeDefined();
  const requests = state.budget.requests.filter(request => request.connection === connection);
  expect(requests).toHaveLength(1);
  expect(requests[0]?.generation).toBe(generation);
  expect(requests[0]?.tokens).toBe(35);
  expect(requests[0]?.usd_nanos ?? 0).toBeGreaterThan(0);
  expect(typeof requests[0]?.response).toBe("string");
  // These receipts come from host.budget.physical; a transport-only success cannot satisfy them.
  const admitted = state.receipts.filter(receipt => receipt.op === "event" && receipt.result.admitted === true
    && receipt.result.connection === connection && typeof receipt.result.paid === "object");
  expect(admitted.length).toBeGreaterThanOrEqual(2); // physical reserve and settle
  const balances = admitted.map(receipt => receipt.result.paid as { allowance_nanos: number; consumed_or_reserved_nanos: number });
  expect(balances.some(balance => balance.allowance_nanos === ALLOWANCE
    && balance.consumed_or_reserved_nanos === requests[0]?.usd_nanos)).toBeTrue();
}
function switched(event: StoreEvent, vendor: string): void {
  expect(event.detail).toMatchObject({ source_connection: `${vendor}-subscription`, target_connection: `${vendor}-api`,
    candidate: "api", allowance_nanos: ALLOWANCE, consumed_or_reserved_nanos: 0 });
}

journey("anthropic worker quota links a replacement run and admits exactly one Messages POST with switch and spend history", async () => {
  const fx = fixture("anthropic");
  let events: StoreEventFollower | undefined;
  try {
    fx.cli("new", "Offline worker continuation", "-p", "app", "--ready");
    events = await StoreEventFollower.open(fx.root);
    fx.cli("dispatch", "T-001", "--profile", "paid-fixture", "--force");
    const source = fx.worker().runs.at(-1);
    if (!source) throw new Error("worker dispatch created no run");
    fx.workspaces.add(source.herdr.workspace_id);
    fx.panes.add(source.herdr.pane_id);
    const ready = await fx.ready("worker", source.id, 1);
    await events.next(event => ended(event, 1, source.id, 1));
    const identity = JSON.parse(fx.cli("run", "progress", "T-001", "--json")) as {
      task: string; run: string; spec_revision: string; head: string; base: string; tree: string; progress: { revision: number } | null;
    };
    const checkpoint = join(fx.root, "checkpoint.json");
    writeFileSync(checkpoint, JSON.stringify({ task: identity.task, run: identity.run, spec_revision: identity.spec_revision,
      head: identity.head, base: identity.base, tree: identity.tree, expected_revision: 0,
      progress_revision: identity.progress?.revision ?? 0, criterion_ids: [], evidence_refs: [],
      next_action: "Continue the offline worker handoff", decisions: [], waits: [], resources: [] }));
    fx.cli("run", "checkpoint", "T-001", "--file", checkpoint, "--json");
    // Paid approval grants spending only; the replacement also needs owner-approved dispatch.
    const scope = ["--task", "T-001", "--dispatch"];
    const approval = JSON.parse(fx.cli("mode", "autopilot", ...scope, "--json")) as { id: string };
    fx.cli("mode", "autopilot", ...scope, "--confirm", approval.id, "--json");
    await submit(ready, "PAID_WORKER_QUOTA");
    await events.next(event => ended(event, 1, source.id, 2));
    fx.cli("sync");
    const replacement = fx.worker().runs.at(-1);
    if (!replacement) throw new Error("continuation created no replacement run");
    fx.workspaces.add(replacement.herdr.workspace_id);
    fx.panes.add(replacement.herdr.pane_id);
    expect(replacement.id).not.toBe(source.id);
    await fx.ready("worker", replacement.id, 1);
    await events.next(event => ended(event, 1, replacement.id, 1));
    const history = storeEvents(fx.corral, fx.root);
    const switches = history.filter(event => event.task === "T-001" && event.kind === "continuation_switched");
    expect(switches).toHaveLength(1);
    switched(switches[0]!, "anthropic");
    expect(switches[0]?.detail?.source).toMatchObject({ kind: "worker", task: "T-001", run: source.id });
    expect(switches[0]?.detail?.successor).toMatchObject({ kind: "worker", task: "T-001", run: replacement.id });
    const status = fx.worker();
    expect(status.runs[0]?.outcome).toBe("superseded");
    paidState(status.runs.at(-1)!.agent_host, "anthropic-api", 1);
    expect(status.runs[0]?.agent_host.paid_episode?.episode).toBe(status.runs.at(-1)?.agent_host.paid_episode?.episode);
    const posts = fx.posts();
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ connection: "anthropic-api", method: "POST" });
    const endpoint = new URL(posts[0]!.url);
    expect(endpoint.origin + endpoint.pathname).toBe("https://api.anthropic.com/v1/messages");
  } catch (error) { fx.diagnose(error); throw error; }
  finally { events?.close(); fx.close(); }
}, 300_000);

journey("openai assistant quota exits 78 and admits exactly one Responses POST in generation n+1 without changing owner-input ids", async () => {
  const fx = fixture("openai");
  let events: StoreEventFollower | undefined;
  try {
    events = await StoreEventFollower.open(fx.root);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", fx.app, "--no-focus"])) as {
      result: { workspace: { workspace_id: string }; root_pane: { pane_id: string } };
    };
    fx.workspaces.add(created.result.workspace.workspace_id);
    const pane = created.result.root_pane.pane_id;
    fx.panes.add(pane);
    ok(["herdr", "--session", "corral-test", "pane", "send-text", pane, `${quote(fx.corral)} --root ${quote(fx.root)} pi --assistant`]);
    ok(["herdr", "--session", "corral-test", "pane", "send-keys", pane, "enter"]);
    const ready = await fx.ready("assistant", "A-1", 1);
    ok(["herdr", "--session", "corral-test", "pane", "send-text", pane, "Keep the fixture owner decision unchanged."]);
    ok(["herdr", "--session", "corral-test", "pane", "send-keys", pane, "enter"]);
    await events.next(event => ended(event, 1, undefined, 1));
    const before = fx.assistant();
    expect(before.container.owner_inputs).toHaveLength(1);
    expect(before.container.owner_inputs?.[0]?.id).toBe("A-1/g1/i1");
    await submit(ready, "PAID_ASSISTANT_QUOTA");
    const next = await fx.ready("assistant", "A-1", 2);
    expect(fx.exits()).toEqual([{ code: 78, generation: 1 }]);
    const resumed = fx.assistant();
    expect(resumed.session.id).toBe(before.session.id);
    expect(resumed.container.owner_inputs).toEqual(before.container.owner_inputs);
    const generations = resumed.container.state.generations;
    expect(generations).toHaveLength(2);
    expect(generations[1]?.connection).toMatchObject({ connection: "openai-api", candidate: "api" });
    const quotaTurn = resumed.container.state.turns?.find(turn => turn.generation === 1 && turn.seq === 2);
    expect(quotaTurn).toMatchObject({ provider_failure: { connection: "openai-subscription", category: "subscription_quota" } });
    // Assistant turns intentionally retain no worker/reviewer classification.
    expect(quotaTurn?.class).toBeUndefined();
    expect(generations[0]?.fenced).toBeDefined();
    expect(generations[1]?.fenced).toBeUndefined();
    expect(generations[1]?.bound?.session_id).not.toBe(generations[0]?.bound?.session_id);
    await submit(next, "Continue from the verified offline context without replaying owner actions.");
    await events.next(event => ended(event, 2));
    const status = fx.assistant();
    expect(status.container.owner_inputs).toEqual(before.container.owner_inputs);
    paidState(status.container.state, "openai-api", 2);
    const switches = storeEvents(fx.corral, fx.root).filter(event => event.kind === "continuation_switched");
    expect(switches).toHaveLength(1);
    switched(switches[0]!, "openai");
    expect(switches[0]?.detail?.source).toEqual(switches[0]?.detail?.successor);
    expect(fx.posts().filter(post => post.connection === "openai-api")).toEqual([
      { connection: "openai-api", method: "POST", url: "https://api.openai.com/v1/responses" },
    ]);
  } catch (error) { fx.diagnose(error); throw error; }
  finally { events?.close(); fx.close(); }
}, 300_000);
