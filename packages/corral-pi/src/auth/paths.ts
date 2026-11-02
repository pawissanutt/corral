// Where the corral-owned Pi credentials live (OD8): $XDG_DATA_HOME/corral/corral-pi/auth/, or
// ~/.local/share/corral/corral-pi/auth/ when XDG_DATA_HOME is unset or relative. The Rust side
// (crates/corral-ops/src/agent/install.rs, data_root) resolves the same directory. No other
// harness store (~/.codex, ~/.pi, ~/.claude, ...) is ever derived from here.
import { existsSync, statSync } from "node:fs";
import { dirname, isAbsolute, join } from "node:path";

export const AUTH_FILE = "auth.json";

export class CredentialsUnavailable extends Error {
  readonly code = "credentials_unavailable";
  constructor(reason: string) {
    super(`credentials_unavailable: ${reason}`);
  }
}

/** Read legacy installations in place; never move data that an older binary still uses. */
export function corralPiDirectory(target: string): string {
  const legacy = join(dirname(target), "pi-native");
  return !existsSync(target) && existsSync(legacy) && statSync(legacy).isDirectory() ? legacy : target;
}

export function corralPiRoot(env: NodeJS.ProcessEnv = process.env): string {
  const xdg = env.XDG_DATA_HOME;
  if (xdg && isAbsolute(xdg)) return corralPiDirectory(join(xdg, "corral", "corral-pi"));
  const home = env.HOME;
  if (!home || !isAbsolute(home)) {
    throw new CredentialsUnavailable("neither XDG_DATA_HOME nor HOME is an absolute path");
  }
  return corralPiDirectory(join(home, ".local", "share", "corral", "corral-pi"));
}

export function authDir(env: NodeJS.ProcessEnv = process.env): string {
  return join(corralPiRoot(env), "auth");
}
