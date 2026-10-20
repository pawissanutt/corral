// Closing acceptance for T-166. Run inside scripts/test-herdr.sh's private environment.
// Unlike timeline.mjs, every event here is produced by a real operation, not seeded.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (process.argv[2] === "--edit") {
  const path = process.argv[3];
  const before = await readFile(path, "utf8");
  assert(before.includes("title: Task history"));
  await writeFile(path, before.replace("title: Task history", "title: Task history log")
    + "\n## Acceptance\nShow actor, via, edited fields, and the resolved wait.\n");
  process.exit(0);
}

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-166/live-120");
const columns = Number(process.argv[4] ?? 120);
assert([80, 120].includes(columns));
assert(process.env.HOME?.startsWith("/tmp/"), "Use scripts/test-herdr.sh");
await mkdir(evidence, { recursive: true });
const root = await mkdtemp("/tmp/corral-task-history-");
const app = join(root, "app");
const env = { ...process.env, CORRAL_ROOT: root, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_PM", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"])
  delete env[key];
const transcript = [];
const result = { status: "FAIL", binary, columns, scenes: [] };
const owned = new Set();
let socketPath, pane, subscription;
let serial = 0;
let onOutput = () => {};
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";

async function command(program, args, cwd = root, overrides = {}) {
  const child = Bun.spawn([program, ...args], {
    cwd, env: { ...env, ...overrides }, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ command: [program, ...args], cwd, overrides, status, stdout, stderr });
  assert.equal(status, 0, `${program} ${args.join(" ")}: ${stderr}`);
  return stdout;
}
const cli = (...args) => command(binary, ["--root", root, ...args]);
const json = async (...args) => JSON.parse(await cli(...args));
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);
async function task() {
  const value = await json("show", "T-001", "--json");
  for (const run of value.runs ?? [])
    if (run.herdr) owned.add(run.herdr.workspace_id);
  return value;
}

function rpc(method, params) {
  const id = `task-history-${++serial}`;
  return new Promise((accept, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    const timer = setTimeout(() => {
      connection.destroy();
      reject(new Error(`${method}: timeout`));
    }, 20000);
    connection.setEncoding("utf8");
    let buffer = "";
    connection.on("data", (chunk) => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(buffer.slice(0, end));
        buffer = buffer.slice(end + 1);
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

async function capture(name, _read, check = () => true) {
  const ansi = (await rpc("pane.read", {
    pane_id: pane, source: "visible", format: "ansi", strip_ansi: false,
  })).read.text;
  const text = Bun.stripANSI(ansi).replaceAll("\r", "");
  assert(check(text), `${name}: actual ANSI screen must show the requested state`);
  assert.match(ansi, /\x1b\[[0-9;]*(?:38|48);[25];/, "Retain real theme colors");
  for (const line of text.split("\n"))
    assert(Bun.stringWidth(line) <= columns, `${name}: row overflow`);
  await writeFile(join(evidence, `${name}.txt`), text);
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  result.scenes.push(name);
  return text;
}

async function stage(name, marker, check, action, save = true) {
  let timer;
  const matched = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 20000);
    onOutput = (read) => {
      if (check(read.text)) {
        onOutput = () => {};
        if (save) capture(name, read, check).then(accept, reject);
        else accept(read.text);
      }
    };
  });
  matched.catch(() => {});
  try {
    await rpc("events.subscribe", { subscriptions: [{
      type: "pane.output_matched", pane_id: pane, source: "visible",
      match: { type: "regex", value: marker }, strip_ansi: true,
    }] });
    await action();
    return await matched;
  } finally { clearTimeout(timer); onOutput = () => {}; }
}

async function clickEvent(screen, kind) {
  const lines = screen.split("\n");
  const pattern = new RegExp(`\\d\\d:\\d\\d\\s+${kind}\\s+`);
  const y = lines.findIndex((line) => pattern.test(line));
  assert(y >= 0, `Missing event row: ${kind}`);
  const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(kind))) + 1;
  transcript.push({ mouse: kind, x, y: y + 1 });
  await raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
}

async function openBoard(tour) {
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", app,
    "--label", "T-166 first-day acceptance", "--no-focus")).result;
  owned.add(created.workspace.workspace_id);
  pane = created.root_pane.pane_id;
  const launch = () => herdr("pane", "run", pane,
    `stty -F /dev/tty cols ${columns} rows 30; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(binary)} --root ${quote(root)} board </dev/null`);
  if (tour) {
    await stage("01-tour", "skip tour", (s) => s.includes("skip tour"), launch);
    await stage("02-board", "No tasks yet", (s) => s.includes("No tasks yet") && !s.includes("skip tour"),
      () => raw("\x1b"));
  } else {
    await stage("06-landed-board", "/ search", (s) => s.includes("/ search"), launch);
  }
  return created.workspace.workspace_id;
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.includes("/sessions/corral-test/"), "Refuse owner-configured server");
  await mkdir(app);
  await command("git", ["init", "-q", "-b", "main"], app);
  await command("git", ["config", "commit.gpgsign", "false"], app);
  await writeFile(join(app, "initial.txt"), "initial\n");
  await command("git", ["add", "-A"], app);
  await command("git", ["commit", "-qm", "Initial fixture"], app);
  await cli("init");
  for (const [key, value] of [
    ["resources.max_load_per_core", "1.7976931348623157e308"],
    ["resources.min_mem_available", "1B"],
    ["resources.min_disk_free", "1B"],
    ["dispatch.paused", "true"],
    ["review.autopilot", "false"],
    ["review.accept", "owner"],
    ["review.gate", JSON.stringify(["git diff --exit-code", "test -f answer.txt"])],
    // A shell function is a deterministic stand-in for a model receiving a prompt.
    // No fixture process reads the terminal or depends on a fixed sleep.
    ["agents.history-worker.command",
      "jsonl() { printf 'JSONL-RECEIVED\\n'; }; printf 'HISTORY-WORKER-READY\\n'"],
    ["agents.history-reviewer.command", "printf 'HISTORY-REVIEWER-READY\\n'"],
    ["review.reviewer", "history-reviewer"],
  ]) await cli("config", "set", key, value);

  const boardWorkspace = await openBoard(true);
  await stage("03-capture", "what's the idea", (s) => s.includes("what's the idea"), () => raw("c"));
  await stage("04-capture-title", "Task history", (s) => s.includes("Task history"),
    () => raw("Task history"));
  await stage("05-captured", "T-001 captured", (s) => s.includes("T-001 captured"), () => raw("\r"));
  assert.equal((await task()).status, "idea");
  await herdr("workspace", "close", boardWorkspace);
  owned.delete(boardWorkspace);
  subscription?.destroy();

  await command(binary, ["--root", root, "edit", "T-001", "--owner-ok"], root,
    { EDITOR: `bun ${fileURLToPath(import.meta.url)} --edit` });
  await command(binary, ["--root", root, "move", "T-001", "ready", "--author", "pm",
    "--reason", "fits sprint"], root, { CORRAL_PM: "1" });
  await cli("config", "set", "dispatch.paused", "false");
  await cli("dispatch", "T-001", "--profile", "history-worker");
  let current = await task();
  const run = current.runs.at(-1);
  const worktree = run.worktree;
  const workerPane = run.herdr.pane_id;
  const decision = {
    version: 1, id: "storage-format", run: run.id, question: "jsonl or sqlite?",
    requested_action: "Answer jsonl or sqlite", owner: "owner", source: "worker", deadline: null,
  };
  const request = join(root, "decision.json");
  await writeFile(request, JSON.stringify(decision));
  await command(binary, ["--root", root, "run", "decision", "T-001", "--file", request], worktree);
  assert.equal((await task()).status, "needs_me");
  // Subscribe to the real worker output before triggering CLI delivery.
  const boardPane = pane;
  pane = workerPane;
  await stage("worker-answer", "JSONL-RECEIVED", (s) => s.includes("JSONL-RECEIVED"),
    () => cli("reply", "T-001", "jsonl"), false);
  pane = boardPane;
  current = await task();
  assert.equal(current.status, "running");
  assert.equal(current.runs.at(-1).obligations.decisions[0].receipt.answer, "jsonl");
  await writeFile(join(worktree, "answer.txt"), "jsonl\n");
  await command("git", ["add", "answer.txt"], worktree);
  await command("git", ["commit", "-qm", "Record storage decision"], worktree);
  const progress = JSON.parse(await command(binary,
    ["--root", root, "run", "progress", "T-001", "--json"], worktree));
  const checkpoint = join(root, "checkpoint.json");
  await writeFile(checkpoint, JSON.stringify({
    task: progress.task, run: progress.run, spec_revision: progress.spec_revision,
    head: progress.head, base: progress.base, tree: progress.tree,
    expected_revision: 0, progress_revision: progress.progress?.revision ?? 0,
    criterion_ids: [], evidence_refs: [], next_action: "Submit first-day fixture for review",
    decisions: [], waits: [], resources: [],
  }));
  await command(binary, ["--root", root, "run", "checkpoint", "T-001", "--file", checkpoint], worktree);
  await command(binary, ["--root", root, "done", "T-001", "--summary",
    "Recorded the owner's jsonl answer.\nVerified: real CLI reply delivered; answer.txt committed.\nNot verified: none"], worktree);
  await command(binary, ["--root", root, "gate", "T-001", "--wait"], worktree);
  await cli("review-run", "T-001");
  current = await task();
  const notes = join(root, "review-notes.md");
  await writeFile(notes, "Fixture reviewed: answer is committed and the actual gate passed.\n");
  const review = current.runs.at(-1).review;
  const attempt = `${review.identity.attempt}:${review.route.callback ?? review.route.attempt}`;
  await command(binary, ["--root", root, "verdict", "T-001", "approve",
    "--attempt", attempt, "--notes-file", notes], worktree);
  await cli("approve", "T-001");
  current = await task();
  assert.equal(current.status, "done");
  assert.equal(await readFile(join(app, "answer.txt"), "utf8"), "jsonl\n");
  result.landed = (await command("git", ["rev-parse", "main"], app)).trim();

  const entries = (await cli("log", "T-001", "--json")).trim().split("\n").map(JSON.parse);
  const text = await cli("log", "T-001");
  await writeFile(join(evidence, "timeline.jsonl"), entries.map((e) => JSON.stringify(e)).join("\n") + "\n");
  await writeFile(join(evidence, "timeline.txt"), text);
  await writeFile(join(evidence, "events.jsonl"), await readFile(join(root, ".corral", "events.jsonl")));
  const one = (kind) => {
    const found = entries.filter((entry) => entry.kind === kind);
    assert.equal(found.length, 1, `Exactly one ${kind}`);
    return found[0];
  };
  for (const [kind, actor, via] of [
    ["created", "owner", "tui"], ["edited", "owner", "cli"], ["moved", "pm", "pm"],
    ["dispatched", "owner", "cli"], ["needs_me", "agent", "run:R1"],
    ["replied", "owner", "cli"], ["done", "agent", "run:R1"],
    ["gate", "agent", "run:R1"], ["verdict", "reviewer", "run:R1"],
    ["approved", "owner", "cli"],
  ]) {
    const entry = one(kind);
    assert.equal(entry.actor, actor, `${kind} actor`);
    assert.equal(entry.via, via, `${kind} via`);
    assert(text.includes(kind), `${kind} in text timeline`);
  }
  assert.deepEqual(one("edited").event.detail.fields.title, { from: "Task history", to: "Task history log" });
  const body = one("edited").event.detail.fields.body;
  assert(body.lines_to > body.lines_from);
  assert.match(body.sha_from, /^[0-9a-f]{12}$/);
  assert.match(body.sha_to, /^[0-9a-f]{12}$/);
  assert.notEqual(body.sha_from, body.sha_to);
  const opened = one("needs_me");
  const closed = one("replied");
  assert.equal(closed.wait.opened_at, opened.at);
  assert.equal(closed.wait.open, false);
  const elapsed = Date.parse(closed.at) - Date.parse(opened.at);
  assert(Math.abs(closed.wait.duration.secs * 1000 + closed.wait.duration.nanos / 1e6 - elapsed) < 1);
  assert(text.includes("┐ waited") && text.includes("┘ waited"));
  const ordered = ["created", "edited", "moved", "dispatched", "needs_me", "replied",
    "done", "gate", "verdict", "approved"].map((kind) => entries.indexOf(one(kind)));
  assert(ordered.every((index, i) => i === 0 || index > ordered[i - 1]),
    "The first-day operations must appear in causal order");
  result.wait = closed.wait;
  result.attribution = "PASS: ten real operation kinds have the expected actor/via";
  result.edited = one("edited").event.detail.fields;

  await openBoard(false);
  await stage("07-search", "results", (s) => s.includes("results"), () => raw("/"));
  await stage("08-query", "id:T-001", (s) => s.includes("id:T-001"), () => raw("id:T-001"));
  await stage("09-spec", "7 Timeline", (s) => s.includes("7 Timeline"), () => raw("\r"));
  const screen = await stage("10-timeline", "Relevant events", (s) =>
    s.includes("Relevant events") && /\d\d:\d\d\s+needs_me\s+agent/.test(s), () => raw("7"));
  for (const kind of ["created", "edited", "moved", "dispatched", "needs_me", "replied",
    "done", "gate", "verdict"])
    assert(new RegExp(`\\d\\d:\\d\\d\\s+${kind}\\s+`).test(screen), `${kind} visible in Timeline`);
  assert(screen.includes("┐ waited") && screen.includes("┘ waited"), "Both wait endpoints visible");
  assert(screen.includes("title") && screen.includes("body"), "Changed field names visible");
  assert(screen.includes("reviewer") && screen.includes("run:R1") && screen.includes("pm"));
  await stage("11-raw-created", "Raw event", (s) =>
    s.includes("Raw event") && s.includes('"kind": "created"'), () => raw("\r"));
  const top = await stage("12-return", "Relevant events", (s) =>
    s.includes("Relevant events") && !s.includes("Raw event"), () => raw("\x1b"));
  await stage("13-raw-edited", "Raw event", (s) =>
    s.includes("Raw event") && s.includes('"kind": "edited"') &&
      s.includes('"sha_to"') && s.includes('"from": "Task history"'),
    async () => { await clickEvent(top, "edited"); await raw("\r"); });
  await stage("15-edit-closed", "Relevant events", (s) =>
    s.includes("Relevant events") && !s.includes("Raw event"), () => raw("\x1b"));
  await stage("16-timeline-bottom", "approved +owner +cli", (s) =>
    /\d\d:\d\d\s+approved\s+owner\s+cli\s+R1\s+landed/.test(s),
    () => raw("\x1b[6~"));
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) {
    try { await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read); }
    catch (error) { result.captureError = String(error); }
  }
} finally {
  subscription?.destroy();
  const live = new Set(JSON.parse(await herdr("workspace", "list")).result.workspaces
    .map((workspace) => workspace.workspace_id));
  for (const workspace of owned) {
    if (!live.has(workspace)) continue; // Landing already closes the run workspace.
    try { await herdr("workspace", "close", workspace); }
    catch (error) { result.cleanupError = String(error); result.status = "FAIL"; }
  }
  await rm(root, { recursive: true, force: true });
  result.cleanup = "Closed only fixture workspaces and removed the disposable board and repository";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(JSON.stringify(result, null, 2));
if (result.status !== "PASS") process.exitCode = 1;
