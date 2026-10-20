// Run after scripts/test-herdr.sh starts a fresh private server, or use the shell wrapper.
// Usage: bun scripts/qa/operating-modes.mjs CANDIDATE EVIDENCE [80|160]
// This is a surface/persistence journey, not an executor or acceptance-proof test.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

assert(process.argv[2] && process.argv[3],
  "Usage: bun scripts/qa/operating-modes.mjs CANDIDATE EVIDENCE [80|160]");
const binary = resolve(process.argv[2]);
const evidence = resolve(process.argv[3]);
const columns = Number(process.argv[4] ?? 80);
assert([80, 160].includes(columns), "Width must be 80 or 160");
const rows = columns === 80 ? 24 : 45;
const privateRoot = dirname(process.env.HOME ?? "");
assert.match(privateRoot, /^\/tmp\/corral-herdr-[^/]+$/);
assert.equal(process.env.HOME, join(privateRoot, "home"));
for (const [key, name] of Object.entries({
  XDG_CONFIG_HOME: "config", XDG_DATA_HOME: "data",
  XDG_STATE_HOME: "state", XDG_CACHE_HOME: "cache",
})) assert.equal(process.env[key], join(privateRoot, name), `Unsafe ${key}`);
assert.equal(process.env.CORRAL_HERDR_SESSION, "corral-test");

await mkdir(evidence, { recursive: true });
const fixture = await mkdtemp(join(evidence, "fixture-"));
const env = { ...process.env, CORRAL_ROOT: fixture, CORRAL_HERDR_SESSION: "corral-test" };
// Simulate owner input only in the disposable store. Never inherit agent provenance,
// socket overrides, candidate gate identity, or a PM identity into fixture commands.
const removed = [
  "AI_AGENT", "PI_CODING_AGENT", "CORRAL_PM", "HERDR_CONFIG_PATH", "HERDR_SOCKET_PATH",
  "HERDR_SESSION", "HERDR_ENV", "HERDR_BIN_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID",
  "HERDR_WORKSPACE_ID", "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD",
  "CORRAL_INTEGRATION_BASE",
];
for (const key of removed) delete env[key];
const result = {
  status: "FAIL", binary, fixture, columns, rows, session: "corral-test",
  scenes: [], assertions: [], gaps: [],
  fixtureKind: "Synthetic task lifecycle projections; no coding executors or real landing",
};
const transcript = [];
let workspace, pane, socketPath, viewport, viewportDrain, viewportErrors;
let subscription;
let activeStage;
let serial = 0;
let screen = "";
const configPath = join(fixture, ".corral", "corral.toml");
const quote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
const modal = (text) => text.includes("grant Autopilot?");
const automation = (text) => text.includes("sections") && text.includes("Automation") && !modal(text);
const board = (text) => text.includes("q leave") && !text.includes("sections") &&
  !modal(text) && !text.includes("close the board?") && !text.includes("skip tour");
const selection = (text) => text.split("\n")
  .map((line) => ({ line, marker: line.lastIndexOf("▸ ") }))
  .find(({ marker }) => marker >= 18)
  ?.line.split("▸ ").at(-1)?.trim();
const selected = (text, label) => selection(text)?.startsWith(label);

function bounded(promise, label, milliseconds = 15000, cancel = () => {}) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => { cancel(); reject(new Error(`${label}: timed out`)); }, milliseconds);
    }),
  ]).finally(() => clearTimeout(timer));
}

async function command(program, args, allowFailure = false) {
  const child = Bun.spawn([program, ...args], {
    cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, status] = await bounded(Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]), `${program} ${args.join(" ")}`, 30000, () => child.kill());
  transcript.push({ command: [program, ...args], status, stdout, stderr });
  if (!allowFailure) assert.equal(status, 0, `${program} ${args.join(" ")}\n${stderr}`);
  return { stdout, stderr, status };
}
const cli = async (...args) => (await command(binary, ["--root", fixture, ...args])).stdout;
const herdr = async (...args) =>
  (await command("herdr", ["--session", "corral-test", ...args])).stdout;
const config = async () => Bun.TOML.parse(await readFile(configPath, "utf8"));
const status = async () => JSON.parse(await cli("mode", "status", "--json"));
const saveJSON = (name, value) =>
  writeFile(join(evidence, `${name}.json`), JSON.stringify(value, null, 2) + "\n");

function rpc(method, params) {
  const id = `operating-modes-${++serial}`;
  transcript.push({ rpc: method, params });
  return new Promise((resolveRequest, reject) => {
    const connection = createConnection(socketPath);
    const streaming = method === "events.subscribe";
    if (streaming) { subscription?.destroy(); subscription = connection; }
    let settled = false;
    const fail = (error) => {
      clearTimeout(timer);
      connection.destroy();
      if (!settled) { settled = true; reject(error); }
      else if (streaming) activeStage?.reject(error);
    };
    const timer = setTimeout(() => fail(new Error(`${method}: timed out`)), 15000);
    connection.setEncoding("utf8");
    let buffer = "";
    connection.on("data", (chunk) => {
      buffer += chunk;
      try {
        let end;
        while ((end = buffer.indexOf("\n")) >= 0) {
          const message = JSON.parse(buffer.slice(0, end));
          buffer = buffer.slice(end + 1);
          if (message.id === id) {
            if (message.error) { fail(new Error(JSON.stringify(message.error))); return; }
            clearTimeout(timer);
            settled = true;
            resolveRequest(message.result);
            if (!streaming) connection.end();
          } else if (message.event === "pane.output_matched" &&
              message.data.pane_id === pane && activeStage?.armed) {
            activeStage.receive(message.data.read);
          }
        }
      } catch (error) { fail(error); }
    });
    connection.on("error", fail);
    connection.on("end", () => {
      if (!settled) fail(new Error(`${method}: socket closed before acknowledgement`));
    });
    connection.once("connect", () => connection.write(JSON.stringify({ id, method, params }) + "\n"));
  });
}
const raw = (text) => rpc("pane.send_input", { pane_id: pane, text });

async function capture(name, receipt) {
  let ansi = await herdr("pane", "read", pane, "--source", "visible", "--format", "ansi", "--ansi");
  if (ansi.trimStart().startsWith("{")) {
    const value = JSON.parse(ansi);
    ansi = value.result?.read?.text ?? value.result?.text ?? value.read?.text;
    assert.equal(typeof ansi, "string", "ANSI response must contain a frame, not an RPC envelope");
  }
  const text = Bun.stripANSI(ansi).replaceAll("\r", "").trimEnd() + "\n";
  await writeFile(join(evidence, `${name}.ansi`), ansi);
  await writeFile(join(evidence, `${name}.txt`), text);
  await saveJSON(name, { receipt, text, columns, rows });
  result.scenes.push(name);
  return text;
}

const escapeRegex = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
function markerFor(name) {
  if (name === "startup-tour") return "welcome to corral";
  if (name.endsWith("-close")) return "close the board";
  if (name.endsWith("-exit")) return "OPERATING-MODES-QA-EXIT";
  if (name.includes("preview")) return "grant Autopilot\\?";
  if (name.includes("cancel") || name === "19-stale-dismiss") return "▸ Grant Autopilot";
  if (name.includes("confirmed")) return "(?m)^Autopilot [|]";
  if (name === "07-settings") return "sections";
  if (name === "08-settings-six-link") return "▸ Operating mode.*scope and authority";
  if (name === "08b-settings-editor-return") return "▸ Operating mode.*scope and authority";
  if (name === "03-status-read-only") return "▸ Covers";
  if (name === "09-delegate-dispatch") return "Start & review.*\\[on\\]";
  if (name === "10-delegate-landing") return "Land\\b.*\\[on\\]";
  if (name === "11-delegate-plan") return "Accept plans.*\\[on\\]";
  if (name === "12-ordinary-save") return "Settings saved";
  if (name === "18-stale-rejected") return "(?i)(changed|stale|fresh|review again)";
  if (name === "28-paused") return "(?m)^Autopilot [(]paused[)] [|]";
  if (name === "29-resumed") return "(?m)^Autopilot [|]";
  if (name === "30-assist") return "(?m)^Assist [|]";
  if (/automation|controls|editor$/.test(name)) return "Grant tasks";
  return "q leave";
}
async function stage(name, check, action, keep = true, marker = markerFor(name)) {
  assert(!activeStage, "Stages must be sequential");
  let resolveMatch, rejectMatch;
  const match = new Promise((yes, no) => { resolveMatch = yes; rejectMatch = no; });
  // Register the JS receiver before the RPC, await its acknowledgement, then act.
  // Ignore any initial subscription snapshot until the action is armed.
  activeStage = {
    armed: false, reject: rejectMatch,
    receive(read) {
      activeStage.last = read;
      if (check(read.text)) {
        activeStage.armed = false;
        resolveMatch(read);
      }
    },
  };
  const pending = bounded(match, name);
  pending.catch(() => {});
  try {
    await rpc("events.subscribe", { subscriptions: [{
      type: "pane.output_matched", pane_id: pane, source: "visible",
      match: { type: "regex", value: marker }, strip_ansi: true,
    }] });
    activeStage.armed = true;
    await action();
    const receipt = await pending;
    screen = keep ? await capture(name, receipt) : receipt.text;
    assert(check(screen), `${name}: captured frame no longer matches its receipt`);
    return screen;
  } catch (error) {
    if (activeStage?.last) await saveJSON(`${name}-unmatched-receipt`, activeStage.last);
    throw error;
  } finally {
    activeStage = undefined;
    subscription?.destroy();
    subscription = undefined;
    // Release the bounded await even if the action failed before any output.
    resolveMatch({});
  }
}

async function click(text, needle, linePredicate = () => true) {
  const lines = text.split("\n");
  const y = lines.findIndex((line) => line.includes(needle) && linePredicate(line));
  assert(y >= 0, `Pointer target missing: ${needle}\n${text}`);
  const x = Bun.stringWidth(lines[y].slice(0, lines[y].indexOf(needle))) + 1;
  assert(x <= columns && y < rows, `Pointer target outside ${columns}x${rows}`);
  transcript.push({ mouse: needle, x, y: y + 1 });
  await raw(`\x1b[<0;${x};${y + 1}M\x1b[<0;${x};${y + 1}m`);
}

async function walk(label) {
  // Walk the actual selection rather than assuming an index or viewport position.
  if (selected(screen, label)) return;
  if (!selected(screen, "Operating mode")) {
    await stage(`rewind-${serial}`, (text) => automation(text) && selected(text, "Operating mode"),
      () => raw("\x1b[H"), false, "▸ Operating mode");
  }
  // Overview rows (Stopped, Needs you) appear only when they apply, so the next row is any later label.
  const fields = ["Operating mode", "Covers", "Allowed", "Stopped", "Needs you", "Grant projects",
    "Grant tasks", "Include new tasks", "Ends after", "Max new tasks", "Exclude tasks",
    "Reviewed scopes", "Start & review", "Land", "Accept plans", "Grant Autopilot",
    "Use Assist", "Pause Autopilot", "Resume Autopilot", "Policy details"];
  const seen = new Set();
  for (let step = 0; step < 64; step++) {
    if (selected(screen, label)) return;
    const before = selection(screen);
    assert(before, `No selected Settings row while seeking ${label}\n${screen}`);
    assert(!seen.has(before), `Cannot reach Settings row ${label}; selection repeated: ${before}`);
    seen.add(before);
    const at = fields.findIndex((field) => before.startsWith(field));
    assert(at >= 0 && at + 1 < fields.length, `Unexpected scope editor row: ${before}`);
    await stage(`walk-${serial}`, (text) => automation(text) &&
      Boolean(selection(text)) && selection(text) !== before, () => raw("j"), false,
    "▸ (" + fields.slice(at + 1).map(escapeRegex).join("|") + ")");
  }
  throw new Error(`Settings row not reached: ${label}`);
}

async function edit(label, value) {
  const committed = value.split(",").map((part) => part.trim()).filter(Boolean).sort().join(", ");
  await walk(label);
  await stage(`edit-${serial}`, (text) => selected(text, label) && text.includes("▏"),
    () => raw("\r"), false, escapeRegex(label) + ".*▏");
  await stage(`value-${serial}`, (text) => selected(text, label) &&
    text.includes(value) && text.includes("▏"), () => raw("\x15" + value), false,
  escapeRegex(label) + ".*" + escapeRegex(value) + ".*▏");
  await stage(`field-${serial}`, (text) => selected(text, label) &&
    text.includes(committed) && !text.includes("▏"), () => raw("\r"), false,
  escapeRegex(label) + ".*" + escapeRegex(committed) + "[ ]+│");
}

async function openAutomation(name, action) {
  return stage(name, (text) => automation(text) && text.includes("Grant tasks"), action, true, "Grant tasks");
}
// Select exactly `ids` in the task picker; the cursor row is marked with "›".
async function pickTasks(ids) {
  const picker = (text) => text.includes("what may Autopilot work on?");
  const cursor = (text) => text.split("\n").find((line) => line.includes("› ["));
  await walk("Grant tasks");
  await stage(`picker-${serial}`, picker, () => raw("\r"), false, "what may Autopilot work on");
  const rowOf = (text, id) => text.split("\n").find((line) => line.includes(`] ${id} `));
  const wrong = [...screen.matchAll(/\[(x| )\] (T-\d+) /g)]
    .filter(([, mark, id]) => (mark === "x") !== ids.includes(id)).map(([, , id]) => id);
  for (const id of ids) assert(rowOf(screen, id), `Task ${id} missing from picker\n${screen}`);
  for (const id of wrong) {
    const seen = new Set();
    while (!cursor(screen)?.includes(`] ${id} `)) {
      const before = cursor(screen);
      assert(before && !seen.has(before), `Picker cursor cannot reach ${id}\n${screen}`);
      seen.add(before);
      const lines = screen.split("\n");
      const next = lines[lines.indexOf(before) + 1]?.match(/\] (\S+)/)?.[1];
      assert(next, `No picker row below the cursor\n${screen}`);
      await stage(`pick-move-${serial}`, (text) => picker(text) && cursor(text) !== before,
        () => raw("j"), false, "› \\[.\\] " + escapeRegex(next));
    }
    const was = cursor(screen);
    const mark = ids.includes(id) ? "x" : " ";
    await stage(`pick-${serial}`, (text) => picker(text) && cursor(text) !== was, () => raw(" "), false,
      `› \\[${mark}\\] ${escapeRegex(id)} `);
  }
  const value = "▸ Grant tasks +" + escapeRegex([...ids].sort().join(", ")) + "\\s*│";
  await stage(`picked-${serial}`, (text) => automation(text) && !picker(text) &&
    new RegExp(value).test(text), () => raw("\r"), false, value);
}
async function openPreview(name) {
  await walk("Grant Autopilot");
  const text = await stage(name, modal, () => raw("\r"));
  assert(text.includes("[Grant] y") && text.includes("[Cancel] esc"), "Both modal hit targets must be visible");
  return text;
}
async function cancelPreview(name, pointer = false) {
  await stage(name, (text) => automation(text) && selected(text, "Grant Autopilot"),
    () => pointer ? click(screen, "[Cancel]") : raw("\x1b"));
}
const launch = () => herdr("pane", "run", pane,
  `stty -F /dev/tty cols ${columns} rows ${rows}; env ${removed.map((key) => `-u ${key}`).join(" ")} ` +
  `-u NO_COLOR TERM=xterm-256color COLORTERM=truecolor CORRAL_ROOT=${quote(fixture)} ` +
  `${quote(binary)} --root ${quote(fixture)} board </dev/null; printf '%s%s\\n' 'OPERATING-MODES-QA-' 'EXIT'`);

async function stopBoard(name) {
  await stage(`${name}-close`, (text) => text.includes("close the board?"), () => raw("q"));
  await stage(`${name}-exit`, (text) => text.includes("OPERATING-MODES-QA-EXIT"), () => raw("q"));
}

async function persisted(name, expected = {}) {
  const value = await status();
  const disk = (await config()).automation;
  const diskPolicy = {
    mode: "assist", paused: false, revision: 0, authorization: null, ...disk,
    authority: { dispatch: false, accept: false, plan_accept: false, ...disk?.authority },
    scope: { tasks: {}, exclusions: [], plans: [], ...disk?.scope },
  };
  assert.deepEqual(value.policy, diskPolicy, "Status JSON must match workspace TOML policy");
  for (const [key, wanted] of Object.entries(expected)) assert.deepEqual(value[key], wanted, name + ": " + key);
  assert.equal(value.mode, value.effective.mode);
  assert.equal(value.paused, value.effective.paused);
  await saveJSON(`${name}-status`, value);
  await writeFile(join(evidence, `${name}-corral.toml`), await readFile(configPath));
  result.assertions.push(name);
  return value;
}

async function noGrantChange(name, before) {
  const after = await status();
  assert.deepEqual(after.policy, before.policy, `${name}: policy changed without confirmation`);
  assert.deepEqual(after.approval, before.approval, `${name}: approval changed without confirmation`);
  result.assertions.push(name);
}

async function seed(id, patch) {
  // Only a fresh disposable fixture is ever writable. Preserve the existing task
  // contract/body, and serialize the current schema envelope (JSON is valid YAML).
  const directory = join(fixture, ".corral", "tasks");
  const names = (await readdir(directory)).filter((name) => name.startsWith(`${id}-`) && name.endsWith(".md"));
  assert.equal(names.length, 1, `Unique fixture file for ${id}`);
  const path = join(directory, names[0]);
  const source = await readFile(path, "utf8");
  const envelope = source.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)([\s\S]*)$/);
  assert(envelope, `Versioned task envelope for ${id}`);
  const stored = Bun.YAML.parse(envelope[1]);
  assert.equal(stored.schema_version, 2);
  Object.assign(stored.task, patch);
  await writeFile(path, `---\n${JSON.stringify(stored, null, 2)}\n---\n${envelope[2]}`);
  const readback = JSON.parse(await cli("show", id, "--json"));
  for (const [key, value] of Object.entries(patch)) {
    // Default-valued fields may be omitted by serde.
    assert.deepEqual(readback[key] ?? (value === false ? false : undefined), value, `Fixture ${id}.${key}`);
  }
}

async function grantCLI(id, dispatch = false) {
  const ids = Array.isArray(id) ? id : [id];
  const args = ["mode", "autopilot", ...ids.flatMap((task) => ["--task", task]), ...(dispatch ? ["--dispatch"] : []), "--json"];
  const preview = JSON.parse(await cli(...args));
  assert.match(preview.id, /^[a-f0-9]{64}$/);
  await saveJSON(`cli-preview-${serial++}`, preview);
  return JSON.parse(await cli(...args, "--confirm", preview.id));
}

async function statusScene(name, expected, label, check) {
  const value = await persisted(name, { status: expected });
  if (check) check(value);
  await stage(name, (text) => board(text) && text.includes(label), launch);
  await stopBoard(name);
  return value;
}

// Render actual captured ANSI, not reconstructed or invented TUI frames.
async function renderFrames() {
  assert.equal(typeof Bun.WebView, "function", "PNG replay requires Bun.WebView");
  const [js, css, font] = await Promise.all([
    fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js", { signal: AbortSignal.timeout(15000) }),
    fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css", { signal: AbortSignal.timeout(15000) }),
    readFile("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
  ]);
  assert(js.ok && css.ok, "Pinned xterm replay assets unavailable");
  const script = await js.text(), style = await css.text();
  let html = "";
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
    fetch: () => new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } }) });
  const width = columns * 10 + 32, height = rows * 20 + 32;
  const browser = new Bun.WebView({ width, height });
  try {
    for (const name of result.scenes) {
      const lines = (await readFile(join(evidence, `${name}.ansi`), "utf8")).split(/\r?\n/);
      while (lines.length && !Bun.stripANSI(lines.at(-1)).trim()) lines.pop();
      assert(lines.length <= rows, `${name}: ANSI exceeds the actual viewport`);
      // Each source line is already a terminal row. Absolute positioning avoids
      // a replay-only extra row from Unicode width or right-margin wrap state.
      const ansi = lines.map((line, index) => `\x1b[${index + 1};1H${line}`).join("");
      html = `<!doctype html><meta charset="utf-8"><style>${style}
        @font-face{font-family:QAMono;src:url(data:font/ttf;base64,${font.toString("base64")})}
        body{margin:0;padding:16px;background:#111318;overflow:hidden}
        .xterm-viewport{overflow:hidden!important}</style><div id="terminal"></div>
        <script>${script}</script><script>window.ready=(async()=>{
        await document.fonts.load("15px QAMono");
        const term=window.term=new Terminal({cols:${columns},rows:${rows},
          fontFamily:"QAMono",fontSize:15,lineHeight:1.2,disableStdin:true,scrollback:0,
          theme:{background:"#111318",foreground:"#d7dce2"}});
        term.open(document.getElementById("terminal"));
        await new Promise(resolve=>term.write("\\x1b[?25l\\x1b[?7l"+
          ${JSON.stringify(ansi).replaceAll("<", "\\u003c")},
          ()=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
        })();</script>`;
      await bounded(browser.navigate(`http://127.0.0.1:${server.port}/`), "Replay navigation");
      await bounded(browser.evaluate("window.ready"), "xterm render");
      const cell = await browser.evaluate("term._core._renderService.dimensions.css.cell");
      const actualWidth = Math.ceil(cell.width * columns) + 32;
      const actualHeight = Math.ceil(cell.height * rows) + 32;
      await browser.cdp("Emulation.setDeviceMetricsOverride", {
        width: actualWidth, height: actualHeight, deviceScaleFactor: 1, mobile: false,
      });
      await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
      const png = Buffer.from(await (await bounded(browser.screenshot(), "PNG capture")).arrayBuffer());
      assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
      assert.equal(png.readUInt32BE(16), actualWidth);
      assert.equal(png.readUInt32BE(20), actualHeight);
      await writeFile(join(evidence, `${name}.png`), png);
    }
  } finally {
    await browser.close();
    await server.stop(true);
  }
  result.frames = "Actual herdr ANSI replayed through xterm.js 5.5.0";
}

try {
  socketPath = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socketPath?.startsWith(`${privateRoot}/`) &&
    socketPath.includes("/sessions/corral-test/"), "Refuse a non-private herdr socket");
  await mkdir(join(fixture, "app"));
  await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
  await cli("init");
  await cli("config", "set", "dispatch.paused", "true");
  await cli("config", "set", "ui.mouse", "true");
  // Explicit owner exceptions must survive delegation.
  await cli("config", "set", "review.accept", "owner");
  await cli("config", "set", "review.plan_accept", "owner");
  const task = (await cli("new", "Operating modes fixture", "-p", "app")).trim();
  const excluded = (await cli("new", "Excluded fixture", "-p", "app")).trim();
  const included = [task];
  // Enough held scope members to require real scrolling at both viewport sizes.
  for (let index = 1; index <= 3; index++) {
    included.push((await cli("new", `Held scope companion ${index}`, "-p", "app")).trim());
  }
  for (const id of included) await seed(id, { dispatch_hold: true });
  await seed(excluded, { dispatch_hold: true });
  const initial = await persisted("initial-assist", { mode: "assist" });
  const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
    "--label", `operating-modes-${columns}`, "--no-focus")).result;
  workspace = created.workspace.workspace_id;
  pane = created.root_pane.pane_id;
  Object.assign(result, { workspace, pane });
  viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
    "control", pane, "--cols", String(columns), "--rows", String(rows)],
  { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  viewportErrors = new Response(viewport.stderr).text();
  const reader = viewport.stdout.getReader();
  const ready = await bounded(reader.read(), "Viewport acknowledgement");
  assert(!ready.done, "Viewport closed before acknowledgement");
  await writeFile(join(evidence, "viewport.json"), new TextDecoder().decode(ready.value));
  viewportDrain = (async () => { while (!(await reader.read()).done) {} })();
  await stage("startup-tour", (text) => text.includes("welcome to corral") && text.includes("Skip tour"), launch, false);
  await stage("01-board-assist", board, () => raw("\x1b"));
  await openAutomation("02-O-automation", () => raw("O"));
  await noGrantChange("Opening O is read only", initial);
  // Enter on the initial status row must not act; j provides the observable
  // redraw barrier without depending on an output event for a no-op.
  await stage("03-status-read-only", (text) => automation(text) && selected(text, "Covers"),
    () => raw("\rj"));
  await noGrantChange("Operating mode row is read only", initial);
  await stage("04-back-to-board", board, () => raw("\x1b"));
  const header = screen.split("\n").find((line) => line.includes("O mode"));
  assert(header, "Operating header must be visible");
  const headerLabel = header.includes("Assist") ? "Assist" : "assist";
  await openAutomation("05-header-automation", () => click(screen, headerLabel, (line) => line === header));
  await noGrantChange("Header opens without granting", initial);
  await stage("06-board", board, () => raw("\x1b"));
  await stage("07-settings", (text) => text.includes("sections"), () => raw(","));
  await stage("08-settings-six-link", (text) => automation(text) && selected(text, "Operating mode"),
    () => raw("6"));
  await openAutomation("08a-settings-six-editor", () => raw("\r"));
  await noGrantChange("Settings 6 matches O without granting", initial);
  // A nested Settings entry may return to ordinary Settings, whereas O returns
  // directly to the board. Observe which view actually owns the back stack.
  await stage("08b-settings-editor-return", (text) => board(text) ||
    (automation(text) && selected(text, "Operating mode") && !text.includes("Use Assist")),
  () => raw("\x1b"));
  if (!board(screen)) await stage("08c-settings-return-board", board, () => raw("\x1b"));
  await openAutomation("08d-grant-editor", () => raw("O"));

  await pickTasks([...included, excluded]);
  await edit("Exclude tasks", excluded);
  await walk("Start & review");
  await stage("09-delegate-dispatch", (text) => selected(text, "Start & review") &&
    /Start & review[^\n]*\[on\]/.test(text), () => raw("\r"));
  await walk("Land");
  await stage("10-delegate-landing", (text) => selected(text, "Land") &&
    /Land\b[^\n]*\[on\]/.test(text), () => raw("\r"));
  await walk("Accept plans");
  await stage("11-delegate-plan", (text) => selected(text, "Accept plans") &&
    /Accept plans[^\n]*\[on\]/.test(text), () => raw("\r"));
  await stage("12-ordinary-save", (text) => text.includes("Settings saved"), () => raw("s"));
  await noGrantChange("Ordinary settings save does not mint grant", initial);
  const shown = await openPreview("13-preview-keyboard-cancel");
  const args = ["mode", "autopilot", ...[...included, excluded].flatMap((id) => ["--task", id]), "--exclude", excluded,
    "--dispatch", "--accept", "--plan-accept", "--json"];
  const preview = JSON.parse(await cli(...args));
  for (const id of included) assert(shown.includes(id), `TUI preview must list CLI-previewed task ${id}`);
  assert.deepEqual(Object.keys(preview.policy.scope.tasks), included);
  assert(preview.constraints[task].owner_acceptance && preview.constraints[task].owner_plan_acceptance);
  await saveJSON("reviewed-preview", preview);
  await cancelPreview("14-keyboard-cancel");
  await noGrantChange("Escape cancellation writes no authority", initial);
  await openPreview("15-preview-pointer-cancel");
  await cancelPreview("16-pointer-cancel", true);
  await noGrantChange("Pointer cancellation writes no authority", initial);

  await openPreview("17-preview-stale");
  // Another process changes a policy input after preview, before confirmation.
  // The next subscription is acknowledged before both that write and Enter.
  await stage("18-stale-rejected", (text) => modal(text) &&
    /changed|stale|fresh|review again/i.test(text), async () => {
    const rounds = (await config()).review?.max_rounds ?? preview.constraints[task].review.max_rounds;
    assert(Number.isInteger(rounds), "Current review round limit must be numeric");
    await cli("config", "set", "review.max_rounds", String(rounds + 1));
    await raw("y");
  });
  await noGrantChange("Stale preview cannot grant", initial);
  await cancelPreview("19-stale-dismiss");
  await openPreview("20-preview-keyboard-confirm");
  await stage("25-keyboard-confirmed", (text) => automation(text) &&
    text.includes("\nAutopilot |"), () => raw("y"));
  const granted = await persisted("keyboard-grant", { mode: "autopilot" });
  assert.deepEqual(Object.keys(granted.policy.scope.tasks), included);
  assert.deepEqual(granted.policy.scope.exclusions, [excluded]);
  assert.deepEqual(granted.policy.authority, { dispatch: true, accept: true, plan_accept: true });
  assert.equal(granted.approval.authorization, granted.policy.authorization);
  assert.equal(granted.approval.revision, granted.policy.revision);
  assert.equal(granted.effective.tasks[task].review.accept, "owner");
  assert.equal(granted.effective.tasks[task].review.plan_accept, "owner");
  const future = (await cli("new", "Future ungranted fixture", "-p", "app")).trim();
  await seed(future, { dispatch_hold: true });
  assert.deepEqual(Object.keys((await status()).policy.scope.tasks), included);
  await stage("26-granted-board", (text) => board(text) && text.includes("Autopilot"), () => raw("\x1b"));
  await openAutomation("27-pause-controls", () => raw("O"));
  await walk("Pause Autopilot");
  await stage("28-paused", (text) => automation(text) &&
    text.includes("\nAutopilot (paused) |"), () => raw("\r"));
  await persisted("paused", { paused: true, status: "needs_your_decision" });
  await walk("Resume Autopilot");
  await stage("29-resumed", (text) => automation(text) &&
    text.includes("\nAutopilot |"), () => raw("\r"));
  await persisted("resumed", { paused: false });
  await walk("Use Assist");
  await stage("30-assist", (text) => automation(text) &&
    text.includes("\nAssist |"), () => raw("\r"));
  await persisted("withdrawn", { mode: "assist" });
  // Re-enter through O to get a fresh draft; all fixture tasks remain held.
  await stage("31-assist-board", (text) => board(text) && text.includes("Assist"), () => raw("\x1b"));
  await openAutomation("32-pointer-grant-controls", () => raw("O"));
  await pickTasks([task]);
  await edit("Exclude tasks", excluded);
  for (const label of ["Start & review", "Land", "Accept plans"]) {
    await walk(label);
    await stage(`pointer-no-${label}`, (text) => selected(text, label) &&
      text.split("\n").find((line) => line.includes(label))?.includes("[off]"),
    () => raw("\r"), false, escapeRegex(label) + ".*\\[off\\]");
  }
  await openPreview("33-pointer-preview");
  await stage("34-pointer-confirmed", (text) => automation(text) &&
    text.includes("\nAutopilot |"), () => click(screen, "[Grant] y"));
  const pointerGrant = await persisted("pointer-grant", { mode: "autopilot" });
  assert.deepEqual(pointerGrant.policy.authority, { dispatch: false, accept: false, plan_accept: false });
  await stage("35-restart-board-return", board, () => raw("\x1b"));
  await stopBoard("36-before-restart");
  await stage("37-restarted", (text) => board(text) && text.includes("Autopilot"), launch);
  const restarted = await persisted("restart");
  assert.deepEqual(restarted.policy, pointerGrant.policy);
  assert.deepEqual(restarted.approval, pointerGrant.approval);
  await stopBoard("38-after-restart");

  // No actual execution is needed for status readbacks. Running/Done below are
  // explicitly synthetic lifecycle projections, with no claimed run receipts.
  // Stop the TUI before fixture writes to avoid competing store writers.
  await seed(task, { status: "running", dispatch_hold: false });
  await grantCLI(task);
  await statusScene("39-working-projection", "working", "Working",
    (value) => assert.equal(value.progress.running, 1));
  await seed(task, { status: "backlog" });
  await grantCLI(task);
  await statusScene("40-waiting-projection", "waiting", "Waiting");
  await seed(task, { status: "needs_me" });
  await grantCLI(task);
  await statusScene("41-owner-decision-projection", "needs_your_decision", "Needs your decision",
    (value) => assert(value.decisions.some((decision) => decision.task === task)));
  await seed(included[1], { status: "running", dispatch_hold: false });
  await grantCLI([task, included[1]]);
  await statusScene("41a-decision-independent-work", "needs_your_decision", "independent work continues",
    (value) => {
      assert.equal(value.independent_work_continues, true);
      // Needs me retains its worker slot as well as the independent Running member.
      assert.equal(value.progress.running, 2);
      assert(value.decisions.some((decision) => decision.task === task));
    });
  await seed(included[1], { status: "backlog", dispatch_hold: true });
  await seed(task, { status: "done" });
  await grantCLI(task);
  await statusScene("42-complete-projection", "complete", "Complete",
    (value) => assert.equal(value.progress.accepted, value.progress.total));
  // Exercise the canonical dispatch CLI preview/confirmation on held tasks only.
  await seed(task, { status: "backlog", dispatch_hold: true });
  await grantCLI(task, true);
  await statusScene("43-held-dispatch-grant", "needs_your_decision", "Needs your decision");
  for (const id of [...included, excluded, future]) {
    assert.equal((JSON.parse(await cli("show", id, "--json")).runs ?? []).length, 0,
      "The UI journey must not launch coding executors");
  }
  result.status = "PASS";
} catch (error) {
  result.error = String(error.stack ?? error);
  if (pane) {
    try { await capture("failure", null); }
    catch (captureError) { result.captureError = String(captureError); }
  }
} finally {
  subscription?.destroy();
  if (viewport) {
    try {
      viewport.kill();
      await bounded(viewport.exited, "Viewport shutdown");
      await bounded(Promise.all([viewportDrain, viewportErrors]), "Viewport streams closed");
      result.viewportErrors = await viewportErrors;
    } catch (error) { result.viewportError = String(error); result.status = "FAIL"; }
  }
  if (workspace) {
    try { await herdr("workspace", "close", workspace); }
    catch (error) { result.cleanupError = String(error); result.status = "FAIL"; }
  }
  try { await renderFrames(); }
  catch (error) {
    result.gaps.push(`Rendered PNG frames unavailable: ${error.stack ?? error}`);
    result.status = "FAIL";
  }
  // Retain receipts/config/frames, remove only this run's disposable store.
  await rm(fixture, { recursive: true, force: true });
  result.fixtureCleanup = "Removed this run's fixture and closed only its private workspace";
  await saveJSON("commands", transcript);
  await saveJSON("result", result);
}
console.log(`${result.status}: ${evidence}`);
if (result.error) console.error(result.error);
for (const gap of result.gaps) console.error(gap);
process.exitCode = result.status === "PASS" ? 0 : 1;
