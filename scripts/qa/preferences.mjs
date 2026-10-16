// Run only after scripts/test-herdr.sh has started its private, plugin-free server.
// Usage: bun scripts/qa/preferences.mjs target/debug/corral <evidence-dir> [80|120|160]
// Subscribe and receive the acknowledgement before every triggering action; no polling.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

const binary = resolve(process.argv[2] ?? "target/debug/corral");
const evidence = resolve(process.argv[3] ?? ".omo/evidence/T-009/live-120");
const columns = Number(process.argv[4] ?? 120);
assert(Number.isInteger(columns) && columns >= 80 && columns <= 200);
// A session selector alone is not isolation: require the helper's private HOME/XDG.
const isolatedRoot = dirname(process.env.HOME ?? "");
assert.match(isolatedRoot, /^\/tmp\/corral-herdr-[^/]+$/);
assert.equal(process.env.HOME, join(isolatedRoot, "home"));
for (const [key, directory] of Object.entries({
  XDG_CONFIG_HOME: "config", XDG_DATA_HOME: "data",
  XDG_STATE_HOME: "state", XDG_CACHE_HOME: "cache",
})) assert.equal(process.env[key], join(isolatedRoot, directory), `Unsafe ${key}`);
assert.equal(process.env.CORRAL_HERDR_SESSION, "corral-test");
await mkdir(evidence, { recursive: true });
const fixture = await mkdtemp(join(evidence, "fixture-"));
const env = { ...process.env, CORRAL_ROOT: fixture, CORRAL_HERDR_SESSION: "corral-test" };
for (const key of [
  "HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE",
]) delete env[key];
const transcript = [];
const result = { status: "FAIL", binary, columns, rows: 42, session: "corral-test", scenes: [] };
let workspace;
let pane;
let socketPath;
let subscription;
let viewport;
let viewportOutput;
let viewportErrors;
let onOutput = () => {};
let serial = 0;

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
const get = async (key) => (await cli("config", "get", key)).trim();
const set = (key, value) => cli("config", "set", key, value);
async function verify(expected) {
  for (const [key, value] of Object.entries(expected)) {
    assert.equal(await get(key), value, `Persisted ${key}`);
  }
}

function rpc(method, params) {
  const id = `preferences-${++serial}`;
  transcript.push({ rpc: method, params });
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
  const ansi = await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi", "--ansi");
  const plain = ansi.replace(/\x1b\[[0-9;]*m/g, "").replaceAll("\r", "").trimEnd() + "\n";
  // Text, structured receipt and PNG replay all describe the same captured frame.
  await writeFile(join(evidence, `${name}.txt`), plain);
  await writeFile(join(evidence, `${name}.json`), JSON.stringify({ ...read, text: plain }, null, 2) + "\n");
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  result.scenes.push(name);
  return plain;
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
      match: { type: "regex", value: `(?m)^.*(?:${marker}).*$` }, strip_ansi: true,
    }] });
    await action();
    return await matched;
  } finally {
    clearTimeout(timer);
    onOutput = () => {};
  }
}

const raw = (text) => rpc("pane.send_input", { pane_id: pane, text });
const displaySetting = async (key, value) => {
  await set(key, value);
  // Returning from Settings explicitly reloads the board, independent of sync load.
  await raw(",\x1b");
};
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
const selected = (text, label) => text.includes(`▸ ${label}`);
const checked = (text, label, value) => {
  const lines = text.split("\n");
  const row = lines.findIndex((line) => line.includes(label));
  return row >= 0 && lines.slice(row, row + 2).join("\n").includes(`[${value}]`);
};
async function click(text, needle) {
  const lines = text.split("\n");
  const row = lines.findIndex((line) => line.includes(needle));
  assert(row >= 0, `Rendered target missing: ${needle}\n${text}`);
  const x = Bun.stringWidth(lines[row].slice(0, lines[row].indexOf(needle))) + 1;
  const y = row + 1;
  transcript.push({ mouse: "SGR left press/release", target: needle, x, y });
  await raw(`\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`);
}
const exited = (text) => text.includes("PREFERENCES-QA-EXIT");
const launch = (mode) => herdr("pane", "run", pane,
  `stty -F /dev/tty cols ${columns} rows 42; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(binary)} --root ${quote(fixture)} ${mode} </dev/null; printf '%s%s\\n' 'PREFERENCES-QA-' 'EXIT'`);
const board = (text) => text.includes("z new") && !text.includes("‹ back") &&
  !text.includes("what's the idea?") && !text.includes("skip tour");
const captureOpen = (text) => text.includes("what's the idea?") && text.includes("save as");

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.startsWith(`${isolatedRoot}/`) &&
    socketPath.includes("/sessions/corral-test/"), "Refuse non-isolated herdr socket");
  await mkdir(join(fixture, "app"));
  await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
  await cli("init");
  await set("dispatch.paused", "true");
  const initial = {
    "ui.accent": "cyan", "ui.compact_cards": "false", "ui.show_wip": "true",
    "ui.notify_needs_me": "false", "ui.notify_review": "false", "dispatch.paused": "true",
  };
  for (const [key, value] of Object.entries(initial)) await set(key, value);
  await set("ui.mouse", "true");
  await set("board.card.density", "comfortable");
  // No Ready tasks or runs exist, even while testing the auto-dispatch toggle.
  const title = "QA card";
  const task = (await cli("new", title, "-p", "app")).trim();
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
    "--label", "T-009 preferences acceptance", "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  result.workspace = workspace;
  result.pane = pane;
  // Own the emulator viewport as well as the PTY; stty alone truncates captures.
  viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
    "control", pane, "--cols", String(columns), "--rows", String(result.rows)],
    { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  viewportErrors = new Response(viewport.stderr).text();
  const reader = viewport.stdout.getReader();
  let readyTimer;
  try {
    const first = await Promise.race([
      reader.read(),
      new Promise((_, reject) => {
        readyTimer = setTimeout(() => reject(new Error("Viewport control not ready")), 15000);
      }),
    ]);
    assert(!first.done, `Viewport control closed: ${first.done ? await viewportErrors : ""}`);
    await writeFile(join(evidence, "viewport.json"), new TextDecoder().decode(first.value));
  } finally {
    clearTimeout(readyTimer);
  }
  viewportOutput = (async () => { while (!(await reader.read()).done) {} })();
  await stage("01-settings", "sections", (text) => text.includes("sections") &&
    text.includes("settings"), () => launch("settings"));
  await stage("02-preferences", "Accent", (text) => selected(text, "Accent") &&
    checked(text, "Accent", "cyan"), () => raw("8"));
  await stage("03-accent-staged", "▸ Accent[^\\n]*\\[blue\\]", (text) =>
    checked(text, "Accent", "blue") && text.includes("unsaved changes"), () => raw("l"));
  for (const [name, label, value] of [
    ["04-compact-staged", "Compact cards", "on"],
    ["05-wip-staged", "Show WIP usage", "off"],
    ["06-needs-me-staged", "Desktop: Needs me", "on"],
    ["07-review-staged", "Desktop: Review", "on"],
    ["08-auto-dispatch-staged", "Auto-dispatch queue", "on"],
  ]) {
    await stage(name, `▸ ${label}[^\\n]*\\[${value}\\]`, (text) => selected(text, label) && checked(text, label, value),
      () => raw("j\r"));
  }
  await verify(initial);
  await stage("08b-key-field", "Key: New task", (text) => selected(text, "Key: New task"),
    () => raw("8" + "j".repeat(23)));
  await stage("08c-key-edit", "z▏", (text) => selected(text, "Key: New task") && text.includes("z▏"),
    () => raw("\rz"));
  await stage("08d-key-staged", "▸ Key: New task[ ]+z[ ]", (text) =>
    selected(text, "Key: New task") && !text.includes("z▏"), () => raw("\r"));
  const saved = {
    "ui.accent": "blue", "ui.compact_cards": "true", "ui.show_wip": "false",
    "ui.notify_needs_me": "true", "ui.notify_review": "true", "dispatch.paused": "false",
    "ui.keys.capture": "z",
  };
  await stage("09-preferences-saved", "Settings saved", (text) =>
    text.includes("Settings saved") && !text.includes("unsaved changes"), () => raw("s"));
  await verify(saved);
  // Stage another accent, cancel the leave confirmation, then discard it.
  await stage("10-discard-draft", "▸ Accent[^\\n]*\\[magenta\\]", (text) =>
    checked(text, "Accent", "magenta") && text.includes("unsaved changes"), () => raw("8l"));
  await stage("11-discard-confirm", "not saved yet", (text) =>
    text.includes("not saved yet") && text.includes("discard"), () => raw("\x1b"));
  await stage("12-cancel-discard", "esc back", (text) =>
    checked(text, "Accent", "magenta") && !text.includes("not saved yet"), () => raw("n"));
  await verify(saved);
  await stage("13-discard-confirm-again", "not saved yet", (text) =>
    text.includes("not saved yet"), () => raw("\x1b"));
  await stage("14-discard-exit", "PREFERENCES-QA-EXIT", exited, () => raw("y"));
  await verify(saved);
  await set("dispatch.paused", "true");
  await set("ui.keys.tour", "'e'");
  await set("ui.keys.agent", "'b'");
  await set("ui.keys.reply", "'v'");
  await set("ui.keys.leave", "'x'");
  await set("ui.columns", '["backlog", "ready"]');
  assert.equal(await get("ui.keys.capture"), "z");
  await stage("15-tour", "skip tour", (text) => text.includes("skip tour"), () => launch("board"));
  for (let step = 2; step <= 6; step++) {
    await stage(`15-tour-step-${step}`, `welcome to corral · ${step}/6`,
      (text) => text.includes(`welcome to corral · ${step}/6`), () => raw("\x1b[C"));
  }
  let screen = await stage("16-board-remapped", "z new", board, () => raw("\x1b"));
  assert(!screen.includes("n new"));
  assert(!screen.split("\n")[0].includes("slots"), "WIP header must be suppressed");
  assert(screen.includes("Backlog") && screen.includes("Ready"));
  assert(!screen.includes("Running") && !screen.includes("Done"), "Hidden columns rendered");
  const cardRow = (text) => text.split("\n").find((line) => line.includes(task));
  assert(cardRow(screen)?.includes(title), "Compact card must occupy one line");
  const ansi = await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi", "--ansi");
  assert.match(ansi, /\x1b\[[0-9;]*38;2;115;166;255[0-9;]*m/, "Blue accent missing");
  assert(!ansi.includes("38;2;92;207;230"), "Cyan accent survived final App composition");
  screen = await stage("16a-help-tour-key", "replay the welcome tour", (text) =>
    text.includes("replay the welcome tour"), () => raw("?"));
  await stage("16b-tour-remapped-key", "welcome to corral · 1/6", (text) =>
    text.includes("welcome to corral · 1/6"), () => raw("e"));
  await stage("16c-tour-key-return", "z new", board, () => raw("\x1b"));
  screen = await stage("16d-help-tour-mouse", "replay the welcome tour", (text) =>
    text.includes("replay the welcome tour"), () => raw("?"));
  await stage("16e-tour-remapped-mouse", "welcome to corral · 1/6", (text) =>
    text.includes("welcome to corral · 1/6"),
    () => click(screen, `e${" ".repeat(17)}replay`));
  await stage("16f-tour-mouse-return", "z new", board, () => raw("\x1b"));
  await stage("17-capture-keyboard", "save as", captureOpen, () => raw("z"));
  await stage("18-capture-literal-n", "nimbus", (text) =>
    text.includes("nimbus") && text.includes("save as"), () => raw("nimbus"));
  screen = await stage("19-capture-cancel", "z new", board, () => raw("\x1b"));
  await stage("20-capture-footer-click", "save as", captureOpen,
    () => click(screen, "z new"));
  await stage("21-footer-capture-cancel", "z new", board, () => raw("\x1b"));
  // Apply display changes through the real Settings return path, without refresh sleeps.
  screen = await stage("22-normal-cards", `▌ ${title}`, (text) => board(text) &&
    text.includes(title) && Boolean(cardRow(text)) && !cardRow(text).includes(title),
    () => displaySetting("ui.compact_cards", "false"));
  await stage("23-wip-visible", "slots", (text) => board(text) &&
    text.split("\n")[0].includes("slots"), () => displaySetting("ui.show_wip", "true"));
  await stage("24-wip-hidden", "▲ 0( needs you)?[ ]+◆ 0", (text) => board(text) &&
    !text.split("\n")[0].includes("slots"), () => displaySetting("ui.show_wip", "false"));
  await stage("25-columns-hidden", `${task} app[ ]{${Math.floor(columns / 2)},}P2 ⋯│`, (text) => board(text) &&
    text.includes("Backlog") && !text.includes("Ready"), () => displaySetting("ui.columns", '["backlog"]'));
  await stage("26-final-board", `${task}.*${title}`, (text) => board(text) &&
    cardRow(text)?.includes(title), () => displaySetting("ui.compact_cards", "true"));
  await stage("27-final-settings", "sections", (text) => text.includes("sections"),
    () => raw(","));
  await stage("28-final-preferences", "▸ Accent[^\\n]*\\[blue\\][^\\n]*magenta", (text) =>
    checked(text, "Accent", "blue") && checked(text, "Compact cards", "on"),
    () => raw("8"));
  await verify({ ...saved, "dispatch.paused": "true", "ui.keys.capture": "z", "ui.keys.tour": "e" });
  await cli("config", "get", "ui");
  await cli("config", "get", "dispatch");
  await stage("29-final-board-return", "z new", board, () => raw("\x1b"));
  await stage("30-board-close-confirm", "close the board", (text) =>
    text.includes("close the board?"), () => raw("x"));
  await stage("31-board-exit", "PREFERENCES-QA-EXIT", exited, () => raw("q"));
  result.status = "PASS";
  result.coverage = "Staged/save/discard persistence; notification flags only (delivery tested separately); remapped keyboard and clickable footer; literal Capture text; columns; WIP; one-line cards; composed blue accent";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) {
    try { await capture("failure", (await rpc("pane.read", { pane_id: pane, source: "visible" })).read); }
    catch (captureError) { result.captureError = String(captureError); }
  }
} finally {
  subscription?.destroy();
  if (viewport) {
    viewport.kill();
    await viewport.exited;
    await viewportOutput;
    result.viewportErrors = await viewportErrors;
  }
  if (workspace) {
    try {
      await herdr("workspace", "close", workspace);
      result.cleanup = "Closed only the created corral-test workspace; server and other workspaces untouched";
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
