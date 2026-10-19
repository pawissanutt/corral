#!/usr/bin/env bun
// bun scripts/qa/t196-ordinary-reply.mjs /absolute/path/to/candidate-corral
// Owns a fresh scripts/test-herdr.sh environment. Never run against an existing server.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once, EventEmitter } from "node:events";
import { access, copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { createConnection, createServer } from "node:net";
import { createInterface } from "node:readline";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(import.meta.url);
const repo = resolve(dirname(script), "../..");
const installed = "/home/pawis/.omo/agent/runtime/d7669b2db560a617-f176205d0bda/dist";
const isolated = process.argv[2] === "--isolated";
const binary = resolve(process.argv[isolated ? 3 : 2] ?? "");
const runtime = resolve(process.env.T196_RUNTIME ?? installed);
const timeout = 60_000;
const quote = text => `'${text.replaceAll("'", "'\\''")}'`;
const jsonFile = async path => JSON.parse(await readFile(path, "utf8"));
const messageWords = message => typeof message.content === "string" ? message.content
  : message.content.filter(block => block.type === "text").map(block => block.text).join("\n");
const exists = async path => access(path).then(() => true, () => false);
const alive = pid => {
  try { process.kill(pid, 0); return true; } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
};
function cleanEnv() {
  // In particular, do not pass a control socket, run identity, or provider credential
  // from the invoking agent to the private server or any of its panes.
  const allowed = /^(PATH|TERM|COLORTERM|LANG|LC_.*|HOME|SHELL|USER|LOGNAME|TMPDIR|CARGO_HOME|RUSTUP_HOME)$/;
  return Object.fromEntries(Object.entries(process.env).filter(([key]) => allowed.test(key)));
}
async function bounded(promise, label, ms = timeout) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Deadline: ${label}`)), ms);
    })]);
  } finally { clearTimeout(timer); }
}

assert.equal(process.platform, "linux", "The fixture checks Linux pane foreground PIDs");
assert.equal(process.cwd(), repo, `Run from ${repo}`);
assert(process.argv[isolated ? 3 : 2], "Supply a built candidate corral binary");
await access(binary, constants.X_OK);
await access(join(runtime, "bun/cli.js"));
await access(join(runtime, "core/extensions/builtin/ask-user/tool.js"));

if (!isolated) {
  const parent = join(repo, ".omo/evidence/T-196");
  await mkdir(parent, { recursive: true });
  const evidence = await mkdtemp(join(parent, "ordinary-reply-"));
  // Bash retains the helper's EXIT trap; it does not exec the JS worker.
  const child = spawn("bash", ["-c", `
    set -e
    source "$1/scripts/test-herdr.sh"
    corral_test_herdr_start
    export CORRAL_TEST_HERDR_ROOT CORRAL_TEST_HERDR_PID
    "$2" "$3" --isolated "$4" "$5"
  `, "t196-native-qa", repo, process.execPath, script, binary, evidence], {
    cwd: repo, env: { ...cleanEnv(), T196_RUNTIME: runtime }, stdio: "inherit", detached: true,
  });
  let code, bootstrapError;
  const childExit = once(child, "exit");
  const interrupt = () => { try { process.kill(-child.pid, "SIGTERM"); } catch {} };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  try {
    [code] = await bounded(childExit, "isolated QA and helper cleanup", 240_000);
  } catch (error) {
    bootstrapError = error.stack ?? String(error);
    interrupt();
    await bounded(childExit, "private process group exit", 15_000).catch(() => {
      try { process.kill(-child.pid, "SIGKILL"); } catch {}
    });
  } finally {
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", interrupt);
  }
  const reportPath = join(evidence, "result.json");
  const report = await exists(reportPath) ? await jsonFile(reportPath) : { status: "FAIL" };
  if (bootstrapError) report.bootstrapError = bootstrapError;
  const identityPath = join(evidence, "isolation.json");
  const identity = await exists(identityPath) ? await jsonFile(identityPath) : undefined;
  report.serverTeardown = {
    helperExited: code === 0,
    serverGone: identity ? !alive(identity.pid) : false,
    privateHomeRemoved: identity ? !await exists(identity.root) : false,
    nativeProcessesGone: (report.nativePids ?? []).every(pid => !alive(pid)),
  };
  const clean = Object.values(report.serverTeardown).every(Boolean);
  if (code !== 0 || !clean) report.status = "FAIL";
  await writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
  console.log(`${report.status}: ${reportPath}`);
  process.exitCode = report.status === "PASS" ? 0 : 1;
} else {
  await scenario();
}

async function scenario() {
  const evidence = resolve(process.argv[4]);
  const privateRoot = process.env.CORRAL_TEST_HERDR_ROOT;
  assert(privateRoot?.startsWith("/tmp/corral-herdr-"), "Missing disposable helper environment");
  assert.equal(process.env.HOME, join(privateRoot, "home"));
  assert.equal(process.env.CORRAL_HERDR_SESSION, "corral-test");
  assert(process.env.CORRAL_TEST_HERDR_PID, "Missing owned helper PID");
  const root = await mkdtemp(join(privateRoot, "t196-"));
  const agentDir = join(root, "agent");
  const candidate = join(root, "corral");
  const ipcPath = join(root, "fixture.sock");
  const enterPath = join(root, "enter.sock");
  const env = {
    ...cleanEnv(), HOME: process.env.HOME,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME, XDG_DATA_HOME: process.env.XDG_DATA_HOME,
    XDG_STATE_HOME: process.env.XDG_STATE_HOME, XDG_CACHE_HOME: process.env.XDG_CACHE_HOME,
    CORRAL_HERDR_SESSION: "corral-test",
    OMO_CODING_AGENT_DIR: agentDir, SENPI_CODING_AGENT_DIR: agentDir,
    PI_CODING_AGENT_DIR: agentDir,
  };
  const commands = [], signals = [], checks = [], snapshots = {};
  const report = { status: "FAIL", binary, runtime, root, checks, snapshots };
  await writeFile(join(evidence, "isolation.json"), JSON.stringify({
    root: privateRoot, pid: Number(process.env.CORRAL_TEST_HERDR_PID),
  }));
  const bus = new EventEmitter(), waiters = new Set();
  let peer, pane, workspace, endpoint, nativePid, control;
  let pendingIpc = "";
  const ipc = createServer(socket => {
    // Retry replaces the real TUI; only the newest extension receives fixture commands.
    peer = socket;
    socket.on("close", () => { if (peer === socket) peer = undefined; });
    socket.setEncoding("utf8");
    socket.on("error", error => bus.emit("fixture-error", error));
    socket.on("data", chunk => {
      pendingIpc += chunk;
      let end;
      while ((end = pendingIpc.indexOf("\n")) >= 0) {
        const record = JSON.parse(pendingIpc.slice(0, end));
        pendingIpc = pendingIpc.slice(end + 1);
        signals.push(record);
        if (record.type === "error") bus.emit("fixture-error", new Error(record.error));
        else bus.emit("record", record);
      }
    });
  });
  const wait = (predicate, label) => {
    const found = signals.find(predicate);
    if (found) return Promise.resolve(found);
    const promise = new Promise((resolveWait, reject) => {
      const onRecord = record => { if (predicate(record)) { cleanup(); resolveWait(record); } };
      const onError = error => { cleanup(); reject(error); };
      const timer = setTimeout(() => onError(new Error(`Deadline: ${label}`)), timeout);
      const cleanup = () => {
        clearTimeout(timer);
        bus.off("record", onRecord);
        bus.off("fixture-error", onError);
        waiters.delete(onError);
      };
      waiters.add(onError);
      bus.on("record", onRecord);
      bus.on("fixture-error", onError);
    });
    // A preceding CLI failure may abandon a pre-armed barrier; teardown cancels it.
    promise.catch(() => {});
    return promise;
  };
  async function command(program, args, required = true, cwd = root) {
    const child = spawn(program, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    child.stdout.on("data", bytes => { stdout += bytes; });
    child.stderr.on("data", bytes => { stderr += bytes; });
    let code;
    try { [code] = await bounded(once(child, "close"), `${program} ${args.join(" ")}`); }
    catch (error) {
      child.kill("SIGKILL");
      await bounded(once(child, "close"), "timed-out CLI termination", 5_000).catch(() => {});
      throw error;
    }
    commands.push({ program, args, code, stdout, stderr });
    if (required) assert.equal(code, 0, `${program} ${args.join(" ")}\n${stderr}`);
    return { code, stdout, stderr };
  }
  const cli = (args, required = true) => command(candidate, ["--root", root, ...args], required);
  const json = async args => JSON.parse((await cli(args)).stdout);
  const herdr = (...args) => command("herdr", ["--session", "corral-test", ...args]);
  const set = (key, value) => cli(["config", "set", key, value, "--workspace"]);
  const check = (label, value) => { assert(value, label); checks.push(label); };
  const send = async (op, token, data = {}) => {
    const ack = wait(r => r.type === "ack" && r.op === op && r.token === token, `${op} acknowledgement`);
    peer.write(JSON.stringify({ ...data, op, token }) + "\n");
    return ack;
  };
  const armAck = join(root, "drop-enter-ack");
  const droppedAck = join(root, "dropped-enter-ack");
  let enterCase;
  const enterSockets = new Set();
  const enterServer = createServer(socket => {
    enterSockets.add(socket);
    socket.on("close", () => enterSockets.delete(socket));
    socket.on("error", error => bus.emit("fixture-error", error));
    const lines = createInterface({ input: socket });
    const requests = lines[Symbol.asyncIterator]();
    const receive = async () => {
      const next = await bounded(requests.next(), "Enter fault handshake");
      assert(!next.done, "Enter wrapper disconnected without a receipt");
      return JSON.parse(next.value);
    };
    (async () => {
      const scenario = enterCase;
      assert(scenario, "Enter fault must be explicitly armed");
      assert.equal((await receive()).op, "prepare");
      const token = "lost-enter-" + (scenario.inputs.length + 1);
      const applied = wait(r => r.type === "terminal-input-applied" && r.token === token,
        "real engine consumed faulted terminal input");
      const accepted = wait(r => signals.indexOf(r) >= scenario.start &&
        r.type === "user" && r.pid === nativePid && r.text === scenario.text,
      "exact lost-ACK payload accepted by real engine");
      const editDraft = scenario.inputs.length === 0;
      const bytes = Buffer.from(editDraft ? "\x1b[200~\r\x1b[201~" : "\r").toString("hex");
      // This round trip arms the input observer before the helper writes a key.
      await send("arm-terminal-input", token, { bytes });
      socket.write(JSON.stringify({ ready: true, token, editDraft }) + "\n");
      const written = await receive();
      assert.equal(written.op, "written");
      assert.equal(written.token, token);
      commands.push({ program: written.program, args: written.args, code: written.code,
        stdout: written.stdout, stderr: written.stderr, source: "enter-fault-helper", token });
      if (written.code !== 0) {
        socket.end(JSON.stringify({ drop: false }) + "\n");
        return;
      }
      const observation = await applied;
      scenario.inputs.push(observation);
      assert.equal(observation.bytes, bytes);
      if (editDraft) {
        assert.equal(observation.before, scenario.text);
        assert.equal(observation.after, scenario.text + "\n");
      }
      if (observation.after !== "") {
        // Let production observe and handle retained input; never claim a lost
        // accepted submission merely because the key-writing subprocess exited 0.
        signals.push({ type: "enter-retained", token, observation });
        socket.end(JSON.stringify({ drop: false }) + "\n");
        return;
      }
      const receipt = await accepted;
      const dropped = { type: "enter-ack-dropped", token, acceptedSequence: receipt.sequence,
        text: receipt.text, observation };
      await writeFile(droppedAck, JSON.stringify(dropped) + "\n", { flag: "wx" });
      await rm(armAck);
      signals.push(dropped);
      socket.end(JSON.stringify({ drop: true, acceptedSequence: receipt.sequence }) + "\n");
    })().catch(error => {
      socket.end(JSON.stringify({ error: error.stack ?? String(error) }) + "\n");
      bus.emit("fixture-error", error);
    }).finally(() => lines.close());
  });
  try {
    await new Promise((resolveListen, reject) => {
      ipc.once("error", reject);
      ipc.listen(ipcPath, resolveListen);
    });
    await new Promise((resolveListen, reject) => {
      enterServer.once("error", reject);
      enterServer.listen(enterPath, resolveListen);
    });
    const realHerdr = Bun.which("herdr");
    assert(realHerdr, "herdr must be installed");
    const wrapper = join(root, "herdr-wrapper");
    const wrapperScreens = join(root, "before-enter");
    await mkdir(wrapperScreens);
    await writeFile(wrapper, [
      "#!/usr/bin/env bash", "set -u",
      'if [[ "$#" -eq 6 && "$1" == --session && "$2" == corral-test && "$3" == pane && "$4" == send-keys && "$6" == enter ]]; then',
      "  screen=$(mktemp " + quote(join(wrapperScreens, "screen-XXXXXX")) + ")",
      "  " + quote(realHerdr) + ' --session corral-test pane read "$5" --source recent-unwrapped --lines 200 > "$screen" || exit "$?"',
      "  if [[ -f " + quote(armAck) + " ]]; then",
      "    exec " + quote(process.execPath) + " " + quote(join(dirname(script), "t196-enter-ack.mjs")) +
        " " + quote(enterPath) + " " + quote(realHerdr) + ' "$@"',
      "  fi", "fi",
      "exec " + quote(realHerdr) + ' "$@"', "",
    ].join("\n"), { mode: 0o700 });
    env.HERDR_BIN_PATH = wrapper;
    await copyFile(binary, candidate);
    await mkdir(agentDir);
    await mkdir(join(root, "inbox"));
    // Clone existing history, not dirty files; no commit is created in either repository.
    await command("git", ["clone", "--shared", "--no-checkout", "--quiet", repo, join(root, "app")]);
    await command("git", ["checkout", "--quiet", "-B", "main", "HEAD"], true, join(root, "app"));
    await cli(["init"]);
    await set("review.autopilot", "false");
    await set("sync.omo_agent_dir", JSON.stringify(agentDir));
    await set("resources.max_load_per_core", "1000000");
    await set("resources.min_mem_available", '"1B"');
    await set("resources.min_disk_free", '"1B"');
    const launch = [
      "env", `HOME=${quote(env.HOME)}`,
      ...["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"].map(key => `${key}=${quote(env[key])}`),
      `OMO_CODING_AGENT_DIR=${quote(agentDir)}`,
      `SENPI_CODING_AGENT_DIR=${quote(agentDir)}`, `PI_CODING_AGENT_DIR=${quote(agentDir)}`,
      `T196_RUNTIME=${quote(runtime)}`, `T196_ORDINARY_IPC=${quote(ipcPath)}`,
      quote(process.execPath), quote(join(runtime, "bun/cli.js")),
      "--offline --no-extensions --no-skills --no-prompt-templates",
      "-e", quote(join(dirname(script), "t196-ordinary-reply-extension.mjs")),
      "--provider t196-fixture --model local --thinking off",
    ].join(" ");
    await set("harnesses.omo.command", JSON.stringify(launch));
    await set("harnesses.omo.flags", '"generic"');
    await set("profiles.omo.harness", '"omo"');
    const id = (await cli(["new", "Ordinary reply fixture", "-p", "app", "--ready"])).stdout.trim();
    const ready = wait(r => r.type === "ready", "real TUI extension registration");
    await cli(["dispatch", id, "--profile", "omo", "--force"]);
    const task = await json(["show", id, "--json"]);
    const run = task.runs.at(-1);
    pane = run.herdr.pane_id;
    workspace = run.herdr.workspace_id;
    const registration = await ready;
    nativePid = registration.pid;
    check("Extension runs in TUI mode", registration.mode === "tui");
    check("Real session cwd equals run worktree", registration.cwd === run.worktree);
    const entries = await readdir(join(agentDir, "rpc-host-daemon"), { withFileTypes: true });
    for (const entry of entries.filter(e => e.isDirectory())) {
      const dir = join(agentDir, "rpc-host-daemon", entry.name);
      const record = await jsonFile(join(dir, "endpoint.json"));
      const pointer = await jsonFile(join(dir, "host.pid"));
      const generation = await jsonFile(join(dir, "generations", pointer.instance_id, "host.pid"));
      if (generation.pid === nativePid) {
        assert.equal(record.endpoint_kind, "tui");
        assert.equal(record.socket, registration.socket);
        endpoint = { dir, record, generation };
      }
    }
    assert(endpoint, "Real endpoint generation must identify the TUI process");
    snapshots.endpoint = endpoint;
    snapshots.processInfo = JSON.parse((await herdr("pane", "process-info", "--pane", pane)).stdout);
    check("Endpoint PID is a pane foreground process",
      snapshots.processInfo.result.process_info.foreground_processes.some(p => p.pid === nativePid));
    check("CLI dispatch stored private herdr socket ownership",
      run.herdr.socket === join(env.XDG_CONFIG_HOME, "herdr/sessions/corral-test/herdr.sock"));
    control = await connectControl(registration.socket);
    const sessions = await control.request("list_sessions");
    check("Authenticated native endpoint lists run cwd",
      sessions.data.sessions.some(s => s.cwd === run.worktree));
    snapshots.sessions = sessions;
    const events = async () => (await readFile(join(root, ".corral/events.jsonl"), "utf8"))
      .trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    const lastDelivery = async text => (await events()).filter(event =>
      event.kind === "replied" && event.detail.text === text).at(-1)?.detail;
    const pending = async text => {
      const state = await json(["show", id, "--json"]);
      snapshots["state-" + Object.keys(snapshots).length] = state;
      check("Canonical pending reply matches expected text", text == null
        ? state.runs.at(-1).pending_reply == null : state.runs.at(-1).pending_reply === text);
      return state;
    };
    const armTurn = (text, matches = actual => actual === text) => {
      const start = signals.length;
      // Both subscriptions are installed BEFORE any terminal write.
      const received = wait(r => signals.indexOf(r) >= start && r.type === "user",
        "real terminal user message");
      const ended = wait(r => signals.indexOf(r) >= start && r.type === "idle",
        "real engine agent_end");
      return async () => {
        const record = await received;
        assert(matches(record.text), "Real user text differs from exact expected payload: " +
          JSON.stringify({ expected: text, actual: record.text }));
        const idle = await ended;
        assert.equal(idle.userSequence, record.sequence, "agent_end belongs to the submitted user message");
        return record.text;
      };
    };
    const single = async text => {
      const messages = (await control.request("get_messages")).data.messages;
      check("Exactly one expanded model-facing user message", messages.filter(message =>
        message.role === "user" && messageWords(message) === text).length === 1);
      check("Exactly one extension user event", signals.filter(record =>
        record.type === "user" && record.text === text).length === 1);
      snapshots["messages-" + Object.keys(snapshots).length] = messages;
    };
    const inspect = async token => {
      const draft = wait(r => r.type === "draft" && r.token === token, "real editor contents");
      await send("inspect-draft", token);
      return draft;
    };
    const noPendingResend = async text => {
      const refused = await cli(["reply", id, "--resend"], false);
      check("Second resend refuses an already submitted reply", refused.code !== 0);
      await control.request("get_state");
      await single(text);
    };
    const long = label => Array.from({ length: 30 }, (_, line) =>
      label + " line " + line + ": exact 'quotes' \"double\" \\ and full multiline payload.").join("\n") + "\n\nlast line";

    // Given an empty, idle REAL editor, exercise both public entry points.
    for (const mode of ["reply", "reply-chars", "comment"]) {
      const text = mode === "reply-chars" ? Array.from({ length: 4 }, (_, line) =>
        "T196-reply-chars " + line + ": " + "exact \u{1f680} UTF-16 payload ".repeat(24) + "end").join("\n")
        : long("T196-" + mode);
      const complete = armTurn(text, actual => mode === "comment"
        ? actual.endsWith("\n\n" + text) : actual === text);
      const screenStart = new Set(await readdir(wrapperScreens));
      await cli(mode === "comment" ? ["comment", id, text, "--send", "--author", "pm"]
        : ["reply", id, text]);
      const deliveredText = await complete();
      await pending(undefined);
      await single(deliveredText);
      const receipt = await lastDelivery(deliveredText);
      check(mode + ": full canonical payload reached the model exactly", receipt?.text === deliveredText);
      if (mode === "comment") {
        const saved = await json(["show", id, "--json"]);
        check("Durable comment preserves exact author and long text",
          saved.comments.at(-1).author === "pm" && saved.comments.at(-1).text === text);
        check("Comment envelope includes long reply exactly once", deliveredText.split(text).length === 2);
      }
      check(mode + ": truthful successful terminal metadata", receipt?.submitted === true &&
        receipt.via === "send_text" && receipt.attempts >= 1);
      const screens = (await readdir(wrapperScreens)).filter(file => !screenStart.has(file));
      const previews = await Promise.all(screens.map(file => readFile(join(wrapperScreens, file), "utf8")));
      check(mode + ": real terminal displayed a collapsed long paste before Enter", previews.length > 0 &&
        previews.some(screen => screen.includes("[paste #")));
      if (mode === "reply-chars") {
        check("Character-marker scenario has at most ten lines and astral Unicode",
          text.split("\n").length <= 10 && text.length > [...text].length);
        check("Real character-count marker uses exact UTF-16 payload length",
          previews.some(screen => new RegExp("\\[paste #\\d+ " + text.length + " chars\\]").test(screen)));
      }
      await noPendingResend(deliveredText);
    }

    // Force one edited-but-unsent input, then lose an ACK only after an exact
    // pre-armed model-facing user event proves the subsequent input submitted.
    const lost = long("T196-lost-enter-ack");
    enterCase = { text: lost, start: signals.length, inputs: [] };
    snapshots.enterFault = enterCase;
    await writeFile(armAck, "armed\n", { flag: "wx" });
    const lostComplete = armTurn(lost);
    const lostResult = await cli(["reply", id, lost], false);
    await lostComplete();
    const dropped = await jsonFile(droppedAck);
    check("Edited terminal input retained the exact draft before production retried",
      enterCase.inputs[0].before === lost && enterCase.inputs[0].after === lost + "\n" &&
      signals.filter(r => r.type === "enter-retained").length === 1);
    check("ACK loss follows an exact real-engine acceptance event",
      dropped.text === lost && signals.some(r => r.type === "user" &&
        r.sequence === dropped.acceptedSequence && r.text === lost &&
        signals.indexOf(r) < signals.findIndex(event => event.type === "enter-ack-dropped")));
    check("Exactly one accepted submission ACK was lost", !await exists(armAck) &&
      signals.filter(r => r.type === "enter-ack-dropped").length === 1);
    // CLI may report pending as nonzero or successful-with-pending; state is authoritative.
    snapshots.lostAckCommand = lostResult;
    await pending(lost);
    const lostReceipt = await lastDelivery(lost);
    check("Lost Enter ACK stores truthful failed submission metadata", lostReceipt?.submitted === false &&
      lostReceipt.via === "send_text" && lostReceipt.attempts === enterCase.inputs.length &&
      lostReceipt.attempts === 2 && !!lostReceipt.error);
    const journal = (await events()).filter(event => event.kind === "reply_terminal_state" && event.detail.text === lost);
    check("Lost ACK retains journaled Enter intent", journal.at(-1)?.detail.state.stage === "entering");
    const screensBeforeResend = (await readdir(wrapperScreens)).length;
    await cli(["reply", id, "--resend"]);
    await pending(undefined);
    const reconciled = await lastDelivery(lost);
    check("Resend reconciles without another Enter", reconciled?.submitted === true &&
      reconciled.attempts === 0 && (await readdir(wrapperScreens)).length === screensBeforeResend);
    await single(lost);
    await noPendingResend(lost);

    // Reuse T-170's exact duplicate opaque-draft safety precondition. An unknown
    // collapsed composer is NOT owned merely because its size matches our text.
    for (const author of ["pm", "owner"]) {
      const token = "external-" + author;
      const block = Array.from({ length: 20 }, (_, line) =>
        "Private pending fixture line " + line + ".").join("\n").padEnd(1334, "x") + "\n";
      assert.equal(block.length, 1335);
      // Keep 1335 characters and distinct authors, without edge whitespace.
      const text = (author + block.slice(author.length)).slice(0, -1) + "x";
      const draft = await send("duplicate-draft", token, { text });
      check(author + ": two opaque real draft blocks", (draft.rendered.match(/\[paste #/g) ?? []).length === 2);
      const refused = await cli(["reply", id, text], false);
      check(author + ": pre-existing duplicate draft fails closed", refused.code !== 0);
      await pending(text);
      const receipt = await lastDelivery(text);
      check(author + ": unknown draft has truthful zero-attempt metadata", receipt?.submitted === false && receipt.attempts === 0);
      check(author + ": unknown draft resend refused", (await cli(["reply", id, "--resend"], false)).code !== 0);
      check(author + ": resend retains exactly both blocks", (await inspect(token + "-retained")).text === text + text);
      check(author + ": replacement reply refused", (await cli(["reply", id, "unrelated replacement"], false)).code !== 0);
      await send("clear-fixture-draft", token);
      check(author + ": empty editor does not replace pending", (await cli(["reply", id, "still unrelated"], false)).code !== 0);
      await pending(text);
      check(author + ": clearing did not deliver pending", !(await control.request("get_messages")).data.messages
        .some(message => message.role === "user" && messageWords(message).includes(text)));

      // External manual delivery STILL uses real herdr paste and terminal Enter.
      const complete = armTurn(text);
      await herdr("pane", "send-text", pane, text);
      const staged = await inspect(token + "-manual");
      check(author + ": manual paste is exact and collapsed", staged.text === text && staged.rendered.includes("[paste #"));
      const inputApplied = wait(r => r.type === "terminal-input-applied" && r.token === token,
        "real manual terminal input applied");
      const enter = "\x1b[13u";
      const enterBytes = Buffer.from(enter).toString("hex");
      await send("arm-terminal-input", token, { bytes: enterBytes });
      // The real decoder maps CSI-u 13 to unmodified Enter. Unlike bare CR,
      // it cannot be absorbed into the preceding unbracketed paste burst.
      await command(realHerdr, ["--session", "corral-test", "pane", "send-text", pane, enter]);
      const observedInput = await inputApplied;
      snapshots[token + "-enter"] = observedInput;
      check(author + ": actual terminal Enter cleared the expanded editor",
        observedInput.bytes === enterBytes && observedInput.before === text && observedInput.after === "");
      await complete();
      await single(text);
      await pending(text);
      const reason = "Observed exact external terminal user message for " + author;
      const before = (await control.request("get_messages")).data.messages;
      await cli(["reply", id, "--mark-delivered", "--author", author, "--reason", reason]);
      await pending(undefined);
      const audit = (await events()).filter(event => event.kind === "reply_marked_delivered" && event.detail.text === text);
      check(author + ": exact audited manual acknowledgement", audit.length === 1 && audit[0].detail.by === author &&
        audit[0].detail.reason === reason && audit[0].detail.submitted === true && audit[0].detail.attempts === 0 &&
        audit[0].detail.run === run.id && audit[0].detail.pane === pane);
      check(author + ": acknowledgement sends no user message", JSON.stringify(before) ===
        JSON.stringify((await control.request("get_messages")).data.messages));
      await noPendingResend(text);
      if (author === "pm") {
        // CLI fixture setup only; no done/gate/commit and no direct store mutations.
        await cli(["move", id, "review", "--force"]);
        const changes = "T196 changes after manual acknowledgement";
        const changed = armTurn(changes);
        await cli(["changes", id, changes]);
        await changed();
        const state = await pending(undefined);
        check("Changes proceeds after acknowledged pending reply", state.status === "running");
        await single(changes);
      }
    }

    // A valid checkpoint is an independent retry requirement, not a delivery bypass.
    const readback = await json(["run", "checkpoint", id, "--json"]);
    const checkpoint = {
      task: id, run: run.id, spec_revision: readback.spec_revision,
      expected_revision: readback.checkpoints.at(-1)?.checkpoint.revision ?? 0,
      progress_revision: readback.progress?.revision ?? 0,
      head: readback.head, base: readback.base, tree: readback.tree,
      criterion_ids: [], evidence_refs: [], next_action: "Continue disposable T-196 QA",
      decisions: [], waits: [], resources: [],
    };
    const checkpointFile = join(root, "checkpoint.json");
    await writeFile(checkpointFile, JSON.stringify(checkpoint));
    await cli(["run", "checkpoint", id, "--file", checkpointFile, "--json"]);
    const signalStart = signals.length;
    const nextReady = wait(r => r.type === "ready" && r.pid !== nativePid &&
      signals.indexOf(r) >= signalStart, "retried real TUI ready");
    await cli(["retry", id, "--run", run.id, "--profile", "omo"]);
    const replacement = await nextReady;
    const retried = await json(["show", id, "--json"]);
    check("Guarded retry proceeds with a new live run", retried.runs.at(-1).id !== run.id &&
      retried.runs.at(-1).pending_reply == null && retried.status === "running");
    check("Retry terminated the old native TUI", !alive(nativePid));
    control.close();
    control = await connectControl(replacement.socket);
    nativePid = replacement.pid;
    pane = retried.runs.at(-1).herdr.pane_id;
    workspace = retried.runs.at(-1).herdr.workspace_id;
    check("Retried generic TUI opens an idle empty editor", (await inspect("retried-editor")).text === "");
    // The replacement endpoint owns its own generation directory.
    endpoint = undefined;
    for (const entry of await readdir(join(agentDir, "rpc-host-daemon"), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const dir = join(agentDir, "rpc-host-daemon", entry.name);
      const pointer = await jsonFile(join(dir, "host.pid"));
      const generation = await jsonFile(join(dir, "generations", pointer.instance_id, "host.pid"));
      if (generation.pid === nativePid) endpoint = { dir, generation };
    }
    check("Retry owns a real replacement endpoint", !!endpoint);
    snapshots.retried = retried;
    snapshots.events = await events();
    snapshots.wrapperScreens = await Promise.all((await readdir(wrapperScreens)).map(async file => ({
      file, screen: await readFile(join(wrapperScreens, file), "utf8"),
    })));
    snapshots.transcript = await readFile(registration.sessionFile, "utf8");
    snapshots.retriedTranscript = await readFile(replacement.sessionFile, "utf8");
    report.status = "PASS";
  } catch (error) {
    report.error = error.stack ?? String(error);
    if (peer && nativePid && alive(nativePid)) {
      try {
        const draft = wait(r => r.type === "draft" && r.token === "failure-draft", "failure editor snapshot");
        await send("inspect-draft", "failure-draft");
        snapshots.failureDraft = await draft;
      } catch (captureError) { snapshots.failureDraftError = String(captureError); }
    }
    if (pane) {
      const capture = await command("herdr", ["--session", "corral-test", "pane", "read", pane,
        "--source", "recent-unwrapped", "--lines", "100"], false);
      snapshots.failurePane = capture;
    }
  } finally {
    // Preserve real terminal previews and transcripts on failure as well as pass.
    const screenDir = join(root, "before-enter");
    if (await exists(screenDir)) {
      snapshots.wrapperScreens = await Promise.all((await readdir(screenDir)).map(async file => ({
        file, screen: await readFile(join(screenDir, file), "utf8"),
      })));
    }
    report.nativePids = signals.filter(record => record.type === "ready").map(record => record.pid);
    snapshots.transcripts = await Promise.all(signals.filter(record => record.type === "ready").map(async record => ({
      pid: record.pid, sessionFile: record.sessionFile,
      text: await exists(record.sessionFile) ? await readFile(record.sessionFile, "utf8") : null,
    })));
    control?.close();
    let nativeExited = nativePid ? !alive(nativePid) : true;
    try {
      if (peer && nativePid && alive(nativePid)) {
        // pidfd readiness is process exit, not a delay or a /proc polling heuristic.
        const watcher = spawn("python3", ["-c", `
import os, select, sys
fd = os.pidfd_open(int(sys.argv[1]))
print("ARMED", flush=True)
if not select.select([fd], [], [], 60)[0]:
    raise TimeoutError("native TUI did not exit")
os.close(fd)
`, String(nativePid)], { env, stdio: ["ignore", "pipe", "pipe"] });
        const exited = once(watcher, "exit");
        try {
          await bounded(new Promise((resolveArmed, reject) => {
            watcher.once("error", reject);
            watcher.stdout.once("data", bytes => {
              if (bytes.toString().trim() === "ARMED") resolveArmed();
              else reject(new Error("Unexpected pidfd watcher output"));
            });
            watcher.once("exit", () => reject(new Error("pidfd watcher exited before arming")));
          }), "native process exit subscription");
          const shutdown = wait(r => r.type === "shutdown", "native extension shutdown");
          await send("shutdown", "teardown");
          const teardown = await shutdown;
          check("Fixture reports real TUI shutdown", teardown.pid === nativePid);
          const [exitCode] = await bounded(exited, "native TUI exit");
          assert.equal(exitCode, 0, "Native TUI exit must be observed through pidfd");
          nativeExited = true;
        } finally { watcher.kill("SIGKILL"); }
      }
      report.fixtureTeardown = {
        nativeProcessExited: nativeExited,
        endpointRemoved: endpoint ? !await exists(endpoint.dir) : true,
      };
      if (!Object.values(report.fixtureTeardown).every(Boolean)) report.status = "FAIL";
    } catch (error) {
      report.status = "FAIL";
      report.teardownError = error.stack ?? String(error);
    }
    // Closing a pane is still required if graceful native shutdown failed.
    if (workspace) {
      try { await herdr("workspace", "close", workspace); }
      catch (error) { report.status = "FAIL"; report.workspaceCloseError = String(error); }
    }
    for (const cancel of [...waiters]) cancel(new Error("Fixture teardown"));
    for (const socket of enterSockets) socket.destroy();
    await new Promise(resolveClose => enterServer.close(resolveClose));
    peer?.destroy();
    await new Promise(resolveClose => ipc.close(resolveClose));
    await rm(root, { recursive: true, force: true });
    report.fixtureRemoved = !await exists(root);
    await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
    await writeFile(join(evidence, "signals.json"), JSON.stringify(signals, null, 2) + "\n");
    await writeFile(join(evidence, "result.json"), JSON.stringify(report, null, 2) + "\n");
    process.exitCode = report.status === "PASS" ? 0 : 1;
  }
}

async function connectControl(path) {
  const secret = await readFile(`${path}.secret`);
  assert.equal(secret.length, 32, "Native socket secret must be 32 binary bytes");
  const socket = createConnection(path);
  const pending = new Map();
  let buffer = "", sequence = 0;
  socket.setEncoding("utf8");
  socket.on("error", error => { for (const entry of pending.values()) entry.reject(error); });
  socket.on("data", chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const frame = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      const entry = pending.get(frame.id);
      if (entry) { pending.delete(frame.id); entry.resolve(frame); }
    }
  });
  await bounded(once(socket, "connect"), "native control socket connect");
  socket.write(secret);
  return {
    close: () => socket.destroy(),
    async request(type) {
      const id = `qa-${++sequence}`;
      const promise = new Promise((resolveRequest, reject) => pending.set(id, { resolve: resolveRequest, reject }));
      socket.write(JSON.stringify({ id, type }) + "\n");
      try {
        const response = await bounded(promise, `native ${type}`);
        assert.equal(response.success, true, JSON.stringify(response));
        return response;
      } finally { pending.delete(id); }
    },
  };
}
