// Where the corral-owned Pi credentials live (OD8): $XDG_DATA_HOME/corral/pi-native/auth/, or
// ~/.local/share/corral/pi-native/auth/ when XDG_DATA_HOME is unset or relative. The Rust side
// (crates/corral-ops/src/agent/install.rs, data_root) resolves the same directory. No other
// harness store (~/.codex, ~/.pi, ~/.claude, ...) is ever derived from here.
import { isAbsolute, join } from "node:path";

export const AUTH_FILE = "auth.json";

export class CredentialsUnavailable extends Error {
  readonly code = "credentials_unavailable";
  constructor(reason: string) {
    super(`credentials_unavailable: ${reason}`);
  }
}

export function piNativeRoot(env: NodeJS.ProcessEnv = process.env): string {
  const xdg = env.XDG_DATA_HOME;
  if (xdg && isAbsolute(xdg)) return join(xdg, "corral", "pi-native");
  const home = env.HOME;
  if (!home || !isAbsolute(home)) {
    throw new CredentialsUnavailable("neither XDG_DATA_HOME nor HOME is an absolute path");
  }
  return join(home, ".local", "share", "corral", "pi-native");
}

export function authDir(env: NodeJS.ProcessEnv = process.env): string {
  return join(piNativeRoot(env), "auth");
}
