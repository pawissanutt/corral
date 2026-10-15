// Real CLI + keyboard/mouse stage acceptance in corral-test. Subscribe before every trigger.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { watch } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile, readFile } from "node:fs/promises";
import { resolve, join, dirname, basename } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-067/live");
const columns = Number(process.argv[4] ?? 120);
assert(Number.isInteger(columns) && columns >= 80 && columns <= 200);
await mkdir(evidence, { recursive: true });
const root = await mkdtemp("/tmp/corral-stages-");
const env = { ...process.env, CORRAL_ROOT: root, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const transcript = [];
const result = { status: "FAIL", binary, columns, session: "corral-test", scenes: [] };
const owned = new Set();
let socketPath, pane, subscription;
let serial = 0;
let onOutput = () => {};
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";

async function command(program, args, cwd = root) {
  const child = Bun.spawn([program, ...args], { cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ command: [program, ...args], status, stdout, stderr });
  assert.equal(status, 0, `${program} ${args.join(" ")}: ${stderr}`);
  return stdout;
}
const cli = (...args) => command(binary, ["--root", root, ...args]);
const json = async (...args) => JSON.parse(await cli(...args));
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);
async function task() {
  const value = await json("show", "T-001", "--json");
  for (const run of value.runs ?? []) if (run.herdr) owned.add(run.herdr.workspace_id);
  return value;
}
function rpc(method, params) {
  const id = `stages-${++serial}`;
  return new Promise((accept, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    const timer = setTimeout(() => { connection.destroy(); reject(new Error(`${method}: timeout`)); }, 20000);
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
async function capture(name, read) {
  await writeFile(join(evidence, `${name}.txt`), read.text);
  await writeFile(join(evidence, `${name}.json`), JSON.stringify(read, null, 2) + "\n");
  const styled = (await rpc("pane.read", {
    pane_id: pane, source: "visible", format: "ansi", strip_ansi: false,
  })).read.text;
  assert.match(styled, /\x1b\[[0-9;]*(?:38|48);[25];/, "Captured screen must retain theme colors");
  await writeFile(join(evidence, `${name}.ansi`), styled);
  result.scenes.push(name);
  return read.text;
}
async function stage(name, marker, check, action) {
  let timer;
  const matched = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 20000);
    onOutput = (read) => {
      if (check(read.text)) {
        onOutput = () => {};
        capture(name, read).then(accept, reject);
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
async function signal(path, action) {
  let timer;
  let watcher;
  const seen = new Promise((accept, reject) => {
    watcher = watch(dirname(path), async (_event, file) => {
      if (String(file) === basename(path)) {
        try { await readFile(path); accept(); } catch { /* wait for atomic creation */ }
      }
    });
    timer = setTimeout(() => reject(new Error(`No signal: ${path}`)), 20000);
  });
  seen.catch(() => {});
  try { await action(); await seen; } finally { watcher.close(); clearTimeout(timer); }
}
async function click(text, needle) {
  const lines = text.split("\n");
  const y = lines.findIndex((line) => line.includes(needle));
  assert(y >= 0, `Missing mouse target ${needle}\n${text}`);
  const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1;
  transcript.push({ mouse: needle, x, y: y + 1 });
  await raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
}
async function checkpoint() {
  const p = await json("run", "progress", "T-001", "--json");
  const input = {
    task: p.task, run: p.run, spec_revision: p.spec_revision,
    head: p.head, base: p.base, tree: p.tree,
    expected_revision: 0, progress_revision: p.progress?.revision ?? 0,
    criterion_ids: [], evidence_refs: [], next_action: "Submit fixture for stage monitoring",
    decisions: [], waits: [], resources: [],
  };
  const file = join(root, "checkpoint.json");
  await writeFile(file, JSON.stringify(input));
  await cli("run", "checkpoint", "T-001", "--file", file);
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.includes("/sessions/corral-test/"), "Refuse non-isolated herdr socket");
  const app = join(root, "app");
  await mkdir(app);
  for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "Stage QA"],
    ["config", "user.email", "stages@test.invalid"], ["config", "commit.gpgsign", "false"]])
    await command("git", args, app);
  await writeFile(join(app, "initial.txt"), "initial\n");
  await command("git", ["add", "-A"], app);
  await command("git", ["commit", "-qm", "initial"], app);
  await cli("init");
  await cli("config", "set", "agents.fake-worker.command", "printf 'WORKER-LIVE\\n'");
  await cli("config", "set", "harnesses.fake-reviewer.command",
    `printf 'REVIEWER-LIVE %s\\n' {model_args}; printf ready > ${quote(join(root, "review.ready"))}; cat ${quote(join(root, "review.release"))} >/dev/null`);
  await cli("config", "set", "harnesses.fake-reviewer.flags", "claude");
  await cli("config", "set", "profiles.sonnet.harness", "fake-reviewer");
  await cli("config", "set", "profiles.sonnet.model", "sonnet");
  await cli("config", "set", "review.reviewer", "sonnet");
  await cli("config", "set", "review.accept", "owner");
  await cli("config", "set", "review.autopilot", "false");
  const middle = `if [ -f ${quote(join(root, "pass"))} ]; then printf 'CLIPPY-PASS\\n'; else printf 'GATE-LIVE-STEP\\n'; printf ready > ${quote(join(root, "gate.ready"))}; cat ${quote(join(root, "gate.release"))} >/dev/null; printf 'error: original gate failure\\n'; exit 7; fi`;
  await cli("config", "set", "review.gate", JSON.stringify(["printf 'FMT-PASS\\n'", middle, "printf 'TESTS-PASS\\n'"]));
  await command("mkfifo", [join(root, "gate.release"), join(root, "review.release")]);
  assert.equal((await cli("new", "Watch every stage", "-p", "app", "--ready")).trim(), "T-001");
  await cli("dispatch", "T-001", "--profile", "fake-worker");
  const launched = await task();
  const worktree = launched.runs.at(-1).worktree;
  await writeFile(join(worktree, "widget.txt"), "clockwise\n");
  await command("git", ["add", "-A"], worktree);
  await command("git", ["commit", "-qm", "widget spins"], worktree);
  await checkpoint();
  await cli("done", "T-001", "--summary", "Worker finished; widget spins. Verified: fixture commit. Not verified: none.");
  await signal(join(root, "gate.ready"), () => cli("gate", "T-001"));
  const gatePane = (await task()).runs.at(-1).gate.pane;
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", root, "--label", "T-067 stage acceptance", "--no-focus")).result;
  owned.add(created.workspace.workspace_id);
  pane = created.root_pane.pane_id;
  result.pane = pane;
  await stage("01-tour", "skip tour", (s) => s.includes("skip tour"),
    () => herdr("pane", "run", pane, `stty -F /dev/tty cols ${columns} rows 30; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(binary)} --root ${quote(root)} board </dev/null`));
  let screen = await stage("02-board-gating", "q (leave|quit)", (s) =>
    s.includes("gating") && s.includes("2/3") && !s.includes("welcome to corral") && s.includes("q leave"),
    () => raw("\x1b"));
  screen = await stage("03-review-preview", "Worker finished", (s) => s.includes("Worker finished"),
    async () => { await click(screen, "T-001"); await click(screen, "T-001"); });
  screen = await stage("04-review-strip", "a approve", (s) => s.includes("F2 Gate") && s.includes("Request changes"),
    () => raw("\x1b"));
  screen = await stage("05-gate-live-keyboard", "GATE-LIVE-STEP *│", (s) =>
    s.includes("GATE-LIVE-STEP") && s.includes("[running]") && s.includes("[pending]"),
    () => raw("\x1bOQ"));
  screen = await stage("06-gate-command-mouse", "FMT-PASS *│", (s) => s.includes("FMT-PASS"),
    () => click(screen, "1/3 [passed]"));
  screen = await stage("07-gate-running-mouse", "GATE-LIVE-STEP *│", (s) => s.includes("GATE-LIVE-STEP"),
    () => click(screen, "2/3 [running]"));
  screen = await stage("08-gate-failure-jump", "error: original gate failure", (s) =>
    s.includes("error: original gate failure") && s.includes("[failed]"),
    () => writeFile(join(root, "gate.release"), "release\n"));
  await writeFile(join(root, "pass"), "pass\n");
  await cli("gate", "T-001", "--wait");
  screen = await stage("09-gate-latest-pass", "TESTS-PASS *│", (s) => s.includes("TESTS-PASS") && s.includes("attempt 2"),
    () => click(screen, "Latest"));
  // First gate tab closed by gate_start, but its command log remains accessible.
  assert(!(JSON.parse(await herdr("pane", "list")).result.panes ?? []).some((p) => p.pane_id === gatePane));
  screen = await stage("10-gate-history-mouse", "original gate failure", (s) =>
    s.includes("original gate failure") && s.includes("attempt 1"),
    () => click(screen, "Previous round"));
  await signal(join(root, "review.ready"), () => cli("review-run", "T-001"));
  const firstReviewer = (await task()).runs.at(-1).review.pane;
  screen = await stage("10b-review-return", "a approve", (s) => s.includes("Request changes"),
    () => raw("\x1b"));
  screen = await stage("10c-board-reviewing", "sonnet", (s) =>
    (s.includes("reviewing:") || s.includes("review:")) && s.includes("sonnet"),
    () => raw("\x1b"));
  await stage("10d-review-preview", "Worker finished", (s) => s.includes("Worker finished"),
    () => raw("\r"));
  screen = await stage("10e-review-strip", "a approve", (s) => s.includes("Request changes"),
    () => raw("\x1b"));
  screen = await stage("11-reviewer-live-mouse", "REVIEWER-LIVE", (s) =>
    s.includes("REVIEWER-LIVE") && s.includes("Model: sonnet") && s.includes("live screen"),
    () => click(screen, "F3 Review"));
  const before = (await rpc("pane.read", { pane_id: firstReviewer, source: "recent_unwrapped" })).read.text;
  await stage("12-read-only-barrier", "Gate - read-only", (s) => s.includes("Gate - read-only"),
    () => raw("z\x1bOQ"));
  const after = (await rpc("pane.read", { pane_id: firstReviewer, source: "recent_unwrapped" })).read.text;
  assert.equal(after, before, "Stage keyboard input must not reach reviewer");
  result.read_only = "PASS: arbitrary typing + stage barrier left reviewer screen unchanged";
  screen = await stage("13-reviewer-keyboard", "REVIEWER-LIVE", (s) => s.includes("REVIEWER-LIVE"),
    () => raw("\x1bOR"));
  const notes = join(root, "notes.md");
  await writeFile(notes, "## Earlier verdict notes\n\n- Keep the widget shape.\n");
  screen = await stage("14-reviewer-verdict", "Earlier verdict notes", (s) =>
    s.includes("Earlier verdict notes") && s.includes("Verdict: changes"),
    () => cli("verdict", "T-001", "changes", "--notes-file", notes));
  await rm(join(root, "review.ready"));
  await signal(join(root, "review.ready"), () => cli("review-run", "T-001"));
  const secondReviewer = (await task()).runs.at(-1).review.pane;
  screen = await stage("15-reviewer-latest", "reviewer 2", (s) =>
    s.includes("reviewer 2") && s.includes("REVIEWER-LIVE"),
    () => click(screen, "Latest"));
  screen = await stage("16-reviewer-history-closed", "Earlier verdict notes", (s) =>
    s.includes("Earlier verdict notes") && s.includes("retained transcript / screen") && s.includes("REVIEWER-LIVE"),
    () => raw("["));
  await writeFile(notes, "## Approved verdict notes\n\nThe widget is ready to land.\n");
  await cli("verdict", "T-001", "approve", "--notes-file", notes);
  await herdr("pane", "close", secondReviewer);
  screen = await stage("17-reviewer-final-retained", "Approved verdict notes", (s) =>
    s.includes("Approved verdict notes") && s.includes("Verdict: approve") && s.includes("REVIEWER-LIVE"),
    () => click(screen, "Latest"));
  screen = await stage("18-land", "Landing", (s) => s.includes("Landing has not started"),
    () => click(screen, "F4 Land"));
  screen = await stage("19-agent", "Worker finished", (s) => s.includes("Worker finished") && s.includes("saved summary"),
    () => click(screen, "F1 Agent"));
  screen = await stage("20-review-return", "a approve", (s) => s.includes("Request changes"),
    () => raw("\x1b"));
  await raw("\x1b"); // board
  await stage("21-task-search", "search · all statuses", (s) =>
    s.includes("search · all statuses") && s.includes("T-001"),
    () => raw("/T-001"));
  screen = await stage("22-task-detail", "empty spec", (s) =>
    s.includes("(empty spec)") && s.includes("F1 Agent") && s.includes("Live"),
    () => raw("\r"));
  screen = await stage("23-detail-gate-mouse", "Gate - read-only", (s) =>
    s.includes("Gate - read-only") && s.includes("[passed]"),
    () => click(screen, "F2 Gate"));
  await stage("24-task-detail-return", "empty spec", (s) =>
    s.includes("(empty spec)") && s.includes("F1 Agent") && s.includes("Live"),
    () => raw("\x1b"));
  await raw("\x1b"); // search
  await raw("\x1b"); // board
  await raw("q");
  result.history = "PASS: failed/pass gate attempts and changes/approve reviewers survive closed tabs";
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) try { await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read); }
  catch (captureError) { result.captureError = String(captureError); }
} finally {
  subscription?.destroy();
  for (const workspace of owned) {
    try { await herdr("workspace", "close", workspace); }
    catch (error) { result.cleanupError = String(error); result.status = "FAIL"; }
  }
  await rm(root, { recursive: true, force: true });
  result.cleanup = "Closed only created corral-test workspaces and removed fixture";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(JSON.stringify(result, null, 2));
if (result.status !== "PASS") process.exitCode = 1;
