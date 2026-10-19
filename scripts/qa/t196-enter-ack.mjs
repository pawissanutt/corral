// Fixture-only herdr fault seam. The coordinator arms real-engine observations
// before permitting input, and decides whether an accepted submission loses its ACK.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createConnection } from "node:net";
import { createInterface } from "node:readline";

const [path, engine, ...args] = process.argv.slice(2);
assert(path?.startsWith("/tmp/corral-herdr-"), "Private fixture socket required");
assert.deepEqual(args.slice(0, 4), ["--session", "corral-test", "pane", "send-keys"]);
assert.equal(args[5], "enter");
const socket = createConnection(path);
const lines = createInterface({ input: socket });
const replies = lines[Symbol.asyncIterator]();
let child;
async function bounded(promise) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("Engine acceptance handshake deadline")), 60_000);
    })]);
  } finally { clearTimeout(timer); }
}
async function response() {
  const next = await bounded(replies.next());
  assert(!next.done, "Acceptance coordinator closed before its receipt");
  const record = JSON.parse(next.value);
  assert(!record.error, record.error);
  return record;
}
try {
  await bounded(once(socket, "connect"));
  socket.write(JSON.stringify({ op: "prepare" }) + "\n");
  const prepared = await response();
  assert.equal(prepared.ready, true);
  // Force the review's edited-but-unsent outcome once. This is a real bracketed
  // terminal paste of CR, not an editor mutation or a synthetic user message.
  const input = prepared.editDraft
    ? ["--session", "corral-test", "pane", "send-text", args[4], "\x1b[200~\r\x1b[201~"] : args;
  child = spawn(engine, input, { stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "", stderr = "";
  child.stdout.on("data", bytes => { stdout += bytes; process.stdout.write(bytes); });
  child.stderr.on("data", bytes => { stderr += bytes; process.stderr.write(bytes); });
  const [code] = await bounded(once(child, "close"));
  socket.write(JSON.stringify({ op: "written", token: prepared.token,
    program: engine, args: input, code, stdout, stderr }) + "\n");
  const receipt = await response();
  if (receipt.drop) {
    assert(Number.isInteger(receipt.acceptedSequence), "Lost ACK requires real engine acceptance");
    console.error("T196 fixture: exact engine submission accepted; acknowledgement lost");
    process.exitCode = 1;
  } else {
    process.exitCode = code ?? 1;
  }
} catch (error) {
  console.error(error.stack ?? String(error));
  process.exitCode = 1;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exited = once(child, "exit");
    child.kill("SIGKILL");
    await bounded(exited);
  }
  lines.close();
  socket.destroy();
}
