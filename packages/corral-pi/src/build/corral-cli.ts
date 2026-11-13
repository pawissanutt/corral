import { spawn } from "node:child_process";

export type Run = (argv: readonly string[], options: { cwd: string; timeoutMs?: number; input?: string }) => Promise<{ code: number; stdout: string; stderr: string }>;

const OUTPUT_LIMIT = 256 * 1024;
const AGENT_MARKERS = ["AI_AGENT", "PI_CODING_AGENT", "CORRAL_ASSISTANT", "CORRAL_PM"] as const;

function runner(executable: string, owner: boolean): Run {
  return async (argv, options) => {
    const env = { ...process.env };
    if (owner) for (const key of AGENT_MARKERS) delete env[key];
    const child = spawn(executable, [...argv], { cwd: options.cwd, env, shell: false, stdio: ["pipe", "pipe", "pipe"] });
    const { promise, resolve, reject } = Promise.withResolvers<{ code: number; stdout: string; stderr: string }>();
    const stdout: Buffer[] = [], stderr: Buffer[] = [];
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
    child.stdout.on("data", (chunk: Buffer) => collect(stdout, chunk));
    child.stderr.on("data", (chunk: Buffer) => collect(stderr, chunk));
    child.on("error", error => reject(new Error(`${executable}: ${error.message}`)));
    child.on("close", (code, signal) => {
      if (signal) failure ??= `${executable} terminated by ${signal}`;
      resolve({ code: failure ? (code || 1) : (code ?? 1), stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: [Buffer.concat(stderr).toString("utf8").trim(), failure].filter(Boolean).join("\n") });
    });
    child.stdin.on("error", error => { failure ??= `${executable} input failed: ${error.message}`; });
    child.stdin.end(options.input);
    try { return await promise; }
    finally { clearTimeout(timer); }
  };
}

export function ownerRun(corral: string): Run { return runner(corral, true); }
export function gitRun(): Run { return runner("git", false); }

export async function checked(run: Run, argv: readonly string[], cwd: string): Promise<string> {
  const result = await run(argv, { cwd });
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `${argv.join(" ")} exited ${result.code}`);
  return result.stdout;
}

export async function json<T>(run: Run, argv: readonly string[], cwd: string): Promise<T> {
  const result = await run(argv, { cwd });
  if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `corral ${argv.join(" ")} exited ${result.code}`);
  try { return JSON.parse(result.stdout) as T; }
  catch { throw new Error(`corral ${argv.join(" ")} returned invalid JSON${result.stderr.trim() ? `: ${result.stderr.trim()}` : " (stderr was empty)"}`); }
}

/** The fields consumed from the full `show --json` task, not the compact list projection. */
export interface TaskReadback {
  id: string;
  title: string;
  status: string;
  depends_on?: string[];
  landed?: { sha: string } | null;
  automation_blocker?: string | null;
  hold?: { reason?: string } | null;
  runs?: { gate?: { result: string; failed_cmd?: string } | null; autopilot?: { stage: string; reason?: string | null } | null }[];
}
