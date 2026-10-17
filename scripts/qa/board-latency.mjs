// Run only inside the private environment from scripts/test-herdr.sh.
// Usage: bun scripts/qa/board-latency.mjs BINARY EVIDENCE [WORKLOAD]
// WORKLOAD is retained, reset deterministically, and must not be used concurrently.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { createHash } from "node:crypto";
import { appendFile, mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { resolve, join, dirname } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-110/live");
const fixture = resolve(process.argv[4] ?? ".omo/evidence/T-110/workload");
const allowed = resolve(".omo/evidence/T-110") + "/";
assert(evidence.startsWith(allowed) && fixture.startsWith(allowed));
assert.notEqual(evidence, fixture);
assert(process.env.HOME?.startsWith("/tmp/corral-herdr-"));
assert(process.env.CORRAL_HERDR_SESSION === "corral-test");
const privateRoot = dirname(process.env.HOME);
const env = { ...process.env, CORRAL_ROOT: fixture };
for (const key of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
await mkdir(evidence, { recursive: true });
await mkdir(fixture, { recursive: true });
const deadline = performance.now() + 900_000;
const transcript = [];
const result = { status: "FAIL", binary, evidence, fixture, columns: 160, rows: 36,
  timing: "performance.now immediately before send_input to matching visible-output event; excludes capture I/O",
  samples: [], cleanup: {}, assumptions: [
    "Synthetic, not copied owner data. 12 of 97 Done tasks redistributed to Backlog for scrolling.",
    "Final mix: 85 done, 4 dropped, 5 idea, 12 backlog, 3 running, 1 review.",
    "Dispatch paused, no queued tasks, automatic backlog admission disabled; no agent launch commands.",
    "Three historical runs and six real artifact files per task; running cards use unowned synthetic runs.",
    "One bounded pass per input mode in idle and concurrent-change conditions; not a statistical benchmark.",
  ] };
let workspace, pane, socketPath, subscription, viewport, viewportOutput, viewportErrors, serial = 0;
let onOutput = () => {};
let screen = "";
const at = "2026-10-17T00:00:00Z";
const hash = (s) => createHash("sha256").update(s).digest("hex");
const quote = (s) => "'" + s.replaceAll("'", "'\\''") + "'";
const budget = () => Math.max(1, Math.min(45_000, deadline - performance.now()));

async function command(program, args) {
  assert(performance.now() < deadline, "Harness exceeded fifteen-minute budget");
  const child = Bun.spawn([program, ...args], { cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const timer = setTimeout(() => child.kill(), budget());
  try {
    const [stdout, stderr, status] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    transcript.push({ command: [program, ...args], status, stdout, stderr });
    assert.equal(status, 0, `${program} ${args.join(" ")}: ${stderr}`);
    return stdout;
  } finally { clearTimeout(timer); }
}
const cli = (...args) => command(binary, ["--root", fixture, ...args]);
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);

function rpc(method, params) {
  const id = `latency-${++serial}`;
  return new Promise((accept, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    const timer = setTimeout(() => { connection.destroy(); reject(new Error(`${method}: timed out`)); }, budget());
    let buffer = "";
    connection.setEncoding("utf8");
    connection.on("data", (chunk) => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(buffer.slice(0, end));
        buffer = buffer.slice(end + 1);
        transcript.push({ receivedAt: performance.now(), message });
        if (message.id === id) {
          clearTimeout(timer);
          if (!streaming) connection.destroy();
          if (message.error) reject(new Error(JSON.stringify(message.error)));
          else accept(message.result);
        } else if (message.event === "pane.output_matched" && message.data.pane_id === pane) {
          onOutput(message.data.read);
        }
      }
    });
    connection.on("error", (error) => { clearTimeout(timer); reject(error); });
    connection.once("connect", () => connection.write(JSON.stringify({ id, method, params }) + "\n"));
  });
}
const raw = (text) => rpc("pane.send_input", { pane_id: pane, text });
async function capture(name, read) {
  await writeFile(join(evidence, name + ".txt"), read.text);
  await writeFile(join(evidence, name + ".json"), JSON.stringify(read, null, 2) + "\n");
  await writeFile(join(evidence, name + ".ansi"),
    await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi", "--ansi"));
}
async function stage(name, check, action, condition = "setup", concurrent) {
  let timer, started, active = false;
  const matched = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: visible state not observed`)), budget());
    onOutput = (read) => {
      if (active && check(read.text)) {
        active = false;
        const visibleAt = performance.now();
        result.samples.push({ name, condition, started, visibleAt, ms: visibleAt - started });
        accept(read);
      }
    };
  });
  matched.catch(() => {});
  try {
    const marker = name === "00-tour" ? "skip tour" :
      name.endsWith("background-task-refresh") ? `Fresh ${condition}` :
      name.endsWith("background-review-refresh") ? (condition === "idle" ? "3 new" : "6 new") :
      name.endsWith("background-status-refresh") ? "Moved ready" :
      name.endsWith("background-needsme-refresh") ? "▲ 1 needs you" :
      name.endsWith("column-right") ? "corral  / search  M messages" :
      name.endsWith("column-left") ? "▌ T-001" :
      name.endsWith("keyboard-board-scroll") ? "▌ T-012" :
      name.endsWith("mouse-board-scroll") ? "T-010" :
      name.endsWith("board-up") ? "T-001" :
      name.endsWith("detail-scroll") ? "Fixture body line 030" :
      name.endsWith("detail-open") ? "Spec" :
      name.endsWith("capture-open") ? "Cancel" :
      name.endsWith("search-open") ? "results" :
      name.endsWith("search-type") ? "results 85" :
      name.endsWith("keyboard-search-scroll") ? "T-067" :
      name.endsWith("mouse-search-scroll") ? "T-080" :
      name.endsWith("history-open") ? "history ·" : "q leave";
    await rpc("events.subscribe", { subscriptions: [{
      type: "pane.output_matched", pane_id: pane, source: "visible",
      match: { type: "regex", value: marker }, strip_ansi: true,
    }] }); // Ack is mandatory before the action or concurrent filesystem changes.
    transcript.push({ stage: name, marker, subscriptionAcknowledged: true });
    console.log(`ready ${name}: ${marker}`);
    started = performance.now();
    active = true;
    const changes = concurrent?.();
    await action();
    const read = await matched;
    await changes;
    screen = read.text;
    await capture(name, read);
    console.log(`${name}: ${result.samples.at(-1).ms.toFixed(2)} ms`);
    return screen;
  } finally { clearTimeout(timer); active = false; onOutput = () => {}; }
}
function locate(needle) {
  const lines = screen.split("\n");
  const row = lines.findIndex((l) => l.includes(needle));
  assert(row >= 0, `Target missing: ${needle}\n${screen}`);
  return [Bun.stringWidth(lines[row].slice(0, lines[row].indexOf(needle))) + 1, row + 1];
}
const click = (needle) => {
  const [x, y] = locate(needle);
  return raw(`\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`);
};
const wheel = (down, x = 8, y = 12, count = 1) =>
  raw(`\x1b[<${down ? 65 : 64};${x};${y}M`.repeat(count));
const board = (t) => t.includes("Backlog") && t.includes("/ search") &&
  !t.includes("‹ back") && !t.includes("Cancel");
const taskPath = (id) => join(fixture, ".corral/tasks", id + "-fixture.md");
async function saveTask(task, body) {
  const path = taskPath(task.id);
  await writeFile(path + ".tmp", `---\n${JSON.stringify(task, null, 2)}\n---\n${body}`);
  await rename(path + ".tmp", path);
}

async function prepare() {
  const manifestPath = join(fixture, "workload.json");
  let workload;
  try { workload = JSON.parse(await readFile(manifestPath, "utf8")); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  if (!workload) {
    await mkdir(join(fixture, "app"), { recursive: true });
    await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
    await cli("init");
    const id = (await cli("new", "Fixture seed", "-p", "app", "--body", "Synthetic latency QA")).trim();
    const seed = JSON.parse(await cli("list", "--json"))[0];
    assert.equal(seed.id, id);
    await writeFile(join(fixture, "cli-seed.json"), JSON.stringify(seed, null, 2));
    const seedEvents = (await readFile(join(fixture, ".corral/events.jsonl"), "utf8"))
      .trim().split("\n").map(JSON.parse);
    const tasks = [];
    for (let i = 1; i <= 110; i++) {
      const task = { ...seed, id: `T-${String(i).padStart(3, "0")}`, title: `Fixture ${String(i).padStart(3, "0")}`,
        status: i <= 12 ? "backlog" : i <= 97 ? "done" : i <= 101 ? "dropped" :
          i <= 106 ? "idea" : i <= 109 ? "running" : "review",
        created: at, updated: at, priority: "P2", queued: false, dispatch_hold: true,
        labels: ["latency", "synthetic", "stage-cost"], kind: "implementation",
        write_scope: [`src/fixture-${i}/**`],
        comments: Array.from({ length: 24 }, (_, j) => ({ author: j % 2 ? "pm" : "owner",
          at, text: `Context ${i}/${j}: ` + "evidence acceptance review implementation ".repeat(35) })),
        runs: Array.from({ length: 3 }, (_, j) => ({
          id: `R${j + 1}`, agent: "fixture", profile: "fixture", harness: "fixture",
          model: "synthetic-model", effort: "high", mode: "interactive",
          worktree: join(fixture, "app"), branch: `corral/fixture-${i}-r${j + 1}`, base: "main",
          base_sha: "0".repeat(40), started: at, ended: at, outcome: "done", seen_working: false,
          summary: `Run ${j + 1}: ` + "Implemented bounded acceptance with artifact receipts. ".repeat(140),
          gate: { result: "pass", at, sha: "0".repeat(40) },
          review: { verdict: "approve", reviewer: "owner", sha: "0".repeat(40), at,
            notes: "Synthetic review of fixture metadata, no external activity." },
          usage: { input: 12000, output: 3500, cache_read: 6000, cache_write: 500 },
        })), artifacts: [] };
      for (let j = 0; j < 6; j++) {
        const path = `artifacts/${task.id}/R3/${j}-report.md`;
        const content = `# Fixture ${i} artifact ${j}\n` + "Acceptance evidence, no owner content.\n".repeat(24);
        await mkdir(dirname(join(fixture, ".corral", path)), { recursive: true });
        await writeFile(join(fixture, ".corral", path), content);
        task.artifacts.push({ run: "R3", kind: j % 2 ? "evidence" : "report",
          title: `Fixture report ${i}/${j}`, path, source: join(fixture, ".corral", path),
          sha256: hash(content), bytes: Buffer.byteLength(content), added: at });
      }
      tasks.push(task);
    }
    const bytes = Math.round(7.7 * 1024 * 1024);
    const bodies = tasks.map(() => "\n# Acceptance\n\nSynthetic fixture acceptance and verification notes.\n");
    let current = tasks.reduce((sum, t, i) =>
      sum + Buffer.byteLength(`---\n${JSON.stringify(t, null, 2)}\n---\n${bodies[i]}`), 0);
    assert(current < bytes);
    for (let i = 0; i < tasks.length; i++) {
      const padding = Math.floor((bytes - current) / (tasks.length - i));
      bodies[i] += "x".repeat(padding);
      current += padding;
    }
    const eventBytes = Math.round(4.2 * 1024 * 1024);
    const events = [];
    let size = 0;
    while (size < eventBytes) {
      const event = { ...seedEvents.at(-1), ts: at, task: tasks[events.length % 110].id,
        kind: "commented", detail: { author: "owner", text: "Synthetic event evidence. ".repeat(16) } };
      let line = JSON.stringify(event) + "\n";
      if (size + Buffer.byteLength(line) > eventBytes - 1500) {
        event.detail.text = "";
        const overhead = Buffer.byteLength(JSON.stringify(event) + "\n");
        event.detail.text = "e".repeat(eventBytes - size - overhead);
        line = JSON.stringify(event) + "\n";
      }
      events.push(line);
      size += Buffer.byteLength(line);
    }
    workload = { version: 1, tasks, bodies, events: events.join(""), taskBytes: bytes, eventBytes,
      redistribution: "T-001..T-012: done -> backlog; preserves all 110 tasks", seedEvents };
    await writeFile(manifestPath, JSON.stringify(workload));
  }
  assert.equal(workload.version, 1);
  // Stable numbered body lines make detail scrolling observable without timing guesses.
  workload.bodies = workload.bodies.map((body) => {
    const prefix = "\n# Acceptance\n\n" + Array.from({ length: 50 }, (_, i) =>
      `Fixture body line ${String(i + 1).padStart(3, "0")}: bounded acceptance and verification evidence.\n`).join("");
    assert(Buffer.byteLength(prefix) < Buffer.byteLength(body));
    return prefix + "x".repeat(Buffer.byteLength(body) - Buffer.byteLength(prefix));
  });
  // Only our marked workload may be reset; never inspect or reuse the owner's board.
  for (const name of await readdir(join(fixture, ".corral/tasks"))) {
    assert(/^T-\d+-.*\.md$/.test(name), `Unexpected fixture task file: ${name}`);
    await rm(join(fixture, ".corral/tasks", name));
  }
  for (let i = 0; i < 110; i++) await saveTask(workload.tasks[i], workload.bodies[i]);
  await writeFile(join(fixture, ".corral/events.jsonl"), workload.events);
  await mkdir(join(fixture, ".corral/state"), { recursive: true });
  await rm(join(fixture, ".corral/state/tour-seen"), { force: true });
  await writeFile(join(fixture, ".corral/state/reviewed-artifacts.json"), "[]");
  // Validation requires a PM profile. Its only command is false, never an agent.
  await writeFile(join(fixture, ".corral/corral.toml"), `[projects.app]\npath = ${JSON.stringify(join(fixture, "app"))}\n[dispatch]\npaused = true\nauto_backlog = false\npromote_ideas = "never"\n[review]\nautopilot = false\n[herdr]\nsession = "corral-test"\n[agents.fixture]\ncommand = "false"\n[pm]\nagent = "fixture"\n`);
  const parsed = JSON.parse(await cli("list", "--all", "--json"));
  assert.equal(parsed.length, 110);
  result.workload = { taskCount: parsed.length, taskBytes: workload.taskBytes,
    eventBytes: (await stat(join(fixture, ".corral/events.jsonl"))).size,
    artifacts: parsed.reduce((n, t) => n + (t.artifacts?.length ?? 0), 0),
    frontmatterBytes: workload.tasks.reduce((n, t) => n + Buffer.byteLength(JSON.stringify(t, null, 2)), 0),
    redistribution: workload.redistribution,
    sha256: hash(JSON.stringify(workload)), statuses: Object.fromEntries(
      [...new Set(parsed.map((t) => t.status))].map((s) => [s, parsed.filter((t) => t.status === s).length])) };
  return workload;
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.startsWith(privateRoot + "/") && socketPath.includes("/sessions/corral-test/"),
    `Refuse non-private socket: ${socketPath}`);
  result.socket = socketPath;
  const workload = await prepare();
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
    "--label", "T-110 latency fixture", "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  result.workspace = workspace;
  result.pane = pane;
  viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
    "control", pane, "--cols", String(result.columns), "--rows", String(result.rows)],
    { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  viewportErrors = new Response(viewport.stderr).text();
  const reader = viewport.stdout.getReader();
  let readyTimer;
  try {
    const first = await Promise.race([reader.read(), new Promise((_, reject) => {
      readyTimer = setTimeout(() => reject(new Error("Viewport control not ready")), 15000);
    })]);
    assert(!first.done, "Viewport control closed");
    await writeFile(join(evidence, "viewport.json"), new TextDecoder().decode(first.value));
  } finally { clearTimeout(readyTimer); }
  viewportOutput = (async () => { while (!(await reader.read()).done) {} })();
  await stage("00-tour", (t) => t.includes("skip tour"), () => herdr("pane", "run", pane,
    `${quote(binary)} --root ${quote(fixture)} board </dev/null`));
  await stage("01-board", board, () => raw("\x1b"));
  let pulse = 0;
  const mutate = async () => {
    const task = structuredClone(workload.tasks[109]);
    task.title = `Pulse ${++pulse}`;
    task.runs[2].review.notes = `Concurrent review change ${pulse}`;
    await saveTask(task, workload.bodies[109]);
    await appendFile(join(fixture, ".corral/events.jsonl"), JSON.stringify({
      ts: at, task: task.id, kind: "review_started", detail: { run: "R3", reviewer: "fixture", pulse },
    }) + "\n");
  };
  for (const condition of ["idle", "concurrent"]) {
    const run = (name, check, action) => stage(`${condition}-${name}`, check, action, condition,
      condition === "concurrent" ? mutate : undefined);
    await run("keyboard-column-right", (t) => board(t) && !t.includes("▌"), () => raw("l"));
    await run("keyboard-column-left", (t) => board(t) && t.includes("▌ T-001"), () => raw("h"));
    // Down movement spans the viewport; the detail verifies the final selected card.
    await run("keyboard-board-scroll", (t) => board(t) && t.includes("▌ T-012") && !t.includes("T-001"),
      () => raw("j".repeat(11)));
    await run("keyboard-detail-open", (t) => t.includes("Spec") && t.includes("T-012"), () => raw("\r"));
    await run("keyboard-detail-scroll", (t) => t.includes("Spec") &&
      t.includes("Fixture body line 030") && !t.includes("Fixture body line 001"), () => raw("j".repeat(10)));
    await run("keyboard-detail-back", board, () => raw("\x1b"));
    await run("keyboard-board-up", (t) => board(t) && t.includes("T-001"), () => raw("k".repeat(11)));
    await run("keyboard-capture-open", (t) => t.includes("Cancel") && t.includes("save as"), () => raw("n"));
    await run("keyboard-capture-close", board, () => raw("\x1b"));
    await run("keyboard-search-open", (t) => t.includes("results") && t.includes("T-110"), () => raw("/"));
    await run("keyboard-search-type", (t) => t.includes("status:done") && t.includes("results 85") &&
      !t.includes("T-001"), () => raw("status:done"));
    await run("keyboard-search-scroll", (t) => t.includes("results") && t.includes("T-067") && !t.includes("T-097"),
      () => raw("\x1b[6~".repeat(3)));
    await run("keyboard-search-back", board, () => raw("\x1b"));
    await run("keyboard-history-open", (t) => t.includes("85 done") && t.includes("4 dropped"), () => raw("Y"));
    await run("keyboard-history-back", board, () => raw("\x1b"));
    await run("mouse-column-right", (t) => board(t) && !t.includes("▌"), () => click("Ready"));
    await run("mouse-column-left", (t) => board(t) && t.includes("▌ T-001"), () => click("Backlog"));
    await run("mouse-board-scroll", (t) => board(t) && !t.includes("T-001"), () => wheel(true, 8, 12, 3));
    await run("mouse-board-up", (t) => board(t) && t.includes("T-001"), () => wheel(false, 8, 12, 10));
    // Selected card: one press/release opens it, no keyboard navigation.
    await run("mouse-detail-open", (t) => t.includes("Spec") && t.includes("T-001"), () => click("T-001"));
    await run("mouse-detail-scroll", (t) => t.includes("Spec") &&
      t.includes("Fixture body line 030") && !t.includes("Fixture body line 001"),
      () => wheel(true, 8, 18, 4));
    await run("mouse-detail-back", board, () => click("‹ back"));
    await run("mouse-capture-open", (t) => t.includes("Cancel") && t.includes("save as"), () => click("n new"));
    await run("mouse-capture-close", board, () => click("Cancel"));
    await run("mouse-search-open", (t) => t.includes("results"), () => click("/ search"));
    await run("mouse-search-scroll", (t) => t.includes("results") && t.includes("T-080") && !t.includes("T-110"),
      () => wheel(true, 60, 15, 36));
    await run("mouse-search-back", board, () => click("‹ back"));
    await run("mouse-history-open", (t) => t.includes("85 done") && t.includes("4 dropped"),
      () => click("Y history"));
    await run("mouse-history-back", board, () => click("‹ back"));
    // No input/refresh key: observe the real two-second background sync seeing a task edit.
    await stage(`${condition}-background-task-refresh`, (t) => board(t) && t.includes(`Fresh ${condition}`),
      async () => {
        const task = structuredClone(workload.tasks[109]);
        task.title = `Fresh ${condition}`;
        await saveTask(task, workload.bodies[109]);
      }, condition);
    // State + event only, no task-file edit. Verify the Review card's unseen-artifact count.
    await stage(`${condition}-background-review-refresh`, (t) => board(t) &&
      t.includes(condition === "idle" ? "3 new" : "6 new"), async () => {
      await writeFile(join(fixture, ".corral/state/reviewed-artifacts.json"),
        JSON.stringify(condition === "idle" ? workload.tasks[109].artifacts.slice(0, 3).map((a) => a.path) : []));
      await appendFile(join(fixture, ".corral/events.jsonl"), JSON.stringify({
        ts: at, task: "T-110", kind: "review_started",
        detail: { run: "R3", reviewer: "fixture", condition },
      }) + "\n");
    }, condition);
  }
  await stage("concurrent-background-status-refresh", (t) => board(t) &&
    /Ready [─ ]+1 ╮/.test(t) && t.includes("Moved ready"), async () => {
    const task = structuredClone(workload.tasks[11]);
    task.status = "ready";
    task.title = "Moved ready";
    await saveTask(task, workload.bodies[11]);
  }, "concurrent");
  await stage("concurrent-background-needsme-refresh", (t) => board(t) &&
    t.includes("▲ 1 needs you") && t.includes("Owner decision"), async () => {
    const task = structuredClone(workload.tasks[106]);
    task.status = "needs_me";
    task.title = "Owner decision";
    await saveTask(task, workload.bodies[106]);
  }, "concurrent");
  const current = JSON.parse(await cli("list", "--all", "--json"));
  assert.equal(current.find((t) => t.id === "T-012").status, "ready");
  assert.equal(current.find((t) => t.id === "T-107").status, "needs_me");
  result.currentStatusChecks = "PASS: Ready and Needs me transitions visible and persisted";
  await raw("q");
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) {
    try { await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read); }
    catch (error) { result.captureError = String(error); }
  }
} finally {
  subscription?.destroy();
  if (viewport) {
    viewport.stdin.end();
    viewport.kill();
    await viewport.exited;
    await viewportOutput;
    result.cleanup.viewport = await viewportErrors;
  }
  if (workspace) {
    try {
      await herdr("workspace", "close", workspace);
      result.cleanup.workspaceClosed = workspace;
    } catch (error) { result.cleanup.error = String(error); result.status = "FAIL"; }
  }
  result.cleanup.fixtureRetained = fixture;
  result.cleanup.server = "Owned exclusively by scripts/test-herdr.sh; shell EXIT trap stops it and removes private HOME/XDG/socket.";
  result.cleanup.agentsLaunched = 0;
  result.summary = Object.fromEntries(["idle", "concurrent"].map((condition) => {
    const values = result.samples.filter((s) => s.condition === condition).map((s) => s.ms).sort((a, b) => a - b);
    return [condition, { count: values.length, medianMs: values[Math.floor(values.length / 2)],
      p95Ms: values[Math.ceil(values.length * .95) - 1], maxMs: values.at(-1) }];
  }));
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(JSON.stringify({ status: result.status, summary: result.summary, evidence }));
if (result.error) console.error(result.error);
process.exitCode = result.status === "PASS" ? 0 : 1;
