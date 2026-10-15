// Real CLI and keyboard/mouse search acceptance in the isolated corral-test engine.
// Every rendered-state subscription is acknowledged before its triggering input.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-047/live");
const columns = Number(process.argv[4] ?? 120);
assert(Number.isInteger(columns) && columns >= 80 && columns <= 200);
await mkdir(evidence, { recursive: true });
const fixture = await mkdtemp(join(evidence, "fixture-"));
const env = { ...process.env, CORRAL_ROOT: fixture, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of [
  "HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE",
]) delete env[key];
const transcript = [];
const result = { status: "FAIL", binary, columns, session: "corral-test", scenes: [] };
let workspace;
let pane;
let socketPath;
let subscription;
let onOutput = () => {};
let serial = 0;

async function command(program, args, pass = true) {
  const child = Bun.spawn([program, ...args], {
    cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  transcript.push({ command: [program, ...args], status, stdout, stderr });
  if (pass) assert.equal(status, 0, `${program} ${args.join(" ")}: ${stderr}`);
  return { stdout, stderr, status };
}
const cli = async (...args) => (await command(binary, ["--root", fixture, ...args])).stdout;
const herdr = async (...args) =>
  (await command("herdr", ["--session", "corral-test", ...args])).stdout;

function rpc(method, params) {
  const id = `search-${++serial}`;
  return new Promise((resolveRequest, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) {
      subscription?.destroy();
      subscription = connection;
    }
    const timer = setTimeout(() => {
      connection.destroy();
      reject(new Error(`${method}: timed out`));
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
          else resolveRequest(message.result);
        } else if (message.event === "pane.output_matched" && message.data.pane_id === pane) {
          onOutput(message.data.read);
        }
      }
    });
    connection.on("error", (error) => { clearTimeout(timer); reject(error); });
    connection.once("connect", () => {
      connection.write(JSON.stringify({ id, method, params }) + "\n");
    });
  });
}

async function capture(name, read) {
  await writeFile(join(evidence, `${name}.txt`), read.text);
  await writeFile(join(evidence, `${name}.json`), JSON.stringify(read, null, 2) + "\n");
  const ansi = await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi", "--ansi");
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  result.scenes.push(name);
  return read.text;
}

async function stage(name, marker, check, action) {
  let timer;
  const matched = new Promise((resolveStage, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: rendered state not observed`)), 15000);
    onOutput = (read) => {
      if (check(read.text)) {
        onOutput = () => {};
        capture(name, read).then(resolveStage, reject);
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
  } finally {
    clearTimeout(timer);
    onOutput = () => {};
  }
}

const raw = (text) => rpc("pane.send_input", { pane_id: pane, text });
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
function locate(text, needle) {
  const lines = text.split("\n");
  const row = lines.findIndex((line) => line.includes(needle));
  assert(row >= 0, `Rendered target missing: ${needle}\n${text}`);
  return [Bun.stringWidth(lines[row].slice(0, lines[row].indexOf(needle))) + 1, row + 1];
}
async function click(text, needle) {
  const [x, y] = locate(text, needle);
  transcript.push({ mouse: "SGR left press/release", target: needle, x, y });
  await raw(`\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`);
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.includes("/sessions/corral-test/"), "Refuse non-isolated herdr socket");
  await mkdir(join(fixture, "app"));
  await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
  await cli("init");
  const idea = (await cli("new", "Nebula search", "-p", "app", "--idea")).trim();
  const done = (await cli("new", "Archived task", "-p", "app", "--body", "Nebula recovery")).trim();
  const dropped = (await cli("new", "Discarded task", "-p", "app")).trim();
  const ready = (await cli("new", "Ready task", "-p", "app", "--ready")).trim();
  await cli("move", done, "done", "--force");
  await cli("move", dropped, "dropped", "--force");
  await cli("pin", done);
  const artifactFile = join(evidence, "supporting-note.md");
  await writeFile(artifactFile, "# Distinct artifact evidence\n");
  await cli("artifact", "add", dropped, artifactFile, "--kind", "report", "--title", "Quasar report");
  const json = async (...args) => JSON.parse(await cli(...args));
  assert.deepEqual((await json("search", "nebula", "--json")).map((t) => t.id), [idea, done]);
  assert.deepEqual((await json("list", "--query", "nebula", "--json")).map((t) => t.id), [idea, done]);
  assert.deepEqual((await json("search", "pinned status:done", "--json")).map((t) => t.id), [done]);
  assert.deepEqual((await json("search", "has:artifact quasar", "--json")).map((t) => t.id), [dropped]);
  assert.deepEqual((await json("search", `id:${ready} -status:done`, "--json")).map((t) => t.id), [ready]);
  const invalid = await command(binary, ["--root", fixture, "search", "unknown:yes"], false);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /unknown query field/);
  result.cli = "PASS: ranked JSON, list parity, every status, body, artifact title, pinned, negation, id, errors";

  const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
    "--label", "T-047 search acceptance", "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  result.workspace = workspace;
  result.pane = pane;
  const boardCheck = (text) => text.includes("Backlog") && text.includes("Ready") &&
    text.includes("/ search") && !text.includes("‹ back");
  await stage("01-tour", "skip tour", (text) => text.includes("skip tour"),
    () => herdr("pane", "run", pane,
      `stty -F /dev/tty cols ${columns} rows 36; ${quote(binary)} --root ${quote(fixture)} board </dev/null`));
  let screen = await stage("02-board", "q (quit|leave)", boardCheck, () => raw("\x1b"));
  screen = await stage("03-search-keyboard", "results", (text) => text.includes("results") &&
    text.includes("search") && text.includes(idea) && text.includes(dropped), () => raw("/"));
  screen = await stage("04-live-query", "status:done", (text) => text.includes("status:done") &&
    text.includes(done) && !text.includes(idea) && !text.includes(dropped),
    () => raw("status:done"));
  screen = await stage("05-detail-keyboard", "Spec", (text) => text.includes("Spec") &&
    text.includes(done) && text.includes("Archived task"), () => raw("\r"));
  screen = await stage("06-search-return", "results", (text) => text.includes("results") &&
    text.includes("status:done"), () => raw("\x1b"));
  screen = await stage("07-error", "unknown query field", (text) => text.includes("unknown query field"),
    () => raw("\x15unknown:yes"));
  screen = await stage("08-empty", "no tasks match", (text) => text.includes("no tasks match"),
    () => raw("\x15id:NO-SUCH-TASK"));
  screen = await stage("09-board-return", "q (quit|leave)", boardCheck, () => raw("\x1b"));
  screen = await stage("10-search-mouse", "results", (text) => text.includes("results") &&
    text.includes(dropped), () => click(screen, "/ search"));
  // Search input takes ordinary text; navigation and opening here use only the mouse.
  screen = await stage("11-mouse-query", "status:dropped", (text) => text.includes("status:dropped") &&
    text.includes(dropped) && !text.includes(idea), () => raw("status:dropped"));
  screen = await stage("12-detail-mouse", "Spec", (text) => text.includes("Spec") &&
    text.includes(dropped) && text.includes("Discarded task"), () => click(screen, dropped));
  screen = await stage("13-search-back-mouse", "results", (text) => text.includes("results") &&
    text.includes("status:dropped"), () => click(screen, "‹ back"));
  screen = await stage("14-board-back-mouse", "q (quit|leave)", boardCheck,
    () => click(screen, "‹ back"));
  screen = await stage("15-history", "history ·", (text) => text.includes("history ·") &&
    text.includes(done) && text.includes(dropped), () => raw("Y"));
  screen = await stage("16-history-query", "status:dropped", (text) =>
    text.includes("status:dropped") && text.includes(dropped) && !text.includes(done),
    () => raw("status:dropped"));
  screen = await stage("17-history-return", "q (quit|leave)", boardCheck, () => raw("\x1b"));
  await raw("q");
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
      result.cleanup = "Closed only the created corral-test workspace; existing server/workspaces untouched";
    } catch (error) { result.cleanup = String(error); result.status = "FAIL"; }
  }
  await rm(fixture, { recursive: true, force: true });
  result.fixtureCleanup = "Removed only this run's temporary fixture";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(result, null, 2) + "\n");
}
console.log(`${result.status}: ${evidence}`);
if (result.error) console.error(result.error);
process.exitCode = result.status === "PASS" ? 0 : 1;
