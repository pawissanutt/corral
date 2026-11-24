// A single-flight ACP v1 peer over JSON-RPC NDJSON. This owns an ordinary POSIX process
// group, not an OS sandbox. See T-2007: ACP capabilities do not constrain native agy tools.
import { spawn, type ChildProcessByStdio } from "node:child_process";
import { accessSync, constants, mkdirSync, realpathSync, statSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { Readable, Writable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";

export interface AgyLaunch {
  /** Explicit trusted executable (or sandbox launcher); never looked up on PATH. */
  executable: string;
  /** Caller-owned isolated workspace and private agy state. Neither is deleted by transport. */
  cwd: string;
  home: string;
  setupTimeoutMs?: number;
  turnTimeoutMs?: number;
  cancelTimeoutMs?: number;
}

type Json = Record<string, unknown>;
export function record(value: unknown): Json | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Json : undefined;
}

type Fault = "launch" | "protocol" | "auth" | "quota" | "rate_limit" | "unavailable" | "timeout" | "aborted" | "cleanup";
export class AcpFailure extends Error {
  constructor(readonly category: Fault, detail: string, readonly source?: "executor_warning") {
    super(`agy ACP: ${detail}`);
  }
}

function providerFailure(status: unknown, code: unknown, text: string, source?: "executor_warning"): AcpFailure | undefined {
  if (status === "UNAUTHENTICATED" || status === "PERMISSION_DENIED" || code === 401 || code === 403 ||
    /^authentication required\b/i.test(text)) return new AcpFailure("auth", "authentication required", source);
  if (status === "RESOURCE_EXHAUSTED" || /\b(?:quota exceeded|quota exhausted|insufficient quota|resource_exhausted)\b/i.test(text)) {
    return new AcpFailure("quota", "quota exhausted", source);
  }
  if (code === 429 || status === "RATE_LIMIT_EXCEEDED" || /\brate limit (?:exceeded|reached)\b/i.test(text)) {
    return new AcpFailure("rate_limit", "429 rate limit exceeded", source);
  }
}

export function rpcFailure(error: Json): AcpFailure {
  const data = record(error.data);
  const text = typeof error.message === "string" ? error.message.slice(0, 512) : "";
  // -32000 is ACP's AuthRequired. Never display error data, raw messages or stderr.
  return providerFailure(data?.status, data?.code ?? error.code, text) ?? (error.code === -32000
    ? new AcpFailure("auth", "authentication required") : new AcpFailure("protocol", "request failed"));
}

export function executorWarning(text: string): AcpFailure | undefined {
  // agy 1.5.14 can send a backend fault as a single warning chunk, then report end_turn.
  // Recognize the fixed envelope, not arbitrary model prose. This is display evidence only;
  // the text channel carries no authenticated signal for retries, routing, or account state.
  const prefix = "\n⚠ agent executor error: generating and executing: ";
  if (!text.startsWith(prefix) || !text.endsWith("\n")) return;
  const backend = /^Error (\d{3}), Message: ([\s\S]*), Status: ([A-Z_]+), Details: /.exec(text.slice(prefix.length));
  return providerFailure(backend?.[3], Number(backend?.[1]), backend?.[2]?.slice(0, 512) ?? "", "executor_warning") ??
    new AcpFailure("protocol", "agent executor failed", "executor_warning");
}

const FRAME_BYTES = 1024 * 1024;
const OUTPUT_BYTES = 8 * FRAME_BYTES;
const OUTPUT_FRAMES = 8192;
const TERMINATE_GRACE_MS = 150;

export function deadlineMs(value: number | undefined, fallback: number, max: number): number {
  if (value === undefined) return fallback;
  if (!Number.isSafeInteger(value) || value < 1 || value > max) throw new AcpFailure("launch", "invalid deadline");
  return value;
}

export async function bounded<T>(work: () => Promise<T>, ms: number, stage: string, signal?: AbortSignal): Promise<T> {
  const aborted = () => new AcpFailure("aborted", "turn cancelled");
  if (signal?.aborted) throw aborted();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  const expired = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new AcpFailure("timeout", `${stage} timed out`)), ms);
    onAbort = () => reject(aborted());
    signal?.addEventListener("abort", onAbort, { once: true });
  });
  try {
    return await Promise.race([expired, work()]);
  } finally {
    clearTimeout(timer);
    if (onAbort) signal?.removeEventListener("abort", onAbort);
  }
}

function validatePaths(launch: AgyLaunch): { executable: string; cwd: string; home: string } {
  try {
    if (![launch.executable, launch.cwd, launch.home].every(path => isAbsolute(path))) throw new Error();
    const executable = realpathSync(launch.executable);
    const cwd = realpathSync(launch.cwd);
    const home = realpathSync(launch.home);
    if (!statSync(executable).isFile() || !statSync(cwd).isDirectory() || !statSync(home).isDirectory()) throw new Error();
    accessSync(executable, constants.X_OK);
    return { executable, cwd, home };
  } catch {
    throw new AcpFailure("launch", "expected an absolute executable file and existing absolute cwd and home directories");
  }
}

function privateEnvironment(home: string): Record<string, string> {
  const env: Record<string, string> = { HOME: home, PATH: "/usr/bin:/bin", LANG: "C.UTF-8", TERM: "dumb" };
  for (const [key, dir] of Object.entries({ XDG_CONFIG_HOME: "config", XDG_DATA_HOME: "data", XDG_STATE_HOME: "state",
    XDG_CACHE_HOME: "cache", XDG_RUNTIME_DIR: "runtime", TMPDIR: "tmp" })) {
    const path = join(home, dir);
    try {
      mkdirSync(path, { recursive: true, mode: 0o700 });
      if (realpathSync(path) !== path) throw new Error();
    } catch {
      throw new AcpFailure("launch", "cannot prepare private agy environment");
    }
    env[key] = path;
  }
  return env;
}

async function* frames(stdout: Readable): AsyncGenerator<Json> {
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let parts: Buffer[] = [], size = 0, received = 0, count = 0;
  for await (const chunk of stdout as AsyncIterable<Buffer>) {
    received += chunk.length;
    if (received > OUTPUT_BYTES) throw new AcpFailure("protocol", "output byte limit exceeded");
    for (let start = 0; start < chunk.length;) {
      const newline = chunk.indexOf(10, start);
      const part = chunk.subarray(start, newline < 0 ? chunk.length : newline);
      size += part.length;
      if (size > FRAME_BYTES) throw new AcpFailure("protocol", "frame too large");
      if (part.length) parts.push(part);
      if (newline < 0) break;
      if (++count > OUTPUT_FRAMES) throw new AcpFailure("protocol", "output frame limit exceeded");
      let message: Json | undefined;
      try { message = record(JSON.parse(decoder.decode(Buffer.concat(parts, size)))); } catch { /* invalid wire */ }
      if (message?.jsonrpc !== "2.0") throw new AcpFailure("protocol", "invalid JSON-RPC frame");
      parts = [];
      size = 0;
      start = newline + 1;
      yield message;
    }
  }
  throw new AcpFailure("protocol", size ? "truncated frame" : "subprocess closed its output before responding");
}

type Child = ChildProcessByStdio<Writable, Readable, null>;
type Observe = (message: Json) => void | Promise<void>;

export class AcpPeer {
  readonly cwd: string;
  private readonly child: Child;
  private readonly input: AsyncGenerator<Json>;
  private readonly exited: Promise<void>;
  private closed = false;
  private spawnFailed = false;
  private requestId = 0;

  constructor(launch: AgyLaunch) {
    if (process.platform === "win32") throw new AcpFailure("launch", "requires POSIX process groups");
    const { executable, cwd, home } = validatePaths(launch);
    this.cwd = cwd;
    const env = privateEnvironment(home);
    const geminiDir = join(home, ".gemini");
    try {
      mkdirSync(geminiDir, { recursive: true, mode: 0o700 });
      if (realpathSync(geminiDir) !== geminiDir) throw new Error();
      this.child = spawn(executable, ["acp", "--stdio", "--disable-telemetry", "--sandbox", "--ide-mcp-port=0",
        "--include-default-customizations=false", `--gemini-dir=${geminiDir}`], {
        cwd, env, stdio: ["pipe", "pipe", "ignore"], shell: false, detached: true,
      });
    } catch {
      throw new AcpFailure("launch", "could not launch subprocess");
    }
    this.exited = new Promise(resolve => {
      this.child.once("exit", () => resolve());
      this.child.once("error", () => { this.spawnFailed = true; resolve(); });
    });
    // A failed write also rejects the callback promise. Handle the stream error event separately.
    this.child.stdin.on("error", () => undefined);
    this.input = frames(this.child.stdout);
  }

  private async send(value: Json): Promise<void> {
    if (this.closed) throw new AcpFailure("protocol", "connection closed");
    const line = JSON.stringify({ jsonrpc: "2.0", ...value }) + "\n";
    if (Buffer.byteLength(line) - 1 > FRAME_BYTES) throw new AcpFailure("protocol", "request frame too large");
    await new Promise<void>((resolve, reject) => this.child.stdin.write(line, error => error
      ? reject(new AcpFailure(this.child.pid === undefined ? "launch" : "protocol", "could not write to subprocess"))
      : resolve()));
  }

  async notify(method: string, params: Json): Promise<void> { await this.send({ method, params }); }

  private async receive(observe: Observe): Promise<Json> {
    const next = await this.input.next();
    if (next.done || this.closed) throw new AcpFailure("protocol", "connection closed");
    const message = next.value;
    await observe(message);
    if (this.closed) throw new AcpFailure("protocol", "connection closed");
    return message;
  }

  private async handleMethod(message: Json): Promise<void> {
    if (typeof message.method !== "string") throw new AcpFailure("protocol", "invalid JSON-RPC method");
    if (!("id" in message)) return; // notification
    const { id } = message;
    if (typeof id !== "string" && !Number.isSafeInteger(id)) throw new AcpFailure("protocol", "invalid JSON-RPC request ID");
    if (message.method === "session/request_permission") {
      await this.send({ id, result: { outcome: { outcome: "cancelled" } } });
    } else {
      await this.send({ id, error: { code: -32601, message: "No ACP host capability for this method" } });
    }
  }

  async request(method: string, params: Json, observe: Observe): Promise<Json> {
    const id = ++this.requestId;
    await this.send({ id, method, params });
    for (;;) {
      const message = await this.receive(observe);
      if ("method" in message) { await this.handleMethod(message); continue; }
      if (message.id !== id || ("error" in message) === ("result" in message)) {
        throw new AcpFailure("protocol", "unexpected JSON-RPC response");
      }
      const error = record(message.error);
      if (error && Number.isSafeInteger(error.code) && typeof error.message === "string") throw rpcFailure(error);
      const result = record(message.result);
      if (!result) throw new AcpFailure("protocol", "invalid JSON-RPC response");
      return result;
    }
  }

  async updatesUntil(ready: () => boolean, observe: Observe): Promise<void> {
    while (!ready()) await this.handleMethod(await this.receive(observe));
  }

  async close(): Promise<void> {
    this.closed = true;
    const { child } = this;
    const signalGroup = (signal: NodeJS.Signals | 0): boolean => {
      if (!child.pid || this.spawnFailed) return false;
      try { process.kill(-child.pid, signal); return true; }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ESRCH") return false;
        throw new AcpFailure("cleanup", "could not terminate subprocess group");
      }
    };
    try {
      const signalled = signalGroup("SIGTERM");
      child.stdin.destroy();
      child.stdout.destroy();
      if (signalled) {
        await Promise.race([this.exited, delay(TERMINATE_GRACE_MS)]);
        // An exited leader does not imply its workers are gone.
        signalGroup("SIGKILL");
      }
      await bounded(() => this.exited, 1000, "subprocess reap");
    } finally {
      child.stdin.destroy();
      child.stdout.destroy();
      child.unref();
      void this.input.return(undefined).catch(() => undefined);
    }
  }
}
