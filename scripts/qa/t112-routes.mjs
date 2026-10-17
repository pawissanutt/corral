// Disposable live QA. Run from the repo root, inside an ALREADY STARTED
// scripts/test-herdr.sh private environment. Never starts a server or agent.
// bun scripts/qa/t112-routes.mjs /absolute/candidate [--baseline /absolute/baseline]
// Optional: --screenshots=off (default: try existing stage-screenshots.mjs).
// UI contract: detail r -> Route, p/v -> picker, h/l/arrows -> selection,
// Enter -> save, Esc -> cancel; buttons "Pin implementer" and "Set reviewer".
// Picker options must expose their profile/keyword names as clickable text.
import assert from "node:assert/strict";
import { createConnection } from "node:net";
import { createHash } from "node:crypto";
import { access, lstat, mkdir, mkdtemp, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { stripVTControlCharacters as plain } from "node:util";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const candidate = args.shift();
assert(candidate && !candidate.startsWith("-"),
  "Usage: bun scripts/qa/t112-routes.mjs CANDIDATE [--baseline BINARY] [--screenshots=off]");
let baseline, screenshots = true, samples = 20;
while (args.length) {
  const arg = args.shift();
  if (arg === "--baseline") {
    baseline = args.shift();
    assert(baseline && !baseline.startsWith("-"), "--baseline needs a binary");
  } else if (arg === "--screenshots=off") screenshots = false;
  else if (arg === "--samples") {
    samples = Number(args.shift());
    assert(Number.isInteger(samples) && samples >= 0 && samples <= 100, "--samples must be 0..100");
  }
  else throw new Error(`Unknown argument: ${arg}`);
}
assert(process.env.CORRAL_HERDR_SESSION === "corral-test", "Require private corral-test environment");
const home = await realpath(process.env.HOME ?? "");
const privateRoot = dirname(home);
assert(/^\/tmp\/corral-herdr-[^/]+\/home$/.test(home), "Source scripts/test-herdr.sh first");
for (const [key, leaf] of Object.entries({
  XDG_CONFIG_HOME: "config", XDG_DATA_HOME: "data", XDG_STATE_HOME: "state", XDG_CACHE_HOME: "cache",
})) assert.equal(await realpath(process.env[key] ?? ""), join(privateRoot, leaf), `Non-private ${key}`);
assert(!process.env.HERDR_CONFIG_PATH, "Refuse a herdr config override");
const binaries = [{ label: "candidate", binary: await realpath(resolve(candidate)) }];
if (baseline) binaries.unshift({ label: "baseline", binary: await realpath(resolve(baseline)) });
for (const { binary } of binaries) await access(binary, constants.X_OK);

// Reject symlinks before creating anything. Every run gets a fresh exclusive root;
// no caller-supplied store path, existing fixture, or owner data is ever reset.
for (const dir of [".omo", ".omo/evidence", ".omo/evidence/T-112"]) {
  const path = join(repo, dir);
  try { assert((await lstat(path)).isDirectory(), `Refuse non-directory/symlink: ${path}`); }
  catch (error) {
    if (error.code !== "ENOENT") throw error;
    await mkdir(path);
  }
}
const evidence = await mkdtemp(join(repo, ".omo/evidence/T-112/routes-"));
const fixture = join(evidence, "fixture");
await mkdir(fixture);
const env = { ...process.env, CORRAL_ROOT: fixture };
for (const key of ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID",
  "HERDR_SESSION", "HERDR_ENV", "HERDR_BIN_PATH",
  "CORRAL_GATE_ATTEMPT", "CORRAL_CANDIDATE_HEAD", "CORRAL_INTEGRATION_BASE"]) delete env[key];
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const transcript = [];
const report = { status: "FAIL", evidence, fixture, runs: [], comparison: [],
  timing: "performance.now before input RPC to first changed matching visible-output event; capture I/O excluded",
  assumptions: [
    "Existing plugin-free private corral-test server supplied by scripts/test-herdr.sh; never reused or stopped.",
    "Proposed Route UI contract and clickable picker keyword/profile labels; mismatches fail with captures.",
    "Baseline compares only common board/detail actions, not the new Route UI.",
    "Twenty round trips between Backlog and Ready per density and binary on a 110-task board.",
    "Ready fixtures are deliberately unqueued and held; dispatch and review automation are disabled.",
  ] };
console.log(`Evidence: ${evidence}`);

async function command(program, argv, timeout = 30000) {
  const child = Bun.spawn([program, ...argv], {
    cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  // Timers are failure deadlines only. There are no sleeps or polling loops.
  const timer = setTimeout(() => child.kill(), timeout);
  try {
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    transcript.push({ program, argv, code, stdout, stderr });
    assert.equal(code, 0, `${program} ${argv.join(" ")} failed: ${stderr}`);
    return stdout;
  } finally { clearTimeout(timer); }
}
const herdr = (...argv) => command("herdr", ["--session", "corral-test", ...argv]);
const cli = (binary, ...argv) => command(binary, ["--root", fixture, ...argv]);
const configPath = join(fixture, ".corral/corral.toml");
const taskPath = id => join(fixture, ".corral/tasks", `${id}-routes.md`);
let tasks;

function config(density) {
  return `[projects.app]
path = ${JSON.stringify(join(fixture, "app"))}
[defaults]
profile = "gpt-sol"
[dispatch]
paused = true
auto_backlog = false
promote_ideas = "never"
[review]
autopilot = false
reviewer = "cross-vendor"
[herdr]
session = "corral-test"
[harnesses.qa]
command = "false {model_args}"
flags = "generic"
model_flag = "--model"
[profiles.gpt-sol]
harness = "qa"
model = "openai/fixture-gpt-sol"
vendor = "openai"
[profiles.sonnet]
harness = "qa"
model = "anthropic/fixture-sonnet"
vendor = "anthropic"
[pm]
agent = "sonnet"
[routing.reviewer_by_vendor]
anthropic = "sonnet"
[board.card]
fields = ["profile"]
density = "${density}"
`;
}

async function prepare() {
  await mkdir(join(fixture, "app"));
  await command("git", ["init", "-q", "-b", "main", join(fixture, "app")]);
  const binary = binaries.find(b => b.label === "candidate").binary;
  await cli(binary, "init");
  await writeFile(configPath, config("rich"));
  await cli(binary, "new", "Synthetic route seed", "-p", "app", "--body", "T-112 disposable QA only");
  const seed = JSON.parse(await cli(binary, "list", "--all", "--json"))[0];
  assert(seed?.id, "Candidate must expose task JSON");
  const files = await readdir(join(fixture, ".corral/tasks"));
  assert.equal(files.length, 1, "Only the freshly created seed may be removed");
  await rm(join(fixture, ".corral/tasks", files[0]));
  const specs = [
    ["T-001", "OpenAI route", "backlog", "gpt-sol", null, false],
    ["T-002", "Anthropic route", "ready", "sonnet", "none", false],
    ["T-003", "Same vendor warning", "backlog", "gpt-sol", "gpt-sol", false],
    // Only the Anthropic reviewer mapping exists: a Sonnet implementer cannot
    // get a cross-vendor reviewer. This is a missing-map warning, not a typo.
    ["T-004", "Unmapped reviewer warning", "ready", "sonnet", "cross-vendor", false],
    ["T-005", "Plan required route", "backlog", "gpt-sol", "cross-vendor", true],
    ["T-006", "Default fallback warning", "backlog", null, "none", false],
  ];
  // T-110's current-size workload has 110 cards. Keep route scenarios visible
  // while filling out the same board size with deterministic historical tasks.
  for (let i = 7; i <= 110; i++) {
    specs.push([`T-${String(i).padStart(3, "0")}`, `Historical fixture ${i}`, "done", "gpt-sol", "none", false]);
  }
  tasks = specs.map(([id, title, status, profile, reviewer, plan_required]) => ({
    ...seed, id, title, status, profile, reviewer, plan_required, priority: "P2",
    queued: false, dispatch_hold: true, runs: [], artifacts: [], plans: [], plan_runs: [],
    created: "2026-10-17T00:00:00Z", updated: "2026-10-17T00:00:00Z",
    labels: ["synthetic", "T-112"], kind: "implementation",
  }));
  report.workloadSha256 = createHash("sha256").update(JSON.stringify(tasks)).digest("hex");
}

async function reset(density) {
  await writeFile(configPath, config(density));
  for (const task of tasks) await writeFile(taskPath(task.id),
    `---\n${JSON.stringify(task, null, 2)}\n---\n# Synthetic route acceptance\nNo owner data; no dispatch.\n`);
  await writeFile(join(fixture, ".corral/events.jsonl"), "");
  await mkdir(join(fixture, ".corral/state"), { recursive: true });
  await writeFile(join(fixture, ".corral/state/tour-seen"), "");
  await writeFile(join(fixture, ".corral/state/harnesses.json"), JSON.stringify({
    detected_at: "2026-10-17T00:00:00Z",
    harnesses: [{ key: "qa", program: "false", path: "/usr/bin/false", version: null,
      models: ["openai/fixture-gpt-sol", "anthropic/fixture-sonnet"],
      model_source: "listed", efforts: [], takes_effort: false, notes: [] }],
  }));
}

class Capture {
  constructor(binary, label, density, socket) {
    Object.assign(this, { binary, socket });
    this.directory = join(evidence, `${label}-${density}`);
    this.result = { status: "FAIL", label, density, binary, columns: 160, rows: 30, scenes: [], samples: [] };
    this.serial = 0;
    this.screen = "";
    this.styled = "";
    this.onOutput = () => {};
    this.onFailure = () => {};
  }

  rpc(method, params) {
    const id = `t112-${++this.serial}`;
    return new Promise((accept, reject) => {
      const connection = createConnection(this.socket);
      const streaming = method === "events.subscribe";
      if (streaming) {
        this.subscription?.removeAllListeners();
        this.subscription?.destroy();
        this.subscription = connection;
      }
      let acknowledged = false, buffer = "";
      const fail = error => {
        clearTimeout(timer);
        connection.destroy();
        if (acknowledged && streaming) this.onFailure(error);
        else reject(error);
      };
      const timer = setTimeout(() => fail(new Error(`${method}: deadline exceeded`)), 20000);
      connection.setEncoding("utf8");
      connection.on("data", chunk => {
        buffer += chunk;
        let end;
        while ((end = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, end);
          buffer = buffer.slice(end + 1);
          try {
            const message = JSON.parse(line);
            transcript.push({ method, receivedAt: performance.now(), message });
            if (message.id === id) {
              clearTimeout(timer);
              if (message.error) { fail(new Error(JSON.stringify(message.error))); return; }
              acknowledged = true;
              if (!streaming) connection.end();
              accept(message.result);
            } else if (message.event === "pane.output_matched" && message.data.pane_id === this.pane) {
              this.onOutput(message.data.read);
            }
          } catch (error) { fail(error); }
        }
      });
      connection.on("error", fail);
      connection.on("end", () => {
        if (!acknowledged || streaming) fail(new Error(`${method}: socket closed`));
      });
      connection.once("connect", () => connection.write(JSON.stringify({ id, method, params }) + "\n"));
    });
  }

  raw(text) { return this.rpc("pane.send_input", { pane_id: this.pane, text }); }

  async capture(name, read) {
    this.styled = read.text;
    this.screen = plain(read.text);
    await writeFile(join(this.directory, `${name}.txt`), this.screen);
    // Capture real emulator ANSI, not reconstructed colors or invented screenshots.
    const styled = (await this.rpc("pane.read", {
      pane_id: this.pane, source: "visible", format: "ansi", strip_ansi: false,
    })).read;
    await writeFile(join(this.directory, `${name}.ansi`), styled.text);
    await writeFile(join(this.directory, `${name}.json`), JSON.stringify(read, null, 2) + "\n");
    this.result.scenes.push(name);
  }

  async stage(name, check, action, capture = true) {
    let timer, started, active = false;
    const previous = this.styled;
    const matched = new Promise((accept, reject) => {
      timer = setTimeout(() => reject(new Error(`${name}: visible state not observed\n${this.screen}`)), 20000);
      this.onFailure = reject;
      this.onOutput = read => {
        const text = plain(read.text);
        if (active && read.text !== previous && check(text)) {
          active = false;
          const visibleAt = performance.now();
          this.result.samples.push({ name, started, visibleAt, ms: visibleAt - started });
          accept(read);
        }
      };
    });
    matched.catch(() => {});
    try {
      const marker = name === "01-board" ? (this.result.label === "candidate" ? "gpt-sol -> sonnet" : "Backlog") :
        name.startsWith("nav-ready-") ? "▌[ ⚠]*(T-)?002" :
        name.startsWith("nav-backlog-") ? "▌[ ⚠]*(T-)?001" :
        name.startsWith("select-") ? "▌[ ⚠]*(T-)?" + name.slice(-3) :
        name === "05-implementer-right" || name === "07-implementer-arrow" ? "\\[sonnet\\]" :
        name === "06-implementer-left" || name === "10-implementer-select" ? "\\[gpt-sol\\]" :
        name === "04-implementer-keyboard" || name === "09-implementer-mouse" ? "╭.*Pin implementer" :
        name === "13-reviewer-down" ? "> cross-vendor" :
        name === "14-reviewer-up" ? "> inherit" :
        name.endsWith("-picker") || name === "12-reviewer-keyboard" ? "╭.*Set reviewer" :
        name.startsWith("reviewer-") && name.endsWith("-select") ? "> " + name.slice(9, -7) :
        name === "route-T-003" ? "Warning: reviewer uses the same vendor" :
        name === "route-T-004" ? "Warning:.*(reviewer|vendor)" :
        name === "route-T-005" ? "Planner source:" :
        /(?:^|-)board(?:-|$)/.test(name) ? "q leave" :
        name === "02-detail-keyboard" || /^detail-T-/.test(name) ? "Spec" :
        name.startsWith("detail-") || name === "16-detail-return" ? "r route" :
        "p pin implementer";
      await this.rpc("events.subscribe", { subscriptions: [{
        type: "pane.output_matched", pane_id: this.pane, source: "visible",
        match: { type: "regex", value: marker }, strip_ansi: true,
      }] });
      // Subscription ACK precedes input, including startup and mouse actions.
      started = performance.now();
      active = true;
      await action();
      const read = await matched;
      if (capture) await this.capture(name, read);
      else { this.styled = read.text; this.screen = plain(read.text); }
      console.log(`${this.result.label}/${this.result.density}/${name}: ${this.result.samples.at(-1).ms.toFixed(2)} ms`);
    } finally {
      clearTimeout(timer);
      this.onOutput = () => {};
      this.onFailure = () => {};
    }
  }

  click(needle, option = false) {
    const lines = this.screen.split("\n");
    const regex = option ? new RegExp(`(?<![\\w-])${needle}(?![\\w-])`, "i") : null;
    const targets = lines.flatMap((line, row) => {
      const col = option ? line.search(regex) : line.indexOf(needle);
      return col < 0 ? [] : [{ row, col, line }];
    });
    assert(targets.length, `Missing clickable ${needle}\n${this.screen}`);
    // Picker options can repeat in the underlying route. Prefer a row exposing
    // multiple options; otherwise use the final matching row of the picker.
    const hit = option
      ? targets.find(t => /inherit.*(?:pm|gpt-sol)|gpt-sol.*sonnet/i.test(t.line)) ?? targets.at(-1)
      : targets[0];
    const x = Bun.stringWidth(hit.line.slice(0, hit.col)) + 1, y = hit.row + 1;
    return this.raw(`\x1b[<0;${x};${y}M\x1b[<0;${x};${y}m`);
  }

  async start() {
    await mkdir(this.directory);
    const created = JSON.parse(await herdr("workspace", "create", "--cwd", fixture,
      "--label", `T-112 ${this.result.label} ${this.result.density}`, "--no-focus")).result;
    this.workspace = created.workspace.workspace_id;
    this.pane = created.root_pane.pane_id;
    this.viewport = Bun.spawn(["herdr", "--session", "corral-test", "terminal", "session",
      "control", this.pane, "--cols", "160", "--rows", "30"],
    { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    this.viewportErrors = new Response(this.viewport.stderr).text();
    const reader = this.viewport.stdout.getReader();
    let timer;
    try {
      const first = await Promise.race([reader.read(), new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("Viewport control deadline exceeded")), 20000);
      })]);
      assert(!first.done, "Viewport control closed");
      await writeFile(join(this.directory, "viewport.json"), new TextDecoder().decode(first.value));
    } finally { clearTimeout(timer); }
    this.viewportOutput = (async () => { while (!(await reader.read()).done) {} })();
    await this.stage("01-board", s => board(s) &&
      (this.result.label === "baseline" || s.includes("gpt-sol -> sonnet")),
      () => herdr("pane", "run", this.pane,
      `env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor ${quote(this.binary)} --root ${quote(fixture)} board </dev/null`));
    if (this.result.label === "candidate") assert(this.screen.includes("gpt-sol -> sonnet"),
      "Candidate must render the route pair");
    for (let sample = 0; sample < samples; sample++) {
      await this.stage(`nav-ready-${sample}`, s => board(s) && selected(s, "T-002"),
        () => this.raw("l"), false);
      await this.stage(`nav-backlog-${sample}`, s => board(s) && selected(s, "T-001"),
        () => this.raw("h"), false);
    }
  }

  async open(id, keyboard = false) {
    if (keyboard) {
      await this.stage("02-detail-keyboard", detail(id), () => this.raw("\r"));
    } else {
      // A non-selected card's first click selects; its second opens. Observe
      // selection before the second click, never rely on double-click timing.
      const label = this.screen.includes(id) ? id : id.slice(2);
      await this.stage(`select-${id}`, s => board(s) && selected(s, id), () => this.click(label));
      await this.stage(`detail-${id}`, detail(id), () => this.click(label));
    }
  }

  async routes() {
    await this.stage("03-route-keyboard", route, () => this.raw("r"));
    await this.routeParity("T-001");
    await this.stage("04-implementer-keyboard", implementerPicker, () => this.raw("p"));
    await this.stage("05-implementer-right", implementerPicker, () => this.raw("l"));
    await this.stage("06-implementer-left", implementerPicker, () => this.raw("h"));
    await this.stage("07-implementer-arrow", implementerPicker, () => this.raw("\x1b[C"));
    await this.stage("08-implementer-save", route, () => this.raw("\r"));
    await this.assertField("T-001", "profile", "sonnet");
    await this.stage("09-implementer-mouse", implementerPicker, () => this.click("Pin implementer"));
    await this.stage("10-implementer-select", implementerPicker, () => this.click("gpt-sol", true));
    await this.stage("11-implementer-mouse-save", route, () => this.raw("\r"));
    await this.assertField("T-001", "profile", "gpt-sol");
    await this.stage("12-reviewer-keyboard", reviewerPicker, () => this.raw("v"));
    await this.stage("13-reviewer-down", reviewerPicker, () => this.raw("\x1b[B"));
    await this.stage("14-reviewer-up", reviewerPicker, () => this.raw("\x1b[A"));
    await this.stage("15-reviewer-cancel", route, () => this.raw("\x1b"));
    await this.assertField("T-001", "reviewer", null);
    for (const value of ["pm", "cross-vendor", "none", "sonnet", "gpt-sol", "inherit"]) {
      await this.stage(`reviewer-${value}-picker`, reviewerPicker, () => this.click("Set reviewer"));
      await this.stage(`reviewer-${value}-select`, reviewerPicker, () => this.click(value, true));
      await this.stage(`reviewer-${value}-save`, route, () => this.raw("\r"));
      await this.assertField("T-001", "reviewer", value === "inherit" ? null : value);
    }
    await this.stage("16-detail-return", detail("T-001"), () => this.raw("\x1b"));
    await this.stage("17-board-return", board, () => this.raw("\x1b"));
    for (const [id, marker] of [["T-003", /reviewer uses the same vendor/i],
      ["T-004", /no cross.vendor|no reviewer|missing.*map|cannot.*reviewer|reviewer_by_vendor|requires.*vendor|not configured/i],
      ["T-005", /planner/i]]) {
      await this.open(id);
      await this.stage(`route-${id}`, s => route(s) && marker.test(s), () => this.raw("r"));
      await this.routeParity(id);
      if (id === "T-005") assert(/plan.?review|review.*plan/i.test(this.screen), "Plan reviewer must be visible");
      await this.stage(`detail-from-${id}`, detail(id), () => this.raw("\x1b"));
      await this.stage(`board-from-${id}`, board, () => this.raw("\x1b"));
    }
  }

  async assertField(id, key, expected) {
    const task = JSON.parse(await cli(this.binary, "list", "--all", "--json")).find(t => t.id === id);
    assert.equal(task[key] ?? null, expected, `${id}.${key} must persist`);
  }

  async routeParity(id) {
    const expected = JSON.parse(await cli(this.binary, "route", id, "--json"));
    await writeFile(join(this.directory, `route-${id}-cli.json`), JSON.stringify(expected, null, 2));
    const text = this.screen.split(/\s+/).join(" ");
    const source = r => r.run ? `planner run ${r.run}` : ({
      override: "dispatch override", task: "task profile", project_default: "project default_profile",
      defaults: "[defaults] profile", fallback: "fallback omo",
    }[r.source] ?? `routing rule ${r.index}${r.name ? ` (${r.name})` : ""}`);
    const has = value => assert(text.includes(value), `${id}: route detail differs from CLI: ${value}\n${this.screen}`);
    has(`Implementer: ${expected.profile} · ${expected.harness} · model ${expected.model ?? "default"} · effort ${expected.effort ?? "default"}`);
    has(`Implementer source: ${source(expected.route)}`);
    for (const warning of expected.warnings) has(`Warning: ${warning}`);
    const mapping = (role, value) => {
      has(`${role}: ${value.selection} -> ${value.profile ?? value.selection}`);
      has(`harness ${value.harness ?? "-"} · model ${value.model ?? "default"} · vendor ${value.vendor ?? "unknown"}`);
      has(`${role} source: ${value.source} · policy ${value.policy_source}`);
      for (const warning of value.warnings) has(`Warning: ${warning}`);
    };
    mapping("Reviewer", expected.review.reviewer);
    if (expected.review.planner) {
      const p = expected.review.planner;
      has(`Planner: ${p.profile} · ${p.harness} · model ${p.model ?? "default"} · vendor ${p.vendor ?? "unknown"}`);
      has(`Planner source: ${source(p.source)}`);
      for (const warning of p.warnings) has(`Warning: ${warning}`);
    }
    if (expected.review.plan_reviewer) mapping("Plan reviewer", expected.review.plan_reviewer);
    (this.result.cliParity ??= []).push(id);
  }

  async close() {
    if (this.result.status !== "PASS" && this.pane) {
      try {
        await this.capture("failure", (await this.rpc("pane.read", {
          pane_id: this.pane, source: "visible", strip_ansi: false,
        })).read);
      } catch (error) { this.result.captureError = String(error); }
    }
    this.subscription?.removeAllListeners();
    this.subscription?.destroy();
    if (this.viewport) {
      this.viewport.kill();
      await this.viewport.exited;
      await this.viewportOutput;
      this.result.viewportStderr = await this.viewportErrors;
    }
    if (this.workspace) await herdr("workspace", "close", this.workspace);
    await writeFile(join(this.directory, "result.json"), JSON.stringify(this.result, null, 2) + "\n");
  }
}

const board = s => s.includes("Backlog") && s.includes("Ready") && s.includes("/ search") && !s.includes("‹ back");
const selected = (s, id) => new RegExp(`▌[ ⚠]*(?:T-)?${id.slice(2)}`).test(s);
const detail = id => s => s.includes(id) && s.includes("Spec") && s.includes("‹ back");
const route = s => s.includes("Pin implementer") && s.includes("Set reviewer") && !implementerPicker(s) && !reviewerPicker(s);
const implementerPicker = s => s.includes("╭ Pin implementer") && s.includes("[Save]");
const reviewerPicker = s => s.includes("╭ Set reviewer") && /inherit/i.test(s) && s.includes("cross-vendor")
  && /(?<![\w-])pm(?![\w-])/.test(s) && /(?<![\w-])none(?![\w-])/.test(s);

try {
  const socket = (await herdr("status", "server")).match(/^socket: (.+)$/m)?.[1];
  assert(socket?.startsWith(privateRoot + "/") && socket.includes("/sessions/corral-test/"),
    `Refuse non-private socket: ${socket}`);
  report.socket = socket;
  await prepare();
  for (const entry of binaries) {
    for (const density of ["rich", "compact"]) {
      await reset(density);
      const capture = new Capture(entry.binary, entry.label, density, socket);
      report.runs.push(capture.result);
      try {
        await capture.start();
        await capture.open("T-001", true);
        if (entry.label === "candidate") await capture.routes();
        else await capture.stage("16-board-return", board, () => capture.raw("\x1b"));
        const current = JSON.parse(await cli(entry.binary, "list", "--all", "--json"));
        assert.equal(current.length, tasks.length);
        assert(current.every(t => !t.runs?.length && !t.plan_runs?.length && !t.launch && !t.queued),
          "Route editing must never reserve or launch an agent");
        capture.result.status = "PASS";
      } catch (error) {
        capture.result.error = String(error.stack ?? error);
        // Keep the other density and candidate independent of a failed baseline.
        console.error(capture.result.error);
      } finally {
        await capture.close();
        if (screenshots) {
          const renderer = join(repo, "scripts/qa/stage-screenshots.mjs");
          try {
            await access(renderer);
            assert(typeof Bun.WebView === "function", "Bun.WebView unavailable");
            await command(process.execPath, [renderer, capture.directory,
              ...(capture.result.status === "PASS" ? [] : ["--failed"])], 120000);
            capture.result.screenshots = { status: "CAPTURED", renderer };
          } catch (error) {
            capture.result.screenshots = { status: "UNAVAILABLE", reason: String(error) };
            console.error(`Screenshots unavailable: ${error}`);
          }
        } else capture.result.screenshots = { status: "DISABLED" };
        await writeFile(join(capture.directory, "result.json"), JSON.stringify(capture.result, null, 2) + "\n");
      }
    }
  }
  report.navigation = [];
  const quantile = (samples, fraction) => {
    const sorted = samples.toSorted((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
  };
  for (const run of report.runs) {
    const samples = run.samples.filter(s => s.name.startsWith("nav-")).map(s => s.ms);
    report.navigation.push({ label: run.label, density: run.density, count: samples.length,
      medianMs: quantile(samples, 0.5), p95Ms: quantile(samples, 0.95) });
  }
  if (baseline) for (const density of ["rich", "compact"]) {
    const before = report.runs.find(r => r.label === "baseline" && r.density === density);
    const after = report.runs.find(r => r.label === "candidate" && r.density === density);
    for (const sample of before.samples) {
      const current = after.samples.find(s => s.name === sample.name);
      if (current) report.comparison.push({ density, name: sample.name,
        baselineMs: sample.ms, candidateMs: current.ms, deltaMs: current.ms - sample.ms });
    }
  }
  report.status = report.runs.every(run => run.status === "PASS") ? "PASS" : "FAIL";
} catch (error) {
  report.error = String(error.stack ?? error);
  console.error(report.error);
} finally {
  report.cleanup = "Only owned workspaces/controllers closed; synthetic fixture retained; supplied server untouched.";
  await writeFile(join(evidence, "commands.json"), JSON.stringify(transcript, null, 2) + "\n");
  await writeFile(join(evidence, "result.json"), JSON.stringify(report, null, 2) + "\n");
}
console.log(JSON.stringify({ status: report.status, evidence, comparison: report.comparison }));
process.exitCode = report.status === "PASS" ? 0 : 1;
