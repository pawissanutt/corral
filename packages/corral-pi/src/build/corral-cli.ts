import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import type { TaskDifficulty } from "./types.ts";

export type Run = (argv: readonly string[], options: {
  cwd: string;
  timeoutMs?: number;
  input?: string;
  /** Overrides for this child only; other environment variables are inherited. */
  env?: Readonly<NodeJS.ProcessEnv>;
  stdoutDigest?: "sha256";
  stdoutFd?: number;
  /** Consume stdout as bytes instead of collecting it; the result's stdout will be empty. */
  stdoutChunk?: (chunk: Buffer) => void;
}) => Promise<{ code: number; stdout: string; stderr: string }>;

const OUTPUT_LIMIT = 256 * 1024;

export function engineRun(executable: string): Run {
  return async (argv, options) => {
    if (options.stdoutChunk && (options.stdoutDigest || options.stdoutFd !== undefined)) throw new Error("Choose one stdout consumer");
    const child = spawn(executable, [...argv], { cwd: options.cwd, shell: false,
      env: options.env && { ...process.env, ...options.env }, stdio: ["pipe", options.stdoutFd ?? "pipe", "pipe"] });
    const { promise, resolve, reject } = Promise.withResolvers<{ code: number; stdout: string; stderr: string }>();
    const stdout: Buffer[] = [], stderr: Buffer[] = [];
    const digest = options.stdoutDigest ? createHash(options.stdoutDigest) : undefined;
    let bytes = 0;
    let failure: string | undefined;
    const stop = (reason: string) => {
      failure ??= reason;
      child.kill("SIGKILL");
    };
    const timer = setTimeout(() => stop(`${executable} timed out after ${options.timeoutMs ?? 120_000} ms`), options.timeoutMs ?? 120_000);
    function collect(chunks: Buffer[], chunk: Buffer) {
      const allowed = Math.max(0, OUTPUT_LIMIT - bytes);
      bytes += chunk.byteLength;
      if (allowed) chunks.push(chunk.subarray(0, allowed));
      if (bytes > OUTPUT_LIMIT) stop(`${executable} exceeded the 256 KiB output limit`);
    }
    child.stdout?.on("data", (chunk: Buffer) => {
      if (digest) digest.update(chunk);
      else if (options.stdoutChunk) {
        if (failure) return;
        try { options.stdoutChunk(chunk); }
        catch { stop(`${executable} stdout consumer failed`); }
      } else collect(stdout, chunk);
    });
    child.stderr?.on("data", (chunk: Buffer) => collect(stderr, chunk));
    child.on("error", error => reject(new Error(`${executable}: ${error.message}`)));
    child.on("close", (code, signal) => {
      if (signal) failure ??= `${executable} terminated by ${signal}`;
      resolve({ code: failure ? (code || 1) : (code ?? 1), stdout: digest ? digest.digest("hex") : Buffer.concat(stdout).toString("utf8"),
        stderr: [Buffer.concat(stderr).toString("utf8").trim(), failure].filter(Boolean).join("\n") });
    });
    child.stdin?.on("error", error => { failure ??= `${executable} input failed: ${error.message}`; });
    child.stdin?.end(options.input);
    try { return await promise; }
    finally { clearTimeout(timer); }
  };
}

export function gitRun(): Run { return engineRun("git"); }

export async function checked(run: Run, argv: readonly string[], cwd: string): Promise<string> {
  const result = await run(argv, { cwd });
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `${argv.join(" ")} exited ${result.code}`);
  return result.stdout;
}

export async function json<T>(run: Run, argv: readonly string[], cwd: string, timeoutMs?: number): Promise<T> {
  const result = await run(argv, { cwd, ...(timeoutMs === undefined ? {} : { timeoutMs }) });
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `corral ${argv.join(" ")} exited ${result.code}`);
  try { return JSON.parse(result.stdout) as T; }
  catch { throw new Error(`corral ${argv.join(" ")} returned invalid JSON${result.stderr.trim() ? `: ${result.stderr.trim()}` : " (stderr was empty)"}`); }
}

/** Task fields used by the Build follow-on display. */
export interface TaskReadback {
  id: string;
  title: string;
  status: string;
  size?: string | null;
  difficulty?: TaskDifficulty | null;
  tags?: string[];
  depends_on?: string[];
  blocked?: { reason: string } | null;
}
