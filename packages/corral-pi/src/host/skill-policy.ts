import { type Dirent, lstatSync, opendirSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

import type { LoadSkillsResult } from "@earendil-works/pi-coding-agent";

import { PLAN_SKILL_ASSET, WORKER_SKILL_ASSET } from "../roles/common/assets.ts";
import { readPiSettings } from "./pi-settings-file.ts";

export interface SkillPolicy {
  readonly disabled: ReadonlySet<string>;
  readonly userDirs: readonly string[];
}

export function readSkillPolicy(root: string, home: string): SkillPolicy {
  const { skills } = readPiSettings(root);
  if (skills === undefined) return { disabled: new Set(), userDirs: [] };
  if (skills === null || typeof skills !== "object" || Array.isArray(skills)) {
    throw new Error(".corral-next/pi.toml: skills must be a table");
  }
  const { disabled, user_dirs } = skills as Record<string, unknown>;
  function strings(key: string, value: unknown): string[] {
    if (value === undefined) return [];
    if (!Array.isArray(value) || value.some(item => typeof item !== "string" || !item || item.trim() !== item || item.includes("\0"))) {
      throw new Error(`.corral-next/pi.toml: skills.${key} must be an array of nonempty strings`);
    }
    return value;
  }
  function real(path: string): string {
    try { return realpathSync(path); } catch { return resolve(path); }
  }
  const actualHome = real(home);
  const configured = strings("user_dirs", user_dirs);
  if (configured.length > 32) throw new Error("too many configured Pi skill directories");
  const userDirs = configured.flatMap((entry) => {
    if (!isAbsolute(entry) && !entry.startsWith("~/")) {
      throw new Error(".corral-next/pi.toml: skills.user_dirs entries must be absolute or start with ~/");
    }
    const dir = resolve(entry.startsWith("~/") ? join(home, entry.slice(2)) : entry);
    const rel = relative(real(dir), actualHome);
    if (rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel))) {
      throw new Error(".corral-next/pi.toml: skills.user_dirs cannot include home or an ancestor of home");
    }
    try {
      for (let path = dir; ; path = dirname(path)) {
        if (!lstatSync(path).isDirectory()) return [];
        if (dirname(path) === path) return [dir];
      }
    } catch { return []; }
  });
  return { disabled: new Set(strings("disabled", disabled)), userDirs: [...new Set(userDirs)] };
}

/** Explicit roots only: inspect ordinary files and directories, never follow nested links. */
export function explicitSkillPaths(roots: readonly string[]): string[] {
  let visited = 0;
  function file(path: string): string[] {
    const stat = lstatSync(path, { throwIfNoEntry: false });
    return stat?.isFile() && stat.size <= 256 * 1024 ? [path] : [];
  }
  function scan(dir: string, depth: number): string[] {
    if (depth > 16) throw new Error("Pi skill directory nesting is too deep");
    let handle;
    try { handle = opendirSync(dir); } catch { return []; }
    const entries: Dirent[] = [];
    try {
      for (let entry; (entry = handle.readSync()) !== null;) {
        if (++visited > 4096) throw new Error("too many Pi skill directory entries");
        if (!entry.name.startsWith(".") && entry.name !== "node_modules") entries.push(entry);
      }
    } finally { handle.closeSync(); }
    if (entries.some(entry => entry.name === "SKILL.md" && entry.isFile())) return file(join(dir, "SKILL.md"));
    entries.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
    return entries.flatMap(entry => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return scan(path, depth + 1);
      return depth === 0 && entry.isFile() && entry.name.endsWith(".md") ? file(path) : [];
    });
  }
  return roots.flatMap(dir => scan(dir, 0));
}

/** Pi resolves precedence first; filtering the winner here never revives a shadowed copy. */
export function applySkillPolicy(result: LoadSkillsResult, policy: SkillPolicy, assetRoot?: string): LoadSkillsResult {
  const required = new Set(assetRoot === undefined ? [] : [WORKER_SKILL_ASSET, PLAN_SKILL_ASSET]
    .map(path => resolve(assetRoot, path)));
  const skills = result.skills.filter(skill => !policy.disabled.has(skill.name) || required.has(skill.filePath));
  // Suppress only collisions for names we deliberately hid, not malformed-file warnings
  // or real conflicts between enabled sources.
  return { skills, diagnostics: result.diagnostics.filter(item =>
    !item.collision || !policy.disabled.has(item.collision.name) || required.has(item.collision.winnerPath)) };
}
