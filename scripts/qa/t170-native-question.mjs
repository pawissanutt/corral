#!/usr/bin/env bun
// bun scripts/qa/t170-native-question.mjs /absolute/path/to/candidate-corral
// Owns a fresh scripts/test-herdr.sh environment. Never run against an existing server.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once, EventEmitter } from "node:events";
import { access, copyFile, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { createConnection, createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(import.meta.url);
const repo = resolve(dirname(script), "../..");
const installed = "/home/pawis/.omo/agent/runtime/d7669b2db560a617-f176205d0bda/dist";
const isolated = process.argv[2] === "--isolated";
const binary = resolve(process.argv[isolated ? 3 : 2] ?? "");
const runtime = resolve(process.env.T170_RUNTIME ?? installed);
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
  const parent = join(repo, ".omo/evidence/T-170");
  await mkdir(parent, { recursive: true });
  const evidence = await mkdtemp(join(parent, "native-question-"));
  // Bash retains the helper's EXIT trap; it does not exec the JS worker.
  const child = spawn("bash", ["-c", `
    set -e
    source "$1/scripts/test-herdr.sh"
    corral_test_herdr_start
    export CORRAL_TEST_HERDR_ROOT CORRAL_TEST_HERDR_PID
    "$2" "$3" --isolated "$4" "$5"
  `, "t170-native-qa", repo, process.execPath, script, binary, evidence], {
    cwd: repo, env: { ...cleanEnv(), T170_RUNTIME: runtime }, stdio: "inherit", detached: true,
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
  const root = await mkdtemp(join(privateRoot, "t170-"));
  const agentDir = join(root, "agent");
  const candidate = join(root, "corral");
  const ipcPath = join(root, "fixture.sock");
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
    assert(!peer, "Only the fixture extension may attach");
    peer = socket;
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
  try {
    await new Promise((resolveListen, reject) => {
      ipc.once("error", reject);
      ipc.listen(ipcPath, resolveListen);
    });
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
      `T170_RUNTIME=${quote(runtime)}`, `T170_NATIVE_IPC=${quote(ipcPath)}`,
      quote(process.execPath), quote(join(runtime, "bun/cli.js")),
      "--offline --no-extensions --no-skills --no-prompt-templates",
      "-e", quote(join(dirname(script), "t170-native-question-extension.mjs")),
      "--provider t170-fixture --model local --thinking off",
    ].join(" ");
    await set("harnesses.omo.command", JSON.stringify(launch));
    await set("harnesses.omo.flags", '"generic"');
    await set("profiles.omo.harness", '"omo"');
    const id = (await cli(["new", "Native question fixture", "-p", "app", "--ready"])).stdout.trim();
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
    for (const [index, loseAck] of [[1, false], [2, true]]) {
      const token = `t170-native-${index}-full-request-0123456789abcdef`;
      const answer = `T-170 exact answer ${index}\nsecond line: 'quotes' \"double\" \\\n\nlast line`;
      const signalStart = signals.length;
      const asked = wait(r => r.type === "question" && r.request.requestId === token, token);
      await send("ask", token);
      await asked;
      await wait(r => r.type === "idle" && r.token === token, "native question turn completed");
      // A real bracketed paste creates an unrelated collapsed composer draft.
      // Native question delivery must neither type into nor submit that editor.
      const draft = Array.from({ length: 20 }, (_, line) =>
        `Unrelated draft ${index}, line ${line}: do not submit this input.`).join("\n");
      const draftRendered = await send("draft", token, { text: draft });
      snapshots[`${token}-rendered`] = draftRendered.rendered;
      // A socket round trip observes the pending UI after the tool's lifecycle event.
      snapshots[`${token}-state`] = await control.request("get_state");
      check(`${token}: unrelated draft holds the ordinary editor`,
        snapshots[`${token}-state`].data.editor_has_draft === true);
      snapshots[`${token}-pane`] = (await herdr("pane", "read", pane,
        "--source", "recent-unwrapped", "--lines", "200")).stdout;
      check(`${token}: ordinary editor displays a collapsed paste`,
        draftRendered.rendered.includes("[paste #"));
      const transcript = (await readFile(registration.sessionFile, "utf8"))
        .trim().split("\n").map(line => JSON.parse(line));
      const callIndex = transcript.findIndex(e => e.type === "message" &&
        e.message.role === "assistant" && e.message.content?.some(c =>
          c.type === "toolCall" && c.name === "request_user_input" && c.id === token &&
          c.arguments.questions[0].id === "native_answer"));
      const lifecycleIndex = transcript.findIndex(e => e.type === "custom" &&
        e.customType === "ask-user:question" && e.data.requestId === token);
      check(`${token}: genuine call persisted before native lifecycle`, callIndex >= 0 && lifecycleIndex > callIndex);
      check(`${token}: async tool result remains pending`, transcript.some(e =>
        e.type === "message" && e.message.role === "toolResult" && e.message.toolCallId === token &&
        e.message.details.status === "pending"));
      const settled = wait(r => r.type === "settled" && r.request.requestId === token, "native settlement");
      const received = wait(r => r.type === "answer-frame" && r.token === token, "native answer transcript frame");
      if (loseAck) {
        await send("arm-drop-answer-ack", token);
        const dropped = wait(r => r.type === "ack-dropped" && r.token === token, "native answer acknowledgement dropped");
        const failed = await cli(["reply", id, answer], false);
        await dropped;
        check("Lost native acknowledgement reports failed delivery", failed.code !== 0);
        const pending = await json(["show", id, "--json"]);
        snapshots.pending = pending;
        check("Exact multiline reply survives failed delivery", pending.runs.at(-1).pending_reply === answer);
      } else {
        await cli(["reply", id, answer]);
      }
      const receipt = await settled;
      await received;
      await wait(r => r.type === "idle" && r.token === token &&
        signals.indexOf(r) > signals.findIndex(s => s.type === "answer-frame" && s.token === token),
      "native answer follow-up completed");
      check(`${token}: exact multiline native receipt`, receipt.response.comment === answer);
      check(`${token}: native control endpoint resolved answer`, receipt.response.resolvedBy === "control_endpoint");
      snapshots[`${token}-receipt`] = receipt;
      if (loseAck) {
        // All durable answer barriers precede reconciliation. Resend must not
        // transmit another response to an already settled native request.
        await cli(["reply", id, "--resend"]);
      }
      snapshots[token] = await json(["show", id, "--json"]);
      check(`${token}: pending reply cleared`, snapshots[token].runs.at(-1).pending_reply == null);
      const messages = (await control.request("get_messages")).data.messages;
      const frames = messages.filter(m => m.role === "user" &&
        JSON.stringify(m.content).includes(`[Answer to question ${token}]`));
      check(`${token}: exactly one model-facing answer frame`, frames.length === 1);
      check(`${token}: model-facing words preserve exact multiline answer`,
        frames[0].content.some(block => block.type === "text" && block.text === answer));
      const draftRead = wait(r => r.type === "draft" && r.token === token, "unrelated editor draft");
      await send("inspect-draft", token);
      check(`${token}: native reply leaves unrelated draft unchanged`, (await draftRead).text === draft);
      check(`${token}: unrelated draft was never submitted`,
        !messages.some(message => message.role === "user" && messageWords(message).includes(draft)));
      snapshots[`${token}-messages`] = messages;
      const resend = await cli(["reply", id, "--resend"], false);
      check(`${token}: no pending reply to resend`, resend.code !== 0);
      // A native command round trip is the barrier after the rejected resend.
      await control.request("get_state");
      check(`${token}: settled once`, signals.filter(r =>
        r.type === "closed" && r.requestId === token).length === 1);
      check(`${token}: no fixture errors during native delivery`,
        !signals.slice(signalStart).some(r => r.type === "error"));
    }
    // Reproduce the PM's later ordinary-composer state without touching T-137:
    // no native question, two identical 1335-character blocks, then an empty
    // composer while the canonical undelivered reply still exists.
    const ordinary = "ordinary-pending-1335";
    const block = Array.from({ length: 20 }, (_, line) =>
      `Private pending fixture line ${line}.`).join("\n").padEnd(1334, "x") + "\n";
    assert.equal(block.length, 1335);
    const duplicateRendered = await send("duplicate-draft", ordinary, { text: block });
    snapshots.ordinaryDuplicateRendered = duplicateRendered.rendered;
    await control.request("get_state");
    const duplicatePane = (await herdr("pane", "read", pane,
      "--source", "recent-unwrapped", "--lines", "200")).stdout;
    snapshots.ordinaryDuplicatePane = duplicatePane;
    check("Ordinary composer has two collapsed paste blocks",
      (duplicateRendered.rendered.match(/\[paste #/g) ?? []).length === 2);
    const duplicateRead = wait(r => r.type === "draft" && r.token === ordinary, "duplicate draft contents");
    await send("inspect-draft", ordinary);
    check("Both ordinary blocks preserve the full 1335-character text",
      (await duplicateRead).text === block + block);
    const failedOrdinary = await cli(["reply", id, block], false);
    check("Collapsed ordinary composer fails closed", failedOrdinary.code !== 0);
    const refusedOrdinary = await cli(["reply", id, "--resend"], false);
    check("Ordinary resend does not add a third paste", refusedOrdinary.code !== 0);
    const afterResend = `${ordinary}-after-resend`;
    const retainedRead = wait(r => r.type === "draft" && r.token === afterResend, "ordinary draft after resend");
    await send("inspect-draft", afterResend);
    check("Ordinary resend leaves both original blocks unchanged", (await retainedRead).text === block + block);
    snapshots.ordinaryPending = await json(["show", id, "--json"]);
    check("Ordinary canonical pending reply is exact",
      snapshots.ordinaryPending.runs.at(-1).pending_reply === block);
    const shortRefused = await cli(["reply", id, "Unrelated short input"], false);
    check("New short input cannot replace the ordinary pending reply", shortRefused.code !== 0);
    // Clear only the disposable editor via its own fixture extension, never the
    // canonical pending reply and never an owner pane.
    await send("clear-fixture-draft", ordinary);
    const cleared = `${ordinary}-cleared`;
    const clearedRead = wait(r => r.type === "draft" && r.token === cleared, "empty fixture editor");
    await send("inspect-draft", cleared);
    check("Private composer is empty after fixture-owned clearing", (await clearedRead).text === "");
    const emptyRefused = await cli(["reply", id, "Still unrelated input"], false);
    check("Empty composer still cannot replace canonical pending text", emptyRefused.code !== 0);
    snapshots.ordinaryAfterClear = await json(["show", id, "--json"]);
    check("Clearing private editor does not clear canonical pending reply",
      snapshots.ordinaryAfterClear.runs.at(-1).pending_reply === block);
    const afterOrdinaryMessages = (await control.request("get_messages")).data.messages;
    check("Neither ordinary pending nor replacement text was submitted",
      !afterOrdinaryMessages.some(message => message.role === "user" &&
        [block, "Unrelated short input", "Still unrelated input"].some(text => messageWords(message).includes(text))));
    // Read only: the CLI alone created and mutated this store.
    const events = (await readFile(join(root, ".corral/events.jsonl"), "utf8"))
      .trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    snapshots.deliveryEvents = events.filter(e => e.kind === "replied");
    check("Successful delivery metadata is stored",
      snapshots.deliveryEvents.filter(e => e.detail.submitted === true).length === 2);
    check("Failed delivery metadata is stored",
      snapshots.deliveryEvents.some(e => e.detail.submitted === false));
    check("Delivery metadata includes transport and attempts",
      snapshots.deliveryEvents.every(e => typeof e.detail.via === "string" && Number.isInteger(e.detail.attempts)));
    const prepared = events.filter(e => e.kind === "reply_prepared" && e.detail.question);
    snapshots.prepared = prepared;
    check("Prepared replies store exact full native request bindings", prepared.length === 2 &&
      prepared.every((entry, index) =>
        entry.detail.question.request === `t170-native-${index + 1}-full-request-0123456789abcdef` &&
        entry.detail.run === run.id && entry.detail.pane === pane &&
        entry.detail.text === snapshots[`${entry.detail.question.request}-receipt`].response.comment &&
        entry.detail.question.session === registration.sessionFile &&
        entry.detail.question.session_id === sessions.data.sessions[0].sessionId &&
        entry.detail.question.pid === nativePid &&
        entry.detail.question.identity.length === 64));
    const lastDelivery = snapshots.deliveryEvents.filter(event => event.detail.via === "omo_question").at(-1).detail;
    check("Lost acknowledgement resend reconciles without another send",
      lastDelivery.submitted === true && lastDelivery.attempts === 0 && lastDelivery.via === "omo_question");
    check("Exactly one real successful answer ACK was dropped",
      signals.filter(r => r.type === "ack-dropped").length === 1);
    check("Native delivery uses the native transport metadata",
      snapshots.deliveryEvents.filter(e => e.detail.submitted).every(e => e.detail.via === "omo_question"));
    const ordinaryDeliveries = snapshots.deliveryEvents.filter(event => event.detail.via === "send_text");
    check("Collapsed ordinary reply and resend retain truthful zero-attempt pending receipts",
      ordinaryDeliveries.length === 2 && ordinaryDeliveries.every(event =>
        event.detail.submitted === false && event.detail.attempts === 0 && event.detail.text === block));
    snapshots.transcript = await readFile(registration.sessionFile, "utf8");
    const durableEntries = snapshots.transcript.trim().split("\n").map(line => JSON.parse(line));
    for (const entry of prepared) {
      check(`${entry.detail.question.request}: one durable native settlement`,
        durableEntries.filter(e => e.type === "custom" && e.customType === "ask-user:settlement" &&
          e.data.requestId === entry.detail.question.request && e.data.status === "comment-submitted").length === 1);
    }
    report.status = "PASS";
  } catch (error) {
    report.error = error.stack ?? String(error);
    if (pane) {
      const capture = await command("herdr", ["--session", "corral-test", "pane", "read", pane,
        "--source", "recent-unwrapped", "--lines", "100"], false);
      snapshots.failurePane = capture;
    }
  } finally {
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
          check("Fixture restores original native Socket.write", teardown.socketWriteRestored === true);
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
    peer?.destroy();
    await new Promise(resolveClose => ipc.close(resolveClose));
    await rm(root, { recursive: true, force: true });
    report.fixtureRemoved = !await exists(root);
    await writeFile(join(evidence, "commands.json"), JSON.stringify(commands, null, 2) + "\n");
    await writeFile(join(evidence, "native-signals.json"), JSON.stringify(signals, null, 2) + "\n");
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
