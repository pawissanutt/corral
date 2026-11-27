import { lstatSync, readdirSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

import type { LoadSkillsResult } from "@earendil-works/pi-coding-agent";

import { DIAGRAM_SKILL_ASSET, PLAN_SKILL_ASSET, WORKER_SKILL_ASSET } from "../roles/common/assets.ts";
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
  const userDirs = strings("user_dirs", user_dirs).flatMap((entry) => {
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
  function scan(dir: string, root: boolean): string[] {
    try {
      const entries = readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
      if (entries.some(entry => entry.name === "SKILL.md" && entry.isFile())) return [join(dir, "SKILL.md")];
      return entries.flatMap(entry => {
        if (entry.name.startsWith(".") || entry.name === "node_modules") return [];
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return scan(path, false);
        return root && entry.isFile() && entry.name.endsWith(".md") ? [path] : [];
      });
    } catch { return []; }
  }
  return roots.flatMap(dir => scan(dir, true));
}

/** Pi resolves precedence first; filtering the winner here never revives a shadowed copy. */
export function applySkillPolicy(result: LoadSkillsResult, policy: SkillPolicy, assetRoot?: string): LoadSkillsResult {
  const required = new Set(assetRoot === undefined ? [] : [WORKER_SKILL_ASSET, PLAN_SKILL_ASSET, DIAGRAM_SKILL_ASSET]
    .map(path => resolve(assetRoot, path)));
  const skills = result.skills.filter(skill => !policy.disabled.has(skill.name) || required.has(skill.filePath));
  const visible = new Set(skills.map(skill => skill.filePath));
  return { skills, diagnostics: result.diagnostics.filter(item => item.type !== "collision" && item.path !== undefined && visible.has(item.path)) };
}
