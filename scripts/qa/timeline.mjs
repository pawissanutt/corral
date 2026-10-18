// Real Timeline keys, mouse, raw JSON, and refresh on a private corral-test server.
// Run inside scripts/test-herdr.sh's environment; never start or reuse an owner server.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-164/live-120");
const columns = Number(process.argv[4] ?? 120);
assert([80, 120].includes(columns));
assert(process.env.HOME?.startsWith("/tmp/"), "Use the private test-herdr environment");
await mkdir(evidence, { recursive: true });
const fixture = await mkdtemp("/tmp/corral-timeline-");
const env = { ...process.env, CORRAL_ROOT: fixture, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const result = { status: "FAIL", binary, columns, session: "corral-test", scenes: [] };
const transcript = [];
let socketPath, workspace, pane, subscription;
let serial = 0;
let onOutput = () => {};
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";

async function command(program, args) {
  const child = Bun.spawn([program, ...args], {
    cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ command: [program, ...args], status, stdout, stderr });
  assert.equal(status, 0, `${program} ${args.join(" ")}: ${stderr}`);
  return stdout;
}
const cli = (...args) => command(binary, ["--root", fixture, ...args]);
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);

function rpc(method, params) {
  const id = `timeline-${++serial}`;
  return new Promise((accept, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    const timer = setTimeout(() => {
      connection.destroy();
      reject(new Error(`${method}: timeout`));
    }, 15000);
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
  const ansi = (await rpc("pane.read", {
    pane_id: pane, source: "visible", format: "ansi", strip_ansi: false,
  })).read.text;
  assert.match(ansi, /\x1b\[[0-9;]*(?:38|48);[25];/, "Capture must retain theme colors");
  await writeFile(join(evidence, `${name}.txt`), read.text);
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  result.scenes.push(name);
  for (const line of read.text.split("\n")) {
    assert(Bun.stringWidth(line) <= columns, `${name}: overflowing row`);
  }
  return read.text;
}

async function stage(name, marker, check, action) {
  let timer;
  const matched = new Promise((accept, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 15000);
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

async function click(screen, needle) {
  const lines = screen.split("\n");
  const y = lines.findIndex((line) => line.includes(needle));
  assert(y >= 0, `Missing mouse target ${needle}\n${screen}`);
  const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1;
  transcript.push({ mouse: needle, x, y: y + 1 });
  await raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.includes("/sessions/corral-test/"), "Refuse non-isolated socket");
  await mkdir(join(fixture, "app"));
  await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
  await cli("init");
  const id = (await cli("new", "Task history log", "-p", "app", "--idea")).trim();
  // Seed only this disposable store with the same historical scenario as the render tests.
  const scenario = await readFile(resolve("crates/corral-ops/tests/fixtures/task-history.jsonl"), "utf8");
  await writeFile(join(fixture, ".corral", "events.jsonl"), scenario.replaceAll("T-123", id));
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
    "--label", "T-164 Timeline acceptance", "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  result.workspace = workspace;
  result.pane = pane;
  await stage("01-tour", "skip tour", (s) => s.includes("skip tour"),
    () => herdr("pane", "run", pane,
      `stty -F /dev/tty cols ${columns} rows 30; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(binary)} --root ${quote(fixture)} board </dev/null`));
  await stage("02-board", "q (quit|leave)", (s) => s.includes("/ search"), () => raw("\x1b"));
  await stage("03-search", "results", (s) => s.includes("results"), () => raw("/"));
  await stage("04-query", `id:${id}`, (s) => s.includes(`id:${id}`), () => raw(`id:${id}`));
  await stage("05-spec", "7 Timeline", (s) => s.includes("6 Comments") && s.includes("7 Timeline"),
    () => raw("\r"));
  let screen = await stage("06-relevant", "Relevant events", (s) =>
    s.includes("Relevant events") && s.includes("needs_me") && !s.includes("future_kind"), () => raw("7"));
  assert(screen.includes("┐ waited 1h 05m"), "Opening wait bracket and duration must be visible");
  assert(screen.includes("┘ waited 1h 05m"), "Reply wait bracket and duration must be visible");
  assert.match(screen, /┐ waiting (?:\d+h )?\d+m \(open\)/, "Unresolved operation must show its age");
  const entries = (await cli("log", id, "--json")).trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(entries.find((entry) => entry.kind === "replied").wait.duration.secs, 3900);
  assert.equal(entries.find((entry) => entry.kind === "blocked").wait.open, true);
  assert.equal(entries.find((entry) => entry.kind === "plan_verdict").wait, null,
    "An unrelated plan verdict must not resolve a post-land operation");
  screen = await stage("07-all", "All events", (s) =>
    s.includes("All events") && s.includes("reply_delivery"), () => raw("a"));
  assert(!screen.includes("OTHER TASK") && !screen.includes("GLOBAL MUST"));
  const rows = screen.split("\n").filter((line) => /2026-10-15 \d\d:\d\d/.test(line));
  assert(rows.length >= 13, "Visible scenario events each occupy exactly one row");
  await click(screen, "edited");
  screen = await stage("08-raw-event", "Raw event", (s) =>
    s.includes("Raw event") && s.includes('"kind": "edited"'), () => raw("\r"));
  await stage("09-raw-scrolled", "sha_to", (s) => s.includes('"sha_to": "9a0e"'),
    () => raw("\x1b[6~"));
  screen = await stage("10-raw-closed", "All events", (s) =>
    s.includes("All events") && !s.includes("Raw event"), () => raw("\x1b"));
  screen = await stage("10b-noise-scrolled", "future_kind", (s) =>
    s.includes("future_kind") && s.includes("reply_delivery"), () => raw("\x1b[6~"));
  screen = await stage("11-noise-hidden", "Relevant events", (s) =>
    s.includes("Relevant events") && !s.includes("future_kind"), () => raw("a"));
  screen = await stage("12-comments-mouse", "Add comment", (s) =>
    s.includes("Add comment") && !s.includes("Relevant events"), () => click(screen, "6 Comments"));
  screen = await stage("13-timeline-mouse", "Relevant events", (s) =>
    s.includes("Relevant events"), () => click(screen, "7 Timeline"));
  await stage("14-refreshed", "REFRESH-LIVE", (s) => s.includes("REFRESH-LIVE"),
    () => cli("comment", id, "REFRESH-LIVE"));
  const review = (await readFile(resolve("crates/corral-ops/tests/fixtures/timeline-wait-review.jsonl"),
    "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  for (const [task, scene, marker, check] of [
    ["T-205", "15-repeated", "┘10m / ┘5m", (s) => !s.includes("(open)")],
    ["T-206", "16-moved-closed", "┘5m / ┐5m", (s) => !s.includes("(open)")],
    ["T-207", "17-moved-open", "┘5m / ┐", (s) => s.includes("(open)")],
    ["T-208", "18-isolation", "REVIEW-208", (s) => (s.match(/\(open\)/g) ?? []).length === 1],
  ]) {
    const events = review.filter((event) => event.task === task)
      .map((event) => JSON.stringify({ ...event, task: id })).join("\n") + "\n";
    const refreshed = `REVIEW-${task.slice(2)}`;
    await stage(scene, refreshed, (s) => s.includes(refreshed) && s.includes(marker) && check(s), async () => {
      await writeFile(join(fixture, ".corral", "events.jsonl"), events);
      await cli("comment", id, refreshed);
    });
    const paired = (await cli("log", id, "--json")).trim().split("\n")
      .map((line) => JSON.parse(line));
    const text = await cli("log", id);
    switch (task) {
      case "T-205":
        assert.equal(paired[1].wait.duration.secs, 300);
        assert.equal(paired[2].additional_waits[0].opened_at, paired[1].at);
        assert.equal(paired[3].wait, null, "No wait may leak into the later done");
        assert(!text.includes("(open)"));
        break;
      case "T-206":
      case "T-207":
        assert.equal(paired[1].wait.opened_at, paired[0].at);
        assert.equal(paired[1].wait.open, false);
        assert.equal(paired[1].opened_wait.opened_at, paired[1].at);
        assert.equal(paired[1].opened_wait.open, task === "T-207");
        break;
      case "T-208":
        for (const [index, seconds] of [[0, 600], [1, 540], [2, 1080], [3, 720], [4, 660]]) {
          assert.equal(paired[index].wait.open, false);
          assert.equal(paired[index].wait.duration.secs, seconds);
        }
        assert.equal(paired[5].wait.open, true, "Only operation Y remains pending");
        break;
    }
  }
  await raw("\x1bq");
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) {
    try { await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read); }
    catch (captureError) { result.captureError = String(captureError); }
  }
} finally {
  subscription?.destroy();
  if (workspace) {
    try {
      await herdr("workspace", "close", workspace);
      result.cleanup = "Closed only this fixture's workspace";
    } catch (error) { result.cleanup = String(error); result.status = "FAIL"; }
  }
  await rm(fixture, { recursive: true, force: true });
  result.fixtureCleanup = "Removed this run's disposable store";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(`${result.status}: ${evidence}`);
if (result.error) console.error(result.error);
process.exitCode = result.status === "PASS" ? 0 : 1;
