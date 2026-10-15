#!/usr/bin/env bun
// Live rendered-state proof. Only SGR mouse bytes navigate the TUI.
import { execFile } from "node:child_process";
import { createConnection } from "node:net";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const exec = promisify(execFile);
const { CORRAL, EV } = process.env;
const timeout = Number(process.env.MOUSE_TIMEOUT_MS ?? 10000);
if (!CORRAL || !EV || !Number.isSafeInteger(timeout) || timeout <= 0) {
  throw new Error("CORRAL, EV and a positive integer MOUSE_TIMEOUT_MS are required");
}
const env = { ...process.env, CORRAL_HERDR_SESSION: "corral-test" };
delete env.HERDR_SOCKET_PATH;
const command = async (file, args) =>
  (await exec(file, args, { env, timeout, maxBuffer: 4 * 1024 * 1024 })).stdout;
const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);
const quote = (text) => "'" + text.replaceAll("'", "'\\''") + "'";
await mkdir(EV, { recursive: true });
const evidence = await mkdtemp(join(EV, "run-"));
const transcript = [];
const report = { status: "FAIL", binary: CORRAL, evidence, session: "corral-test",
  server: "pre-existing; never stopped", workspace: null, fixture: null };
let socket;
let socketPath;
let root;
let pane;
let workspace;
let onOutput = () => {};
let serial = 0;

function rpc(method, params) {
  const id = `mouse-${++serial}`;
  return new Promise((resolve, reject) => {
    // Subscription streams own their connection. Other requests use separate sockets.
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) {
      socket?.destroy();
      socket = connection;
    }
    const timer = setTimeout(() => {
      connection.destroy();
      reject(new Error(`${method} timed out after ${timeout} ms`));
    }, timeout);
    connection.setEncoding("utf8");
    let buffer = "";
    connection.on("data", (chunk) => {
      buffer += chunk;
      let newline;
      while ((newline = buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(buffer.slice(0, newline));
        buffer = buffer.slice(newline + 1);
        transcript.push(message);
        if (message.id === id) {
          clearTimeout(timer);
          if (!streaming) connection.destroy();
          if (message.error) reject(new Error(JSON.stringify(message.error)));
          else resolve(message.result);
        } else if (message.event === "pane.output_matched" && message.data.pane_id === pane) {
          onOutput(message.data.read);
        }
      }
    });
    connection.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    connection.once("connect", () => {
      connection.write(JSON.stringify({ id, method, params }) + "\n");
    });
  });
}

async function capture(name, result) {
  // The matching event includes the exact snapshot that satisfied the subscription.
  await writeFile(join(evidence, `${name}.txt`), result.text);
  await writeFile(join(evidence, `${name}.json`), JSON.stringify(result, null, 2) + "\n");
  const ansi = await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi");
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  return result.text;
}

// Acknowledged server-side subscription BEFORE the action; no interval polling.
async function stage(name, pattern, check, action) {
  let timer;
  const matched = new Promise((resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`${name}: no matching rendered state in ${timeout} ms`)), timeout);
    onOutput = (read) => {
      if (check(read.text)) capture(name, read).then(resolve).catch(reject);
    };
  });
  // Observe a timeout even if the action fails before awaiting matched.
  matched.catch(() => {});
  try {
    await rpc("events.subscribe", { subscriptions: [{
      type: "pane.output_matched", pane_id: pane, source: "visible",
      match: { type: "regex", value: pattern }, strip_ansi: true,
    }] });
    await action();
    return await matched;
  } finally {
    clearTimeout(timer);
    onOutput = () => {};
  }
}

function locate(text, needle) {
  const lines = text.split("\n");
  const y = lines.findIndex((line) => line.includes(needle));
  if (y < 0) throw new Error(`Rendered target missing: ${needle}`);
  // SGR coordinates are 1-based terminal cells, not UTF-8 bytes.
  return [Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1, y + 1];
}

async function click([x, y]) {
  const text = `\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`;
  transcript.push({ input: "SGR left press/release", x, y, bytes: text });
  // send_input writes raw bytes; send_text can apply bracketed paste.
  await rpc("pane.send_input", { pane_id: pane, text });
}

try {
  await command("test", ["-x", CORRAL]);
  let status;
  try {
    status = await herdr("status", "server");
  } catch (error) {
    throw new Error("Start a plugin-free server in the same subshell: source scripts/test-herdr.sh; corral_test_herdr_start || exit 1", { cause: error });
  }
  socketPath = status.match(/^socket: (.+)$/m)?.[1];
  if (!socketPath || !socketPath.includes("/sessions/corral-test/")) {
    throw new Error(`Refusing a non-corral-test socket: ${socketPath}`);
  }
  root = await mkdtemp(join(tmpdir(), "corral-mouse-"));
  report.fixture = root;
  await mkdir(join(root, "app"));
  await command("git", ["-C", join(root, "app"), "init", "-q", "-b", "main"]);
  const corral = (...args) => command(CORRAL, ["--root", root, ...args]);
  await corral("init");
  await corral("new", "Mouse backlog", "-p", "app");
  const id = (await corral("new", "Mouse Ready", "-p", "app", "--ready")).trim();
  if (!/^T-\d+$/.test(id)) throw new Error(`Unexpected task id: ${id}`);
  report.task = id;
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", root,
    "--label", "corral-mouse-proof", "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  report.workspace = workspace;
  report.pane = pane;
  await writeFile(join(evidence, "workspace.json"), JSON.stringify(created, null, 2) + "\n");
  const isBoard = (text) => text.includes("Backlog") && text.includes("Ready") &&
    text.includes("q quit") && !text.includes("‹ back");
  const selected = `▌ ${id}`;
  const board = await stage("01-board", id, (text) => isBoard(text) && text.includes(id) &&
    !text.includes(selected), () => herdr("pane", "run", pane, `${quote(CORRAL)} --root ${quote(root)} board`));
  const spot = locate(board, id);
  const selection = await stage("02-selected", selected,
    (text) => isBoard(text) && text.includes(selected), () => click(spot));
  // A second click on the selected card opens detail, independent of double-click timing.
  const detail = await stage("03-detail", "‹ back",
    (text) => text.includes("‹ back") && text.includes(id) && text.includes("Mouse Ready") &&
      text.includes("Spec") && text.includes("Live"),
    () => click(locate(selection, id)));
  let screen = await stage("04-back", selected, (text) => isBoard(text) && text.includes(selected),
    () => click(locate(detail, "‹ back")));
  const navigate = async (name, label, marker) => {
    screen = await stage(name, marker, (text) => text.includes(marker),
      () => click(locate(screen, label)));
  };
  await navigate("05-settings-general", ", settings", "Global slot limit");
  await navigate("06-settings-profiles", "Profiles", "▸ Profiles");
  await navigate("07-profile-editor", "claude", "Profiles › claude");
  await navigate("08-profile-model-picker", "▾ choose", "model for claude");
  await navigate("09-profile-model-cancel", "esc cancel", "esc profiles");
  await navigate("10-settings-harnesses", "Harnesses", "▸ Harnesses");
  await navigate("11-settings-projects", "Projects", "▸ Projects");
  await navigate("12-settings-artifacts", "Artifacts", "▸ Artifacts");
  await navigate("13-settings-general-return", "General", "Global slot limit");
  screen = await stage("14-settings-back", selected,
    (text) => isBoard(text) && text.includes(selected),
    () => click(locate(screen, "‹ back")));
  await navigate("15-dispatch", "d dispatch", `dispatch ${id}`);
  await navigate("16-dispatch-model-picker", "M model", "model · omo");
  await navigate("17-dispatch-model-cancel", "esc close", "⏎ dispatch");
  screen = await stage("18-dispatch-cancel", "q quit",
    (text) => isBoard(text) && text.includes(selected),
    () => click(locate(screen, "[Cancel]")));
  // Toggle last so config-reload notifications cannot obscure subsequent controls.
  await navigate("19-autopilot-off", "O autopilot", "autopilot off");
  await navigate("20-autopilot-on", "O autopilot", "autopilot on");
  report.scenes = 20;
  report.status = "PASS";
} catch (error) {
  report.error = String(error.stack ?? error);
  if (pane) {
    try {
      const result = await rpc("pane.read", { pane_id: pane, source: "visible" });
      await capture("failure", result.read);
    } catch (captureError) { report.captureError = String(captureError); }
  }
} finally {
  socket?.destroy();
  if (workspace) {
    try {
      await herdr("workspace", "close", workspace);
      report.workspaceCleanup = "closed only the workspace created by this run";
    } catch (error) { report.workspaceCleanup = String(error); report.status = "FAIL"; }
  }
  if (root) {
    await rm(root, { recursive: true, force: true });
    report.fixtureCleanup = "removed temporary fixture; no user tasks touched";
  }
  await writeFile(join(evidence, "events.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(report, null, 2) + "\n");
}
console.log(`${report.status}: ${evidence}`);
if (report.error) console.error(report.error);
process.exitCode = report.status === "PASS" ? 0 : 1;
