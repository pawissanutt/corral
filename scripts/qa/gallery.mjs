// Run inside the plugin-free scripts/test-herdr.sh environment. No sleeps: subscribe before input.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { watch } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile, copyFile, rm } from "node:fs/promises";
import { resolve, join } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-024/live");
await mkdir(evidence, { recursive: true });
const root = await mkdtemp("/tmp/corral-gallery-");
const env = { ...process.env, CORRAL_ROOT: root, CORRAL_HERDR_SESSION: "corral-test" };
for (const name of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_WORKSPACE_ID", "HERDR_TAB_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[name];
const commands = [];
const result = { status: "FAIL", columns: 80, rows: 24, scenes: [], checks: [], cleanup: "" };
const owned = new Set();
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
let socket, pane, subscription, viewport, viewportOutput, viewportErrors, onOutput = () => {}, serial = 0;

async function command(program, args, cwd = root) {
  const child = Bun.spawn([program, ...args], { cwd, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  commands.push({ command: [program, ...args], status, stdout, stderr });
  assert.equal(status, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return stdout;
}
const cli = (...args) => command(binary, ["--root", root, ...args]);
const json = async (...args) => JSON.parse(await cli(...args));
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);
async function task() {
  const task = await json("show", "T-001", "--json");
  const run = task.runs?.at(-1);
  if (run?.herdr) owned.add(run.herdr.workspace_id);
  return task;
}
function rpc(method, params) {
  const id = `gallery-${++serial}`;
  return new Promise((accept, reject) => {
    const connection = createConnection(socket);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    const timer = setTimeout(() => { connection.destroy(); reject(new Error(`${method}: timeout`)); }, 20000);
    connection.setEncoding("utf8");
    let pending = "";
    connection.on("data", (chunk) => {
      pending += chunk;
      let end;
      while ((end = pending.indexOf("\n")) >= 0) {
        const message = JSON.parse(pending.slice(0, end));
        pending = pending.slice(end + 1);
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
async function resize(columns, rows) {
  if (viewport) {
    viewport.kill();
    await viewport.exited;
    await viewportOutput;
    await viewportErrors;
  }
  viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
    "control", pane, "--cols", String(columns), "--rows", String(rows)],
    { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  viewportErrors = new Response(viewport.stderr).text();
  const reader = viewport.stdout.getReader();
  let timer;
  try {
    const first = await Promise.race([
      reader.read(),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Viewport not ready")), 20000); }),
    ]);
    assert(!first.done, "Owned viewport controller must acknowledge size");
  } finally { clearTimeout(timer); }
  viewportOutput = (async () => { while (!(await reader.read()).done) {} })();
}
async function capture(name, text) {
  await writeFile(join(evidence, `${name}.txt`), text);
  const styled = (await rpc("pane.read", {
    pane_id: pane, source: "visible", format: "ansi", strip_ansi: false,
  })).read.text;
  await writeFile(join(evidence, `${name}.ansi`), styled);
  result.scenes.push({ name, columns: result.columns, rows: result.rows });
  return text;
}
async function stage(name, marker, check, action) {
  let timer;
  const observed = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 20000);
    onOutput = (read) => {
      if (check(read.text)) {
        onOutput = () => {};
        capture(name, read.text).then(accept, reject);
      }
    };
  });
  observed.catch(() => {});
  try {
    await rpc("events.subscribe", { subscriptions: [{
      type: "pane.output_matched", pane_id: pane, source: "visible",
      match: { type: "regex", value: marker }, strip_ansi: true,
    }] });
    await action();
    return await observed;
  } finally { clearTimeout(timer); onOutput = () => {}; }
}
async function click(text, needle) {
  const lines = text.split("\n");
  const y = lines.findIndex((line) => line.includes(needle));
  assert(y >= 0, `Missing ${needle}\n${text}`);
  const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1;
  await raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
}
async function checkpoint() {
  const progress = await json("run", "progress", "T-001", "--json");
  const file = join(root, "checkpoint.json");
  await writeFile(file, JSON.stringify({
    task: progress.task, run: progress.run, spec_revision: progress.spec_revision,
    head: progress.head, base: progress.base, tree: progress.tree,
    expected_revision: progress.checkpoints.at(-1)?.checkpoint.revision ?? 0,
    progress_revision: progress.progress?.revision ?? 0,
    criterion_ids: [], evidence_refs: [], next_action: "Submit the generated report",
    decisions: [], waits: [], resources: [],
  }));
  await cli("run", "checkpoint", "T-001", "--file", file);
}
async function finishReport(version) {
  const worktree = (await task()).runs.at(-1).worktree;
  const html = `<!doctype html><html lang="en"><meta charset="utf-8"><title>Build health</title>
  <style>body{margin:0;background:#111827;color:#f9fafb;font:24px sans-serif;padding:70px}
  h1{font-size:60px;margin:0 0 30px}p{color:#a5b4fc}.bars{display:flex;gap:24px;align-items:end;height:330px}
  .bars div{background:#34d399;width:180px;border-radius:10px 10px 0 0;padding:20px}
  .bars div:nth-child(2){background:#818cf8}.bars div:nth-child(3){background:#38bdf8}</style>
  <h1>Build health / v${version}</h1><p>Living dashboard - generated report</p><div class="bars">
  <div style="height:170px">Tests<br>${120 + version}</div><div style="height:250px">Pass<br>100%</div>
  <div style="height:${150 + version * 30}px">Runs<br>${version}</div></div></html>`;
  await writeFile(join(worktree, "dashboard-report.html"), html);
  await cli("artifact", "add", "T-001", join(worktree, "dashboard-report.html"),
    "--kind", "report", "--title", "Build health");
  await command("git", ["add", "-A"], worktree);
  await command("git", ["commit", "-qm", `Generate dashboard version ${version}`], worktree);
  await checkpoint();
  await cli("done", "T-001", "--summary",
    `Generated report version ${version}. Verified: fixture generation. Not verified: none.`);
  const finished = await task();
  const run = finished.runs.at(-1).id;
  const artifact = (await json("artifact", "list", "--task", "T-001", "--json"))
    .find((row) => row.artifact.run === run && row.artifact.title === "Build health").artifact;
  const path = join(root, ".corral", artifact.path);
  const png = await readFile(`${path}.thumbnail.png`);
  assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
  assert.equal(png.readUInt32BE(16), 1200);
  assert.equal(png.readUInt32BE(20), 800);
  await copyFile(`${path}.thumbnail.png`, join(evidence, `html-v${version}.png`));
  result.checks.push(`PASS: corral done ${run} captured immutable HTML as 1200x800 PNG`);
  return artifact;
}
try {
  socket = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socket?.includes("/sessions/corral-test/"), "Refuse non-isolated herdr socket");
  const app = join(root, "app");
  await mkdir(app);
  for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "Gallery QA"],
    ["config", "user.email", "gallery@test.invalid"], ["config", "commit.gpgsign", "false"]])
    await command("git", args, app);
  await writeFile(join(app, "initial.txt"), "initial\n");
  await command("git", ["add", "-A"], app);
  await command("git", ["commit", "-qm", "initial"], app);
  await cli("init");
  for (const [key, value] of [
    ["resources.max_load_per_core", "1.7976931348623157e308"], ["resources.min_mem_available", "1B"],
    ["resources.min_disk_free", "1B"], ["agents.gallery-worker.command", "printf 'GALLERY-WORKER\\n'"],
    ["defaults.profile", "gallery-worker"], ["review.reviewer", "none"], ["review.accept", "owner"],
    ["review.autopilot", "false"], ["review.gate", '["true"]'],
  ]) await cli("config", "set", key, value);
  await cli("new", "Build health dashboard", "-p", "app", "--body", "Regenerate dashboard-report.html with title Build health.");
  const due = "2026-01-01T00:00:00Z";
  await cli("dashboard", "set", "T-001", "--interval-secs", "3600", "--next-due", due, "--authorize-dispatch");
  await cli("sync");
  assert.equal((await task()).runs.length, 1);
  await cli("sync");
  assert.equal((await task()).runs.length, 1);
  const first = await finishReport(1);
  await cli("pin", `T-001:R1:${first.path.split("/").at(-1)}`, "--follow-latest", "--label", "Build health");
  await cli("sync");
  assert.equal((await task()).runs.length, 1, "Review blocks recurrence");
  await cli("gate", "T-001", "--wait");
  const approvedWorkspace = (await task()).runs.at(-1).herdr?.workspace_id;
  await cli("approve", "T-001", "--merge", "ff");
  owned.delete(approvedWorkspace);
  await cli("dashboard", "set", "T-001", "--interval-secs", "3600", "--next-due", due, "--authorize-dispatch");
  await cli("sync");
  assert.equal((await task()).runs.length, 2);
  const second = await finishReport(2);
  const pins = await json("pins", "--json");
  await writeFile(join(evidence, "following-pin.json"), JSON.stringify(pins, null, 2));
  assert(JSON.stringify(pins).includes(second.path), "Following pin must resolve R2");
  result.checks.push("PASS: two sync-admitted runs, no overlap or recurrence during review, R1 pin follows R2");
  await cli("dashboard", "pause", "T-001");
  assert.equal((await json("dashboard", "show", "T-001")).paused, true);
  const chart = join(root, "inline-chart.png");
  await copyFile(join(evidence, "html-v1.png"), chart);
  await cli("artifact", "add", "T-001", chart, "--kind", "screenshot", "--title", "Inline chart");
  const missing = join(root, "historical-report.html");
  await writeFile(missing, "<h1>Historical report without a cached preview</h1>");
  await cli("artifact", "add", "T-001", missing, "--kind", "report", "--title", "Historical report");
  const opener = join(root, "opener");
  await writeFile(opener, `#!/bin/sh\nprintf '%s' "$1" > ${quote(join(root, "opened"))}\n`, { mode: 0o755 });
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", root, "--label", "T-024 gallery", "--no-focus")).result;
  owned.add(created.workspace.workspace_id);
  pane = created.root_pane.pane_id;
  await resize(80, 24);
  await stage("01-tour", "skip tour", (text) => text.includes("skip tour"), () =>
    herdr("pane", "run", pane,
      `stty -F /dev/tty cols 80 rows 24; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor CORRAL_OPENER=${quote(opener)} ${quote(binary)} --root ${quote(root)} board </dev/null`));
  await stage("02-board", "q leave", (text) => !text.includes("skip tour") && text.includes("q leave"), () => raw("\x1b"));
  let screen = await stage("03-gallery-80", "Gallery / recent", (text) =>
    text.includes("Gallery / recent") && text.includes("Inline chart") && text.includes("HTML preview unavailable"), () => raw("V"));
  screen = await stage("04-pinned-80", "Gallery / pinned", (text) =>
    text.includes("Gallery / pinned") && text.includes("1 visuals"), () => click(screen, "tab pinned"));
  screen = await stage("05-empty-filter-80", "No matching visuals", (text) =>
    text.includes("No matching visuals"), () => raw("zzzzz"));
  screen = await stage("06-recent-80", "Historical report", (text) =>
    text.includes("Gallery / recent") && text.includes("Historical report"), () => raw("\x15\t"));
  result.columns = 160; result.rows = 45;
  screen = await stage("07-gallery-160", "pgup/pgdn page.*ctrl\\+a finder", (text) =>
    text.includes("Inline chart") && text.includes("Historical report")
      && text.split("\n").some((line) => line.includes("pgup/pgdn page") && line.includes("ctrl+a finder"))
      && text.split("\n").length >= 40, () =>
    resize(160, 45));
  await stage("08-pinned-160", "Gallery / pinned", (text) =>
    text.includes("Gallery / pinned") && text.includes("1 visuals"), () => raw("\t"));
  result.checks.push("PASS: 80x24 and 160x45 real gallery, inline images, missing-preview state, filter, pinned scope and mouse parity");
  // Open only after the visual states are captured so its success toast cannot obscure empty copy.
  let timer;
  const opened = new Promise((accept, reject) => {
    const watcher = watch(root, (_event, file) => {
      if (String(file) === "opened") { clearTimeout(timer); watcher.close(); accept(); }
    });
    timer = setTimeout(() => { watcher.close(); reject(new Error("Original viewer was not invoked")); }, 10000);
  });
  await raw("\r");
  await opened;
  const openedPath = await readFile(join(root, "opened"), "utf8");
  assert(openedPath.endsWith("dashboard-report.html") && !openedPath.endsWith(".thumbnail.png"));
  assert.equal(await readFile(openedPath, "utf8"), await readFile(join(root, ".corral", second.path), "utf8"));
  result.checks.push("PASS: Enter opens immutable original HTML, not its thumbnail");
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) try {
    await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read.text);
  } catch (captureError) { result.captureError = String(captureError); }
} finally {
  subscription?.destroy();
  if (viewport) {
    viewport.kill();
    await viewport.exited;
    await viewportOutput;
    await viewportErrors;
  }
  for (const workspace of owned) {
    try { await herdr("workspace", "close", workspace); }
    catch (error) { result.cleanupError = String(error); result.status = "FAIL"; }
  }
  await rm(root, { recursive: true, force: true });
  result.cleanup = "Closed only owned fixture workspaces; removed fixture; isolated server is owned by helper";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(JSON.stringify(result, null, 2));
if (result.status !== "PASS") process.exitCode = 1;
