// Assistant write classification (T-656): fixed refusals, default scratch/artifact/temp roots,
// then capability settings. Only the owner's scoped permission can authorize an ask decision.
import { lstatSync, realpathSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import type { RoleContext } from "../types.ts";
import { PermissionBroker } from "./permissions.ts";

export interface WriteSettings { readonly allow: readonly string[]; readonly deny: readonly string[] }
export interface PathContext {
  workspace: string; session: string; temp?: string; home?: string; protectedDirs?: readonly string[];
  cwd?: string;
  enabled: boolean; settings?: WriteSettings; scratch?: readonly string[];
}
export interface Classification {
  decision: "allow" | "refuse" | "ask"; rule: string; shown: string; canonical?: string;
  ignored?: string[];
}

/** Mirror of Rust write_permissions::scope_limit, applied to lexical and canonical forms. */
export function scopeLimit(path: string, home = homedir(), canonicalHome = real(home)): boolean {
  if (!isAbsolute(path) || path.split(sep).includes("..")) return false;
  const normalized = resolve(path);
  return normalized.split(sep).filter(Boolean).length > 1
    && ![resolve(home), canonicalHome].some((h) => h !== undefined && within(h, normalized));
}

export function settingPath(entry: string, home = homedir()): string {
  if (!entry || Buffer.byteLength(entry) > 4096 || /[\0*?\[]/.test(entry)
      || entry.split("/").includes("..") || !(isAbsolute(entry) || entry === "~" || entry.startsWith("~/"))) {
    throw new Error(`write_permissions invalid directory prefix ${JSON.stringify(entry)}`);
  }
  return resolve(entry.startsWith("~") ? join(home, entry.slice(1)) : entry);
}

export function writeSettings(value: unknown, home = homedir()): WriteSettings {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("write_permissions needs settings");
  const object = value as Record<string, unknown>;
  if (Object.keys(object).some((k) => k !== "allow" && k !== "deny")) throw new Error("write_permissions unknown setting");
  const lists = ["allow", "deny"].map((key) => {
    const entries = object[key] ?? [];
    if (!Array.isArray(entries) || entries.length > 64) throw new Error(`write_permissions.${key} needs at most 64 prefixes`);
    return entries.map((entry: unknown) => {
      if (typeof entry !== "string") throw new Error(`write_permissions.${key} needs strings`);
      const path = settingPath(entry, home);
      if (key === "allow" && !scopeLimit(path, home)) throw new Error(`write_permissions.allow entry ${entry} is broad`);
      return entry;
    });
  });
  return { allow: lists[0] ?? [], deny: lists[1] ?? [] };
}

export function classifyWrite(input: string, context: PathContext): Classification {
  const { workspace, session } = context;
  const home = context.home ?? homedir();
  const temp = context.temp ?? tmpdir();
  const shown = resolveToolPath(input, context.cwd ?? workspace);
  const canonical = real(shown);
  const result = (decision: Classification["decision"], rule: string): Classification => ({ decision, rule, shown, canonical });
  const own = [...(context.scratch ?? []), join(workspace, ".corral", "artifacts", "assistant", session)];
  const config = process.env.XDG_CONFIG_HOME || join(home, ".config");
  // Guards protect both rollout roots, even when absent; selectors must not narrow protection.
  const xdg = process.env.XDG_DATA_HOME;
  const dataParents = [join(home, ".local/share/corral"),
    ...(xdg && isAbsolute(xdg) ? [join(xdg, "corral")] : [])];
  const protectedDirs = context.protectedDirs ?? [join(config, "corral"), join(config, "herdr"),
    join(home, ".config", "corral"), join(home, ".config", "herdr"),
    ...dataParents.flatMap((parent) => [join(parent, "corral-pi"), join(parent, "pi-native")])];
  const protectedForms = protectedDirs.flatMap((p) => [p, real(p)]).filter((p): p is string => p !== undefined);
  for (const form of [shown, canonical]) {
    if (form === undefined) return result("refuse", "fixed_rule");
    if (form.split(sep).includes(".git") || protectedForms.some((p) => within(form, p))) return result("refuse", "fixed_rule");
    // Only the trusted prefix is exempt. Nested .git/.corral/.worktrees remain protected.
    const prefixes = own.flatMap((p) => [p, real(p)]).filter((p): p is string => p !== undefined);
    const prefix = prefixes.filter((p) => within(form, p)).sort((a, b) => b.length - a.length)[0];
    const components = (prefix ? relative(prefix, form) : form).split(sep);
    if (components.includes(".git") || components.includes(".corral") || components.includes(".worktrees")) return result("refuse", "fixed_rule");
  }
  if (canonical === undefined) return result("refuse", "fixed_rule");
  const roots = own.map(real).filter((p): p is string => p !== undefined);
  const realTemp = real(temp);
  const excluded = [home, workspace].flatMap((p) => [p, real(p)]).filter((p): p is string => p !== undefined);
  if (roots.some((p) => within(canonical, p)) || (realTemp && within(canonical, realTemp)
      && !excluded.some((p) => within(p, realTemp) && (within(shown, p) || within(canonical, p))))) return result("allow", "default");
  if (!context.enabled) return result("refuse", "capability_off");
  const forms = (entry: string) => { const lexical = settingPath(entry, home); return [lexical, real(lexical)].filter((p): p is string => p !== undefined); };
  if (context.settings?.deny.some((entry) => forms(entry).some((p) => within(shown, p) || within(canonical, p)))) return result("refuse", "configured_deny");
  const ignored: string[] = [];
  for (const entry of context.settings?.allow ?? []) {
    const lexical = settingPath(entry, home); const resolved = real(lexical);
    if (!resolved || !scopeLimit(lexical, home) || !scopeLimit(resolved, home)) { ignored.push(entry); continue; }
    if (within(canonical, resolved)) return { ...result("allow", "config"), ignored };
  }
  return { ...result("ask", "permission"), ignored };
}

/** Candidates never rise above an enclosing Git work tree, even when its root is far away. */
export function scopeCandidates(target: string, home = homedir()): { shown: string; canonical: string }[] {
  let root: string | undefined;
  for (let p = dirname(target); ; p = dirname(p)) {
    if (present(join(p, ".git"))) { root = p; break; }
    if (p === dirname(p)) break;
  }
  const directory = dirname(target);
  const canonicalTarget = real(target);
  const candidates = [directory, dirname(directory), dirname(dirname(directory)), ...(root ? [root] : [])];
  return [...new Set(candidates)].filter((p) => !root || within(p, root)).flatMap((shown) => {
    const canonical = real(shown);
    return canonical && canonicalTarget && within(canonicalTarget, canonical)
      && scopeLimit(shown, home) && scopeLimit(canonical, home) ? [{ shown, canonical }] : [];
  });
}

const WRITE_TOOLS: ReadonlySet<string> = new Set(["edit", "write"]);
const UNICODE_SPACES = /[\u00A0\u1680\u2000-\u200A\u202F\u205F\u3000]/g;

/** A tool path resolved as Pi's edit and write tools resolve it: `@`, `~`, file URLs, cwd. */
export function resolveToolPath(input: string, cwd: string): string {
  let path = input.replace(UNICODE_SPACES, " ");
  if (path.startsWith("@")) path = path.slice(1);
  if (path === "~") path = homedir();
  else if (path.startsWith("~/")) path = join(homedir(), path.slice(2));
  else if (path.startsWith("file://")) path = fileURLToPath(path);
  return resolve(cwd, path);
}

function present(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

/** Where a write lands once symlinks in the existing part of the path are followed. */
export function real(path: string): string | undefined {
  const rest: string[] = [];
  let existing = path;
  while (!present(existing)) {
    const parent = dirname(existing);
    if (parent === existing) return undefined;
    rest.unshift(basename(existing));
    existing = parent;
  }
  try {
    return join(realpathSync(existing), ...rest);
  } catch {
    return undefined;
  }
}

export function within(path: string, root: string): boolean {
  const rel = relative(root, path);
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}

/** Recorded roots from the canonical assistant-show response, never a legacy Markdown mirror. */
export function scratchRoots(snapshot: unknown, session: string): string[] {
  if (!snapshot || typeof snapshot !== "object" || !("session" in snapshot)) return [];
  const record = snapshot.session;
  if (!record || typeof record !== "object" || !("id" in record) || record.id !== session
      || !("scratch" in record) || !Array.isArray(record.scratch)) return [];
  return record.scratch.flatMap((entry: unknown) => {
    if (!entry || typeof entry !== "object" || !("path" in entry) || typeof entry.path !== "string") return [];
    const path = entry.path;
    return isAbsolute(path) && basename(dirname(path)) === ".worktrees" && basename(path).startsWith(`${session}-`) ? [path] : [];
  });
}

/** Why the assistant may not edit or write `input`, or undefined when it is inside its roots. */
export function writeRefusal(
  input: string, workspace: string, session: string, temp: string = tmpdir(), scratch: readonly string[] = [],
): string | undefined {
  const classification = classifyWrite(input, { workspace, session, temp, scratch, enabled: false });
  if (classification.decision === "allow") return undefined;
  if (classification.rule === "fixed_rule") return "assistant rule: fixed_rule; nothing was written.";
  return `assistant rule: edit and write reach only this session's scratch worktrees, `
    + `.corral/artifacts/assistant/${session}/ and the temp directory, not ${JSON.stringify(input)}; `
    + "nothing was written. Make a scratch with `corral assistant scratch new -p <project>` and edit there.";
}

/** Classify every edit/write, top-level or nested, and broker external permissions. */
export function installWriteGuard(pi: ExtensionAPI, context: RoleContext): void {
  const session = context.identity.assistant;
  const broker = new PermissionBroker(context);
  pi.on("session_start", async (_event, ctx) => { await context.writePermission?.onBound(() => broker.notice(ctx)); });
  pi.on("tool_call", async (event, ctx) => {
    if (!WRITE_TOOLS.has(event.toolName)) return undefined;
    const path = (event.input as Record<string, unknown>).path;
    if (session === undefined || typeof path !== "string") {
      return { block: true, reason: "assistant rule: edit and write need a path and a bound assistant session; nothing was written." };
    }
    return broker.authorize(event.toolName, event.input as Record<string, unknown>, event.toolCallId, ctx);
  });
}
