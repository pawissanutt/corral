// Account operations only. Starting an ACP model session here would bypass Corral's
// tool boundary (T-2007). Keep this module separate from provider/session registration.
import { spawnSync } from "node:child_process";
import { accessSync, constants, mkdtempSync, realpathSync, rmSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, isAbsolute, join } from "node:path";

export const AGY_PROVIDER = "antigravity-cli";

const SIGN_IN = "Please sign in to view available models. Launch the CLI without arguments to sign in.";
const EFFORT = /-(off|minimal|low|medium|high|xhigh|max)$/;
const GEMINI_ROW = /^(gemini-[a-z0-9][a-z0-9.-]{0,119})\t[^\t\r\n]+$/;

export interface AgyListedModel {
  readonly provider: typeof AGY_PROVIDER;
  readonly id: string;
  readonly selector: string;
  /** Effort is part of the exact model ID, not an override on a base model. */
  readonly effort?: string;
}

export class AgyAccountError extends Error {
  constructor(readonly code: "agy_missing" | "agy_signed_out" | "agy_catalog_unavailable" | "agy_no_models"
    | "agy_login_requires_terminal" | "agy_login_failed", message: string) {
    super(`${code}: ${message}`);
  }
}

function executable(): string {
  // Ignore empty and relative PATH entries. Resolve the user's explicit install
  // before changing to the neutral cwd.
  for (const dir of (process.env.PATH ?? "").split(delimiter).filter(isAbsolute)) {
    try {
      const resolved = realpathSync(join(dir, "agy"));
      accessSync(resolved, constants.X_OK);
      if (statSync(resolved).isFile()) return resolved;
    } catch { /* try the next directory */ }
  }
  throw new AgyAccountError("agy_missing", "Install the Antigravity CLI (agy) and put it on PATH.");
}

function accountEnvironment(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { HOME: homedir(), PATH: "/usr/bin:/bin", LANG: "C.UTF-8" };
  // agy alone owns its OAuth state and any interactive browser flow. No inherited
  // Gemini/Google API credentials, alternate model endpoints, or Corral state are passed.
  for (const key of ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR"]) {
    const value = process.env[key];
    if (value && isAbsolute(value)) env[key] = value;
  }
  return env;
}

function withAccount<T>(failure: () => AgyAccountError, run: (command: string, cwd: string, env: NodeJS.ProcessEnv) => T): T {
  const command = executable();
  try {
    // Do not inherit TMPDIR: it can point inside the invoking workspace.
    const cwd = mkdtempSync("/tmp/corral-agy-account-");
    try { return run(command, cwd, accountEnvironment()); }
    finally { rmSync(cwd, { recursive: true, force: true }); }
  } catch (error) {
    throw error instanceof AgyAccountError ? error : failure();
  }
}

export function discoverAgyModels(): AgyListedModel[] {
  const unavailable = () => new AgyAccountError("agy_catalog_unavailable", "Antigravity could not list account models; run `agy models` to troubleshoot.");
  return withAccount(unavailable, (command, cwd, env) => {
    const result = spawnSync(command, ["models"], {
      cwd, env, stdio: ["ignore", "pipe", "pipe"], encoding: "utf8", timeout: 15_000,
      maxBuffer: 256 * 1024, killSignal: "SIGKILL",
    });
    if (!result.error && result.status !== 0 && result.stderr.split(/\r?\n/).includes(`Error: ${SIGN_IN}`)) {
      throw new AgyAccountError("agy_signed_out", `Run corral-pi login --provider ${AGY_PROVIDER}.`);
    }
    if (result.error || result.status !== 0) throw unavailable();
    const models = new Map<string, AgyListedModel>();
    for (const line of result.stdout.split(/\r?\n/)) {
      const id = GEMINI_ROW.exec(line)?.[1];
      if (!id || models.has(id)) continue;
      const effort = EFFORT.exec(id)?.[1];
      models.set(id, { provider: AGY_PROVIDER, id, selector: `${AGY_PROVIDER}/${id}`, ...(effort ? { effort } : {}) });
    }
    if (!models.size) throw new AgyAccountError("agy_no_models", "Antigravity did not advertise any Gemini models for this account.");
    return [...models.values()];
  });
}

export function loginAgy(): void {
  // Never send OAuth prompts or codes to a captured pipe. This invokes only the
  // CLI's documented terminal auth operation, not a model/agent session.
  if (!process.stdin.isTTY || !process.stdout.isTTY || !process.stderr.isTTY) {
    throw new AgyAccountError("agy_login_requires_terminal", "Run `agy acp --login` directly from a terminal.");
  }
  const failed = () => new AgyAccountError("agy_login_failed", "Antigravity login did not complete.");
  withAccount(failed, (command, cwd, env) => {
    for (const key of ["TERM", "COLORTERM", "DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]) {
      if (process.env[key]) env[key] = process.env[key];
    }
    const result = spawnSync(command, ["acp", "--login", "--disable-telemetry"], { cwd, env, stdio: "inherit" });
    if (result.error || result.status !== 0) throw failed();
  });
}
