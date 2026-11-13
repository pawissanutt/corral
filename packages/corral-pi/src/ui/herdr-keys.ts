// Keys the owner's environment takes. The sidebar toggle is Ctrl+B unless the session runs inside herdr and
// Ctrl+B is herdr's prefix (its default), then Alt+B. The host reads herdr's config once at startup, with one
// bounded synchronous read of a regular file and never a subprocess; any failure falls back to herdr's default prefix.
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";
import { join } from "node:path";

export type SidebarKey = "ctrl+b" | "alt+b";
/** herdr's prefix when its config names none. */
const HERDR_DEFAULT_PREFIX = "ctrl+b";
/** A herdr config is a few kilobytes; anything larger is not read. */
export const HERDR_CONFIG_MAX_BYTES = 256 * 1024;

/**
 * The text of herdr's config, or undefined unless `path` is a regular file of at most HERDR_CONFIG_MAX_BYTES. It
 * opens without blocking (a FIFO at that path never stalls startup) and checks the opened file, not the path.
 */
export function readHerdrConfig(path: string): string | undefined {
  let fd: number;
  try { fd = openSync(path, constants.O_RDONLY | (constants.O_NONBLOCK ?? 0)); } catch { return undefined; }
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > HERDR_CONFIG_MAX_BYTES) return undefined;
    const buffer = Buffer.alloc(stat.size);
    const read = readSync(fd, buffer, 0, stat.size, 0);
    return buffer.toString("utf8", 0, read);
  } catch { return undefined; } finally { closeSync(fd); }
}

/**
 * Where herdr reads its `config.toml`, by the same precedence as herdr (crates/corral-herdr/src/config.rs):
 * `$XDG_CONFIG_HOME/herdr`, then `%APPDATA%\herdr` on Windows, then `~/.config/herdr`.
 */
export function herdrConfigPath(env: NodeJS.ProcessEnv, platform: NodeJS.Platform = process.platform): string | undefined {
  // An empty variable counts as unset, as herdr reads it.
  const set = (name: string) => env[name] || undefined;
  const xdg = set("XDG_CONFIG_HOME");
  if (xdg) return join(xdg, "herdr", "config.toml");
  if (platform === "win32") {
    const profile = set("USERPROFILE");
    const appdata = set("APPDATA") ?? (profile ? join(profile, "AppData", "Roaming") : undefined);
    if (appdata) return join(appdata, "herdr", "config.toml");
  }
  const home = set("HOME");
  return home ? join(home, ".config", "herdr", "config.toml") : undefined;
}

/** `keys.prefix` of a herdr config, lower case without spaces; undefined when the config names none or does not parse. */
export function herdrPrefix(text: string): string | undefined {
  const bun = Reflect.get(globalThis, "Bun") as { readonly TOML?: { parse(text: string): unknown } } | undefined;
  let config: unknown;
  try { config = bun?.TOML?.parse(text); } catch { return undefined; }
  const keys = typeof config === "object" && config !== null ? Reflect.get(config, "keys") : undefined;
  const prefix = typeof keys === "object" && keys !== null ? Reflect.get(keys, "prefix") : undefined;
  return typeof prefix === "string" && prefix.trim() ? prefix.replace(/\s+/g, "").toLowerCase() : undefined;
}

/**
 * The sidebar toggle: Alt+B inside herdr while herdr's prefix is Ctrl+B (named or by default), Ctrl+B everywhere
 * else. A missing, unreadable, oversized or non-regular config counts as herdr's default prefix.
 */
export function sidebarToggleKey(env: NodeJS.ProcessEnv = process.env,
  read: (path: string) => string | undefined = readHerdrConfig): SidebarKey {
  if (env.HERDR_ENV !== "1") return "ctrl+b";
  const path = herdrConfigPath(env);
  let text: string | undefined;
  if (path) { try { text = read(path); } catch { text = undefined; } }
  const prefix = text === undefined ? undefined : herdrPrefix(text);
  return (prefix ?? HERDR_DEFAULT_PREFIX) === "ctrl+b" ? "alt+b" : "ctrl+b";
}

/** The key cap label: `Ctrl+B` or `Alt+B`. */
export function sidebarKeyLabel(key: SidebarKey): string {
  return key === "alt+b" ? "Alt+B" : "Ctrl+B";
}
