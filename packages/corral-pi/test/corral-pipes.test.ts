// Real disposable processes; exit and output are synchronized with IPC/socket barriers.
import { afterAll, expect, test } from "bun:test";
import { type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CorralOp } from "../src/roles/types.ts";
import { CorralCallError, CorralClient } from "../src/tools/corral.ts";

const dir = mkdtempSync(join(tmpdir(), "corral-pipes-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));
const call = { requestId: `rq-${"a".repeat(32)}`, payload: { text: "mutation" } };

class ControlledClient extends CorralClient {
  executor?: ChildProcessWithoutNullStreams;
  expire?: () => void;
  protected override spawnCall(op: CorralOp) {
    this.executor = super.spawnCall(op);
    return this.executor;
  }
  protected override deadline(expire: () => void) {
    this.expire = expire;
    return () => { this.expire = undefined; };
  }
}

function fixture(pipe: "stdout" | "stderr", receipt: boolean) {
  const work = mkdtempSync(join(dir, "case-"));
  const socket = join(work, "control");
  const script = join(work, "corral");
  const descendant = `
    import { createServer } from "node:net";
    createServer(s => { process.${pipe}.write("x".repeat(300000), () => s.end("sent")); })
      .listen(${JSON.stringify(socket)}, () => process.send("ready"));
  `;
  writeFileSync(script, `#!/usr/bin/env bun
    import { spawn } from "node:child_process";
    import { readFileSync, writeFileSync } from "node:fs";
    const input = JSON.parse(readFileSync(0, "utf8"));
    const child = spawn(process.execPath, ["-e", ${JSON.stringify(descendant)}], {
      stdio: ["ignore", ${pipe === "stdout" ? "1" : '"ignore"'}, ${pipe === "stderr" ? "2" : '"ignore"'}, "ipc"],
    });
    child.on("message", () => {
      writeFileSync("descendant", String(child.pid));
      ${receipt ? 'process.stdout.write(JSON.stringify({version:1, op:input.op, request_id:input.request_id, state:"committed", result:{effect:"recorded"}}) + "\\n");' : 'process.stderr.write("executor detail");'}
      process.exit(0);
    });
  `, { mode: 0o700 });
  const client = new ControlledClient({ corral: script, root: work, identity: {
    store: "st-test", project: "demo", role: "worker", task: "T-1", run: "R1", generation: 1,
  }, hostCap: "h".repeat(64), sessionId: () => "s" });
  return { client, socket, cleanup() { process.kill(Number(readFileSync(join(work, "descendant"), "utf8")), "SIGKILL"); } };
}

// This is a test hang guard, not a synchronization delay or a production deadline change.
async function bounded<T>(promise: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("call did not settle after its bound")), 1000);
    })]);
  } finally { clearTimeout(timer); }
}

async function flood(path: string) {
  const socket = connect(path);
  try {
    socket.resume();
    await once(socket, "end");
  } finally { socket.destroy(); }
}

for (const pipe of ["stdout", "stderr"] as const) {
  for (const receipt of [false, true]) test(`deadline after executor exit with retained ${pipe}, receipt=${receipt}`, async () => {
    const fx = fixture(pipe, receipt);
    const outcome = fx.client.call("comment", call).then(value => value, (error: unknown) => error);
    const executor = fx.client.executor;
    if (!executor) throw new Error("executor not started");
    try {
      // Corral's exit listener runs first; the descendant is now the only pipe holder.
      await once(executor, "exit");
      expect(executor.exitCode).toBe(0);
      if (!fx.client.expire) throw new Error("deadline not armed");
      fx.client.expire();
      const result = await bounded(outcome);
      if (receipt) {
        expect(result).toMatchObject({ request_id: call.requestId, state: "committed", result: { effect: "recorded" } });
      } else {
        if (!(result instanceof CorralCallError)) throw new Error("expected transport error");
        expect(result.code).toBe("call_timeout");
        expect(result.diagnostics).toMatchObject({ exit_code: 0, signal: null, timed_out: true, effect: "unknown", diagnostic: "executor detail" });
      }
    } finally { fx.cleanup(); await outcome; }
  });

  for (const receipt of pipe === "stderr" ? [false, true] : [false]) test(`output bound after executor exit with retained ${pipe}, receipt=${receipt}`, async () => {
    const fx = fixture(pipe, receipt);
    const outcome = fx.client.call("comment", call).then(value => value, (error: unknown) => error);
    const executor = fx.client.executor;
    if (!executor) throw new Error("executor not started");
    try {
      await once(executor, "exit");
      expect(executor.exitCode).toBe(0);
      await flood(fx.socket);
      const result = await bounded(outcome);
      if (receipt) {
        expect(result).toMatchObject({ request_id: call.requestId, state: "committed", result: { effect: "recorded" } });
      } else {
        if (!(result instanceof CorralCallError)) throw new Error("expected transport error");
        expect(result.code).toBe("call_output");
        expect(result.diagnostics).toMatchObject({ exit_code: 0, signal: null, timed_out: false, effect: "unknown" });
        expect(result.diagnostics.diagnostic?.length).toBeLessThanOrEqual(4096);
      }
    } finally { fx.cleanup(); await outcome; }
  });
}
